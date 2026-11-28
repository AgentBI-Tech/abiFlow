use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::domain::audit::redact_value;
use crate::domain::circuit_breaker::{self, DEFAULT_CONFIDENCE_THRESHOLD};
use crate::domain::gateway::{self, Actor, Decision, Mode};
use crate::domain::llm::LlmProvider;
use crate::domain::reviewer;
use crate::domain::sop::{effective_owner, eval_condition, NodeType, SopDefinition};
use crate::infra::store::{self, GateState};
use crate::models::{new_id, now_secs, AppError, AppResult};

/// 工具执行器：配置驱动。`scope` 是 tools.scope_json 的解析结果，
/// 决定工具行为（builtin demo 或通用 HTTP），执行器不绑定任何具体站点。
pub trait ToolRunner {
    fn run(&self, tool: &str, args: &Value, scope: &Value) -> AppResult<Value>;
}

/// 内置演示工具（kind=builtin）：只读、合成数据、不连真实店铺/支付。供测试与未配置 scope 的兜底。
pub struct DemoTools;

impl ToolRunner for DemoTools {
    fn run(&self, tool: &str, args: &Value, _scope: &Value) -> AppResult<Value> {
        match tool {
            "demo:check_anomalies" => {
                let n = args.get("anomalies").and_then(|v| v.as_i64()).unwrap_or(0);
                Ok(json!({
                    "synthetic": true,
                    "anomalies": n,
                    "note": "演示数据（非真实店铺）",
                    "checked_at": now_secs()
                }))
            }
            _ => Err(AppError::McpUnavailable(format!("无此内置工具：{}", tool))),
        }
    }
}

fn node_type_str(t: NodeType) -> &'static str {
    match t {
        NodeType::Start => "start",
        NodeType::AgentStep => "agent_step",
        NodeType::McpTool => "mcp_tool",
        NodeType::HumanGate => "human_gate",
        NodeType::Condition => "condition",
        NodeType::Acceptance => "acceptance",
        NodeType::End => "end",
    }
}

/// 工具结果是否要求人工介入（内置 browser 遇登录/验证码时返回 requires_human）。
/// MCP 工具Shape 是 {content:[{type:"text",text:"...json..."}]}；文本里可能直接是 JSON。
fn raw_requires_human(raw: &Value) -> bool {
    if raw.get("requires_human").and_then(|v| v.as_bool()).unwrap_or(false) {
        return true;
    }
    let text = raw.get("content")
        .and_then(|c| c.get(0))
        .and_then(|x| x.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    if text.contains("\"requires_human\":true") || text.contains("\"requires_human\": true") {
        return true;
    }
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return v.get("requires_human").and_then(|b| b.as_bool()).unwrap_or(false);
    }
    false
}

/// 选边：优先 when 求值为真的分支，其次默认（无 when）分支。
fn choose_next(
    def: &SopDefinition,
    from: &str,
    outputs: &HashMap<String, Value>,
) -> AppResult<Option<String>> {
    let edges = def.outgoing(from);
    for e in &edges {
        if let Some(w) = &e.when {
            if eval_condition(w, outputs)? {
                return Ok(Some(e.to.clone()));
            }
        }
    }
    for e in &edges {
        if e.when.is_none() {
            return Ok(Some(e.to.clone()));
        }
    }
    Ok(None)
}

/// v0.5：human-owned 的 agent_step/mcp_tool 在执行前先过一次 owner 确认门。
/// 返回 Some(status) 表示本步应暂停（wait_human）或终止（failed），None 表示可继续。
/// 资金/不可逆工具由调用方排除（它们走既有 high_risk/irreversible 门，永不放权）。
fn owner_review_gate(
    conn: &Connection,
    run_id: &str,
    step_id: &str,
    current: &str,
    outputs: &HashMap<String, Value>,
    skipped: &HashSet<String>,
) -> AppResult<Option<String>> {
    let owner: String = conn
        .query_row("SELECT owner FROM steps WHERE id=?1", params![step_id], |r| r.get(0))
        .unwrap_or_else(|_| "agent".into());
    if owner != "human" {
        return Ok(None);
    }
    match store::gate_state_for_step(conn, step_id)? {
        GateState::None => {
            store::insert_gate(
                conn,
                run_id,
                step_id,
                "owner_review",
                "该步骤当前由人工负责，请确认是否采纳 Agent 的产出",
                None,
            )?;
            store::update_step(conn, step_id, "wait_human", None, None)?;
            store::update_run_status(conn, run_id, "wait_human", None, None)?;
            store::update_run_cursor(
                conn,
                run_id,
                Some(current),
                &serde_json::to_string(outputs).unwrap_or_else(|_| "{}".into()),
                &serde_json::to_string(skipped).unwrap_or_else(|_| "[]".into()),
            )?;
            Ok(Some("wait_human".into()))
        }
        GateState::Pending => Ok(Some("wait_human".into())),
        GateState::Rejected => {
            store::update_step(conn, step_id, "failed", None, None)?;
            store::update_run_status(conn, run_id, "failed", Some("fail"), Some("人工驳回该步骤产出"))?;
            Ok(Some("failed".into()))
        }
        GateState::Approved => Ok(None),
    }
}

/// 启动一条 run：固定定义快照 + 建 step 行 + 进入 running 并推进。
/// 只创建 run（落库 + 建 step 行 + status=running），**不推进**。
/// 命令层用它先返回 runId，再由 runner::spawn_run 在后台线程里 advance。
pub fn create_run(
    conn: &Connection,
    sop_id: &str,
    mode: &str,
    trigger_by: &str,
    input: &Value,
) -> AppResult<String> {
    let sop = store::get_sop(conn, sop_id)?;
    if !sop.enabled {
        return Err(AppError::InvalidInput(format!("SOP {} 已停用", sop_id)));
    }
    let def: SopDefinition = serde_json::from_str(&sop.definition)
        .map_err(|e| AppError::InvalidInput(format!("SOP 定义非法：{}", e)))?;
    let known = store::known_tools(conn)?;
    def.validate(&known)?;

    let run_id = store::insert_run(conn, sop_id, sop.version, mode, trigger_by, &sop.definition)?;

    // 为每个节点建一条 step 行（含 start/end，便于看板展示完整路径）。
    // owner 由节点类型 + config.owner 决定（v0.5）：human_gate/acceptance 恒 human，
    // agent_step/mcp_tool 可被标为 human（待人确认产出），是经验放权的升级对象。
    for (i, n) in def.nodes.iter().enumerate() {
        let owner = effective_owner(n);
        let tool_ref = if n.node_type == NodeType::McpTool {
            n.config.get("tool").and_then(|v| v.as_str()).map(|s| s.to_string())
        } else {
            None
        };
        store::insert_step(
            conn,
            &new_id("step"),
            &run_id,
            i as i32,
            node_type_str(n.node_type),
            owner,
            Some(&n.id),
            tool_ref.as_deref(),
        )?;
    }

    // 初始 outputs 放入 input（键 "input"），供条件表达式引用。
    let mut outputs = HashMap::new();
    outputs.insert("input".to_string(), input.clone());
    let outputs_json = serde_json::to_string(&outputs).unwrap_or_else(|_| "{}".into());
    store::update_run_cursor(conn, &run_id, None, &outputs_json, "[]")?;
    store::update_run_status(conn, &run_id, "running", None, None)?;

    Ok(run_id)
}

/// 同步创建并推进（保留给单测与需要阻塞结果的调用方）。
/// 命令层的用户触发入口改用 create_run + runner::spawn_run 实现异步可观察推进。
pub fn start_run(
    conn: &Connection,
    sop_id: &str,
    mode: &str,
    trigger_by: &str,
    input: &Value,
    tools: &dyn ToolRunner,
    llm: &dyn LlmProvider,
) -> AppResult<String> {
    let run_id = create_run(conn, sop_id, mode, trigger_by, input)?;
    advance(conn, &run_id, tools, llm)?;
    Ok(run_id)
}

/// 推进一条 run 直到：完成 / 失败 / 等人工。返回最终状态字符串。
pub fn advance(conn: &Connection, run_id: &str, tools: &dyn ToolRunner, llm: &dyn LlmProvider) -> AppResult<String> {
    let run = store::get_run(conn, run_id)?;

    // 不变量 1：wait_human 的 run 不得推进，除非对应 gate 已裁决。
    if run.status == "wait_human" {
        let has_pending = store::list_gates(conn, true)?
            .iter()
            .any(|g| g.run_id == run_id);
        if has_pending {
            return Ok("wait_human".into());
        }
        store::update_run_status(
            conn,
            run_id,
            "failed",
            Some("fail"),
            Some("状态机违例：wait_human 无待裁决人工门"),
        )?;
        return Ok("failed".into());
    }
    if matches!(run.status.as_str(), "done" | "failed" | "cancelled") {
        return Ok(run.status.clone());
    }
    if run.status != "running" {
        return Err(AppError::InvalidInput(format!("run 状态非法：{}", run.status)));
    }

    let def: SopDefinition = serde_json::from_str(
        run.definition_snapshot
            .as_deref()
            .ok_or_else(|| AppError::InvalidInput("run 缺定义快照".into()))?,
    )
    .map_err(|e| AppError::InvalidInput(format!("定义快照非法：{}", e)))?;

    let mut outputs: HashMap<String, Value> =
        serde_json::from_str(&run.outputs_json).unwrap_or_default();
    let mut skipped: HashSet<String> =
        serde_json::from_str(&run.branch_skipped).unwrap_or_default();

    let start_id = def
        .nodes
        .iter()
        .find(|n| n.node_type == NodeType::Start)
        .map(|n| n.id.clone())
        .ok_or_else(|| AppError::InvalidInput("AST 缺 start 节点".into()))?;
    let mut current = match run.current_node.as_deref() {
        Some(n) => n.to_string(),
        None => start_id.clone(),
    };

    const MAX_STEPS: usize = 200;
    for _ in 0..MAX_STEPS {
        let node = def
            .node(&current)
            .ok_or_else(|| AppError::InvalidInput(format!("节点不存在：{}", current)))?;

        match node.node_type {
            NodeType::Start => {
                let step_id = store::step_id_for_node(conn, run_id, &current)?;
                if let Some(id) = step_id {
                    store::update_step(conn, &id, "done", None, None)?;
                }
                current = choose_next(&def, &current, &outputs)?
                    .ok_or_else(|| AppError::InvalidInput("start 无后继".into()))?;
            }
            NodeType::End => {
                let step_id = store::step_id_for_node(conn, run_id, &current)?;
                if let Some(id) = step_id {
                    store::update_step(conn, &id, "done", None, None)?;
                }
                let acc = store::list_acceptance(conn, run_id)?;
                let all_pass = !acc.is_empty() && acc.iter().all(|a| a.passed);
                store::update_run_cursor(conn, run_id, None, &serde_json::to_string(&outputs).unwrap_or_default(), "[]")?;
                if all_pass {
                    store::update_run_status(conn, run_id, "done", Some("pass"), None)?;
                    return Ok("done".into());
                }
                store::update_run_status(conn, run_id, "done", Some("partial"), None)?;
                return Ok("done".into());
            }
            NodeType::McpTool => {
                let tool = node
                    .config
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AppError::InvalidInput("mcp_tool 缺 tool".into()))?;
                let step_id = store::step_id_for_node(conn, run_id, &current)?
                    .ok_or_else(|| AppError::InvalidInput("缺 tool step 行".into()))?;

                // v0.3：统一走 gateway 授权（Allow/Deny/NeedGate）；不变量 2/3 在此集中判定。
                let spec = store::tool_spec(conn, tool)?;
                // v0.5：human-owned 且非资金/不可逆工具 → 执行前先过 owner 确认门。
                if !(spec.irreversible || spec.risk == "high" || spec.requires_approval) {
                    if let Some(status) =
                        owner_review_gate(conn, run_id, &step_id, &current, &outputs, &skipped)?
                    {
                        return Ok(status);
                    }
                }
                let mode = Mode::parse(&run.mode)?;
                // 用户权限接入（2026-11-24）：run 按当前登录用户角色判定可见数据面/人工门；
                // 未登录回退 agent，保持既有行为不变。
                let actor = match crate::domain::user::current_user() {
                    Some(u) => Actor::human(&u.id, &u.role),
                    None => Actor::agent(),
                };
                let ctx = store::call_ctx(conn, run_id, actor, mode, tool)?;
                let decision = gateway::authorize(&spec, &ctx);
                match decision {
                    Decision::Deny => {
                        return Err(AppError::ToolDenied(format!(
                            "{}（{}）", tool, gateway::deny_reason(&spec, &ctx)
                        )))
                    }
                    Decision::NeedGate | Decision::Allow => {}
                }
                // irreversible/high/requiresApproval → NeedGate，且必须先有人工批准（不变量 2/3）。
                if decision == Decision::NeedGate
                    && store::gate_state_for_step(conn, &step_id)? != GateState::Approved
                {
                    if store::gate_state_for_step(conn, &step_id)? == GateState::None {
                        let reason = if spec.irreversible { "irreversible" } else { "high_risk" };
                        store::insert_gate(
                            conn,
                            run_id,
                            &step_id,
                            reason,
                            &format!("工具 {} 需人工批准后才能调用", tool),
                            None,
                        )?;
                        store::update_step(conn, &step_id, "wait_human", None, None)?;
                        store::update_run_status(conn, run_id, "wait_human", None, None)?;
                        store::update_run_cursor(
                            conn,
                            run_id,
                            Some(&current),
                            &serde_json::to_string(&outputs).unwrap_or_default(),
                            &serde_json::to_string(&skipped).unwrap_or_else(|_| "[]".into()),
                        )?;
                        return Ok("wait_human".into());
                    }
                    return Ok("wait_human".into());
                }

                let args = node.config.get("args").cloned().unwrap_or_else(|| json!({}));
                store::update_step(conn, &step_id, "running", None, None)?;
                let scope = store::tool_config(conn, tool)?;
                let raw = tools.run(tool, &args, &scope)?;
                // 内置 browser 等工具遇到登录/验证码时返回 requires_human → 插人工门转人工，不绕过。
                if raw_requires_human(&raw) {
                    let note = raw["content"][0]["text"].as_str().unwrap_or("工具要求人工介入");
                    store::insert_gate(
                        conn, run_id, &step_id, "requires_human", note, None,
                    )?;
                    store::update_step(conn, &step_id, "wait_human", None, None)?;
                    store::update_run_status(conn, run_id, "wait_human", None, Some(note))?;
                    store::update_run_cursor(
                        conn, run_id, Some(&current),
                        &serde_json::to_string(&outputs).unwrap_or_default(),
                        &serde_json::to_string(&skipped).unwrap_or_else(|_| "[]".into()),
                    )?;
                    return Ok("wait_human".into());
                }
                let safe = redact_value(&raw);
                let out_str = serde_json::to_string(&safe).unwrap_or_else(|_| "{}".into());
                let in_str = serde_json::to_string(&redact_value(&args)).unwrap_or_else(|_| "{}".into());
                store::insert_log(
                    conn,
                    &step_id,
                    "agent",
                    Some(tool),
                    Some(&in_str),
                    Some(&out_str),
                    Some("ok"),
                )?;
                store::update_step(conn, &step_id, "done", Some(&in_str), Some(&out_str))?;
                outputs.insert(current.clone(), safe);
                current = choose_next(&def, &current, &outputs)?
                    .ok_or_else(|| AppError::InvalidInput("mcp_tool 无后继".into()))?;
            }
            NodeType::AgentStep => {
                let step_id = store::step_id_for_node(conn, run_id, &current)?;
                // v0.5：human-owned 的 agent_step 先过 owner 确认门（LLM 步不涉及资金/不可逆）。
                if let Some(ref id) = step_id {
                    if let Some(status) =
                        owner_review_gate(conn, run_id, id, &current, &outputs, &skipped)?
                    {
                        return Ok(status);
                    }
                }
                let goal = node
                    .config
                    .get("goal")
                    .and_then(|v| v.as_str())
                    .unwrap_or("汇总上游结果并给出处置建议");
                // 把上游工具输出作为上下文喂给 LLM（只传本地事实，不跨店比绝对值）。
                let prompt = format!(
                    "你是电商运营助手。任务：{}\n已知的上游步骤输出（JSON）：{}\n\
                     约束：不编造金额、不下合规判定、不替人做资金/不可逆决定；\
                     有异常建议转人工。用简短 JSON 给出结论。",
                    goal,
                    serde_json::to_string(&outputs).unwrap_or_else(|_| "{}".into())
                );
                let generated = llm.generate(&prompt)?;
                let mut safe = redact_value(&serde_json::from_str::<Value>(&generated)
                    .unwrap_or_else(|_| json!({"text": generated})));
                // 标注实际 provider/mock 到产物本身——持久化的 step 输出与日志也要能追溯来源。
                if let Some(obj) = safe.as_object_mut() {
                    obj.insert("provider".into(), json!(llm.name()));
                    obj.insert("mock".into(), json!(llm.is_mock()));
                }

                // 熔断（§5.2 转人工判定③）：置信度低 / 原地打转 → 停人工门，不把无进展产物写入 outputs。
                let threshold = node
                    .config
                    .get("confidence_threshold")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(DEFAULT_CONFIDENCE_THRESHOLD);
                if let Some(reason) = circuit_breaker::check_agent_output(&safe, &outputs, threshold) {
                    let (gate_reason, prompt, payload, log_detail) = match &reason {
                        circuit_breaker::BreakReason::LowConfidence { score, threshold } => (
                            "low_confidence",
                            format!("Agent 置信度 {:.2} 低于阈值 {:.2}，请人工核实后放行或驳回", score, threshold),
                            json!({"score": score, "threshold": threshold}).to_string(),
                            "熔断：低置信度",
                        ),
                        circuit_breaker::BreakReason::Loop { same_as } => (
                            "loop",
                            format!("Agent 产出与步骤 {} 完全相同，疑似原地打转，请人工介入", same_as),
                            json!({"same_as": same_as}).to_string(),
                            "熔断：原地打转",
                        ),
                    };
                    if let Some(ref id) = step_id {
                        store::update_step(conn, id, "wait_human", None, None)?;
                        store::insert_gate(conn, run_id, id, gate_reason, &prompt, Some(&payload))?;
                        // verdict 受 CHECK 限制（ok/fail/needs_human）；原因写 output_redact。
                        store::insert_log(conn, id, "agent", Some(llm.name()), None, Some(log_detail), Some("needs_human"))?;
                    }
                    store::update_run_status(conn, run_id, "wait_human", None, Some(log_detail))?;
                    store::update_run_cursor(
                        conn,
                        run_id,
                        Some(&current),
                        &serde_json::to_string(&outputs).unwrap_or_else(|_| "{}".into()),
                        &serde_json::to_string(&skipped).unwrap_or_else(|_| "[]".into()),
                    )?;
                    return Ok("wait_human".into());
                }

                let out_str = serde_json::to_string(&safe).unwrap_or_else(|_| "{}".into());
                if let Some(ref id) = step_id {
                    store::update_step(conn, id, "done", None, Some(&out_str))?;
                    store::insert_log(
                        conn, id, "agent", Some(llm.name()), None, Some(&out_str), Some("ok"),
                    )?;
                }
                outputs.insert(current.clone(), safe);
                current = choose_next(&def, &current, &outputs)?
                    .ok_or_else(|| AppError::InvalidInput("agent_step 无后继".into()))?;
            }
            NodeType::Condition => {
                let to = choose_next(&def, &current, &outputs)?
                    .ok_or_else(|| AppError::InvalidInput("condition 无匹配分支".into()))?;
                // 被跳过分支的 step 标记为 skipped。
                for e in def.outgoing(&current) {
                    if e.to != to {
                        if let Some(id) = store::step_id_for_node(conn, run_id, &e.to)? {
                            store::update_step(conn, &id, "skipped", None, None)?;
                        }
                        skipped.insert(e.to.clone());
                    }
                }
                let step_id = store::step_id_for_node(conn, run_id, &current)?;
                if let Some(id) = step_id {
                    store::update_step(conn, &id, "done", None, None)?;
                }
                current = to;
            }
            NodeType::HumanGate => {
                let step_id = store::step_id_for_node(conn, run_id, &current)?
                    .ok_or_else(|| AppError::InvalidInput("缺 gate step 行".into()))?;
                match store::gate_state_for_step(conn, &step_id)? {
                    GateState::None => {
                        let reason = node.config.get("reason").and_then(|v| v.as_str()).unwrap_or("human_gate");
                        let prompt = node.config.get("prompt").and_then(|v| v.as_str()).unwrap_or("请确认是否继续");
                        store::insert_gate(conn, run_id, &step_id, reason, prompt, None)?;
                        store::update_step(conn, &step_id, "wait_human", None, None)?;
                        store::update_run_status(conn, run_id, "wait_human", None, None)?;
                        store::update_run_cursor(
                            conn,
                            run_id,
                            Some(&current),
                            &serde_json::to_string(&outputs).unwrap_or_default(),
                            &serde_json::to_string(&skipped).unwrap_or_else(|_| "[]".into()),
                        )?;
                        return Ok("wait_human".into());
                    }
                    GateState::Pending => {
                        return Ok("wait_human".into());
                    }
                    GateState::Approved => {
                        store::update_step(conn, &step_id, "done", None, None)?;
                        current = choose_next(&def, &current, &outputs)?
                            .ok_or_else(|| AppError::InvalidInput("gate 无后继".into()))?;
                    }
                    GateState::Rejected => {
                        store::update_step(conn, &step_id, "failed", None, None)?;
                        store::update_run_status(conn, run_id, "failed", Some("fail"), Some("人工驳回"))?;
                        return Ok("failed".into());
                    }
                }
            }
            NodeType::Acceptance => {
                let criteria = node
                    .config
                    .get("criteria")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                // 先收齐本轮评审结论，再判是否打回（避免部分写入后又回滚）。
                let mut verdicts: Vec<(String, reviewer::ReviewVerdict)> = Vec::new();
                for c in &criteria {
                    // 评审者核对：规则能求值走 rule，模糊标准（或 mode=llm）走 LLM。
                    let verdict = reviewer::review(c, &outputs, llm)?;
                    let expr = c.as_str().map(|s| s.to_string())
                        .or_else(|| c.get("criterion").and_then(|v| v.as_str()).map(|s| s.to_string()))
                        .unwrap_or_else(|| c.to_string());
                    verdicts.push((expr, verdict));
                }
                let any_fail = verdicts.iter().any(|(_, v)| !v.passed);
                if any_fail {
                    // 评审不过 = 打回。资金/不可逆动作在任何情况下都不因打回自动放行，
                    // 仍由各自的人工门管辖（本分支只改 run 状态与验收留痕，不碰工具授权）。
                    let n = store::bump_reject_count(conn, run_id)?;
                    for (expr, v) in &verdicts {
                        let note = format!("第{}轮打回：{}", n, v.note);
                        store::insert_acceptance(
                            conn, run_id, expr, v.passed, v.checked_by, Some(&note),
                        )?;
                    }
                    if let Some(id) = store::step_id_for_node(conn, run_id, &current)? {
                        store::update_step(conn, &id, "failed", None, None)?;
                    }
                    // status 沿用既有 CHECK 允许的 rejected；轮次靠 reject_count 区分，
                    // 仲裁理由写 note（note 无 CHECK），不引入新的 status/verdict 取值。
                    if n >= 2 {
                        store::update_run_status(
                            conn, run_id, "rejected", None,
                            Some("验收连续打回两轮，待人工仲裁"),
                        )?;
                        return Ok("rejected".into());
                    }
                    store::update_run_status(
                        conn, run_id, "rejected", None,
                        Some("验收第 1 轮打回，可重试"),
                    )?;
                    return Ok("rejected".into());
                }
                // 全部通过：写入验收留痕并前进（不变量 4 仍需 End 复核）。
                for (expr, v) in &verdicts {
                    store::insert_acceptance(
                        conn, run_id, &expr, v.passed, v.checked_by, Some(&v.note),
                    )?;
                }
                let step_id = store::step_id_for_node(conn, run_id, &current)?;
                if let Some(id) = step_id {
                    store::update_step(conn, &id, "done", None, None)?;
                }
                current = choose_next(&def, &current, &outputs)?
                    .ok_or_else(|| AppError::InvalidInput("acceptance 无后继".into()))?;
            }
        }

        // 持久化游标，便于崩溃后从当前节点恢复。
        store::update_run_cursor(
            conn,
            run_id,
            Some(&current),
            &serde_json::to_string(&outputs).unwrap_or_default(),
            &serde_json::to_string(&skipped).unwrap_or_else(|_| "[]".into()),
        )?;
    }

    store::update_run_status(conn, run_id, "failed", Some("fail"), Some("超出最大步数"))?;
    Ok("failed".into())
}

/// 人工裁决后恢复 run：把状态从 wait_human 改回 running，再推进。
pub fn resume_after_gate(
    conn: &Connection,
    run_id: &str,
    tools: &dyn ToolRunner,
    llm: &dyn LlmProvider,
) -> AppResult<String> {
    let run = store::get_run(conn, run_id)?;
    if run.status != "wait_human" {
        return Ok(run.status);
    }
    store::update_run_status(conn, run_id, "running", None, None)?;
    advance(conn, run_id, tools, llm)
}

/// 打回第 1 轮后的重试：清空本轮验收留痕、游标回退到起点重跑一遍（不重置 reject_count，
/// 再不过即第 2 轮打回→转 review）。资金/不可逆动作仍走各自人工门，不因重试自动放行。
pub fn retry_rejected(
    conn: &Connection,
    run_id: &str,
    tools: &dyn ToolRunner,
    llm: &dyn LlmProvider,
) -> AppResult<String> {
    let run = store::get_run(conn, run_id)?;
    if run.status != "rejected" || run.reject_count >= 2 {
        return Ok(run.status);
    }
    store::clear_acceptance(conn, run_id)?;
    store::update_run_cursor(conn, run_id, None, &run.outputs_json, "[]")?;
    store::update_run_status(conn, run_id, "running", None, Some("第 1 轮打回后重试"))?;
    advance(conn, run_id, tools, llm)
}

/// 人工仲裁（打回两轮后）：resume = 清零打回计数与验收留痕后重跑（给一次带修正的机会）；
/// fail = 直接判失败。仅 rejected 且 reject_count>=2（即打回两轮待仲裁）可调用。
pub fn arbitrate(
    conn: &Connection,
    run_id: &str,
    decision: &str,
    note: Option<&str>,
    tools: &dyn ToolRunner,
    llm: &dyn LlmProvider,
) -> AppResult<String> {
    let run = store::get_run(conn, run_id)?;
    if run.status != "rejected" || run.reject_count < 2 {
        return Err(AppError::InvalidInput("仅打回两轮待仲裁的 run 可人工仲裁".into()));
    }
    match decision {
        "resume" => {
            store::clear_acceptance(conn, run_id)?;
            store::reset_reject_count(conn, run_id)?;
            store::update_run_cursor(conn, run_id, None, &run.outputs_json, "[]")?;
            let decision_note = note.unwrap_or("人工仲裁：放行重跑");
            store::update_run_status(conn, run_id, "running", None, Some(decision_note))?;
            let final_status = advance(conn, run_id, tools, llm)?;
            // advance 内打回/完成会重写 note，再把仲裁决定追加保留，确保可审计。
            store::append_run_note(conn, run_id, decision_note)?;
            Ok(final_status)
        }
        "fail" => {
            store::update_run_status(
                conn, run_id, "failed", Some("fail"),
                Some(note.unwrap_or("人工仲裁：判失败")),
            )?;
            Ok("failed".into())
        }
        _ => Err(AppError::InvalidInput(format!("非法仲裁决定：{}", decision))),
    }
}

/// 人工干预：暂停 / 恢复 / 取消 / 接管 / 改路径（§3 v1 第 2 条、§5.2）。
///
/// 上下文保留：四种干预都不重置游标与 outputs（不从头重读）。
/// 安全底线：resume 只放行「仅含 pause 门」的 run，绝不绕过真实人工门
/// （高风险/审批/owner_review/熔断/打回等）；资金与不可逆动作仍由各自门管辖。
///
/// - pause：running/pending → 插 reason=pause 门，run→wait_human（status CHECK 无 paused，复用 wait_human）。
/// - resume：仅当所有 pending 门都是 pause → 批准它们 + 续跑；否则拒绝（防绕过真实门）。
/// - takeover：把当前节点 step 的 owner 改 human + log(actor=human) + 插 takeover 门等人操作/裁决。
/// - change_path：value=目标节点 id，须在定义快照中存在；改游标后从新节点续跑。
pub fn control(
    conn: &Connection,
    run_id: &str,
    action: &str,
    value: Option<&str>,
    note: Option<&str>,
    tools: &dyn ToolRunner,
    llm: &dyn LlmProvider,
) -> AppResult<String> {
    let run = store::get_run(conn, run_id)?;
    match action {
        "cancel" => {
            if matches!(run.status.as_str(), "done" | "failed" | "cancelled") {
                return Err(AppError::InvalidInput("终态 run 不可取消".into()));
            }
            store::update_run_status(conn, run_id, "cancelled", None, Some(note.unwrap_or("用户取消")))?;
            Ok("cancelled".into())
        }
        "pause" => {
            if !matches!(run.status.as_str(), "running" | "pending") {
                return Err(AppError::InvalidInput(format!("仅运行中 run 可暂停，当前 {}", run.status)));
            }
            let pending = store::list_gates(conn, true)?
                .into_iter().filter(|g| g.run_id == run_id).collect::<Vec<_>>();
            if !pending.is_empty() {
                // 已有真实人工门待裁决：暂停无意义，直接停在 wait_human，不新增门掩盖它。
                return Ok("wait_human".into());
            }
            if let Some(node) = run.current_node.as_deref() {
                if let Some(sid) = store::step_id_for_node(conn, run_id, node)? {
                    store::insert_gate(conn, run_id, &sid, "pause", "流程已暂停，批准后从当前步骤继续", None)?;
                    store::update_step(conn, &sid, "wait_human", None, None)?;
                    if let Some(n) = note { store::append_run_note(conn, run_id, n)?; }
                }
            }
            store::update_run_status(conn, run_id, "wait_human", None, Some("已暂停（上下文保留）"))?;
            Ok("wait_human".into())
        }
        "resume" => {
            if run.status != "wait_human" {
                return Ok(run.status); // 非等待态无需恢复
            }
            let pending = store::list_gates(conn, true)?
                .into_iter().filter(|g| g.run_id == run_id).collect::<Vec<_>>();
            if pending.is_empty() {
                return Err(AppError::InvalidInput("无待裁决门，无需恢复".into()));
            }
            // 安全底线：只要有一个非 pause 门，就不许一键恢复。
            if pending.iter().any(|g| g.reason != "pause") {
                return Err(AppError::InvalidInput("存在真实人工门待裁决，请在人工待办中处理，不可用恢复绕过".into()));
            }
            for g in &pending {
                store::decide_gate(conn, &g.id, "approve", note)?;
            }
            if let Some(n) = note { store::append_run_note(conn, run_id, n)?; }
            resume_after_gate(conn, run_id, tools, llm)
        }
        "takeover" => {
            if matches!(run.status.as_str(), "done" | "failed" | "cancelled") {
                return Err(AppError::InvalidInput("终态 run 不可接管".into()));
            }
            if let Some(node) = run.current_node.as_deref() {
                if let Some(sid) = store::step_id_for_node(conn, run_id, node)? {
                    store::set_step_owner(conn, &sid, "human")?;
                    // verdict 受 CHECK 限制（ok/fail/needs_human）；细节写 output_redact。
                    store::insert_log(conn, &sid, "human", Some("takeover"), None, Some("人工接管当前步骤"), Some("needs_human"))?;
                    store::insert_gate(conn, run_id, &sid, "takeover", "已由人工接管，请处理后再批准继续", None)?;
                }
            }
            if let Some(n) = note { store::append_run_note(conn, run_id, n)?; }
            if run.status != "wait_human" {
                store::update_run_status(conn, run_id, "wait_human", None, Some("人工接管（上下文保留）"))?;
            }
            Ok("wait_human".into())
        }
        "change_path" => {
            let target = value.ok_or_else(|| AppError::InvalidInput("改路径需提供目标节点 id".into()))?;
            let def: SopDefinition = serde_json::from_str(
                run.definition_snapshot.as_deref()
                    .ok_or_else(|| AppError::InvalidInput("run 缺定义快照".into()))?,
            )
            .map_err(|e| AppError::InvalidInput(format!("定义快照非法：{}", e)))?;
            if def.node(target).is_none() {
                return Err(AppError::InvalidInput(format!("目标节点不存在于当前 SOP：{}", target)));
            }
            // 改的是本次 run 的剩余路径（游标），不改 SOP 定义；outputs/skipped 保留。
            store::update_run_cursor(conn, run_id, Some(target), &run.outputs_json, &run.branch_skipped)?;
            let detail = note.unwrap_or("人工改路径");
            store::append_run_note(conn, run_id, &format!("{}：→ {}", detail, target))?;
            if let Some(node) = run.current_node.as_deref() {
                if let Some(sid) = store::step_id_for_node(conn, run_id, node)? {
                    let detail_str = format!("{} → {}", detail, target);
                    store::insert_log(conn, &sid, "human", Some("change_path"), None, Some(&detail_str), Some("ok"))?;
                }
            }
            if run.status != "running" {
                // 从等待态改路径后直接续跑（资金/不可逆门仍由 advance 内原逻辑管辖）。
                store::update_run_status(conn, run_id, "running", None, None)?;
            }
            advance(conn, run_id, tools, llm)
        }
        _ => Err(AppError::InvalidInput(format!("未知控制动作：{}", action))),
    }
}

/// 安装演示 SOP + 内置工具 + 调度，供看板与端到端验收。
pub fn demo_install(conn: &Connection) -> AppResult<String> {
    // 工具表：注册只读 demo 工具。
    conn.execute(
        "INSERT OR IGNORE INTO tools(id,server,command,title,risk,scope_json,enabled) \
         VALUES('demo:check_anomalies','builtin','demo','异常巡检（演示）','low','{\"read_only\":true,\"plane\":\"public\"}',1)",
        [],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;

    // v0.3：为 demo 工具在三种模式都种授权（public/content 面），否则 gateway 判 Deny。
    for mode in [Mode::Plan, Mode::Build, Mode::Review] {
        store::upsert_tool_grant(
            conn,
            "demo:check_anomalies",
            mode,
            true,
            &["public".into(), "content".into()],
        )?;
    }

    let definition = json!({
        "version": 1,
        "nodes": [
            {"id":"n0","type":"start","config":{}},
            {"id":"n1","type":"mcp_tool","config":{"tool":"demo:check_anomalies","args":{"anomalies":3},"timeout_s":60}},
            {"id":"n2","type":"human_gate","config":{"reason":"anomaly_review","prompt":"发现 3 条演示异常，是否转人工处理？"}},
            {"id":"n3","type":"acceptance","config":{"criteria":["steps.n1.anomalies == 3"]}},
            {"id":"n4","type":"end","config":{}}
        ],
        "edges": [
            {"from":"n0","to":"n1"},
            {"from":"n1","to":"n2"},
            {"from":"n2","to":"n3"},
            {"from":"n3","to":"n4"}
        ]
    });
    let sop_id = "sop-demo-daily-anomaly";
    let existing = conn
        .query_row("SELECT version FROM sops WHERE id=?1", [&sop_id], |r| r.get::<_, i32>(0))
        .optional()
        .map_err(|e| AppError::Db(e.to_string()))?;
    if existing.is_none() {
        let now = now_secs();
        conn.execute(
            "INSERT INTO sops(id,name,version,trigger_json,risk,owner_policy,enabled,created_at,updated_at,definition) \
             VALUES(?1,'每日异常巡检（演示）',1,'{\"type\":\"interval\",\"secs\":3600}','low','hybrid',1,?2,?2,?3)",
            params![sop_id, now, definition.to_string()],
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
    }
    store::upsert_schedule(conn, sop_id, 3600)?;
    Ok(sop_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::MockProvider;
    use crate::infra::db;

    fn setup() -> Connection {
        let c = db::open_in_memory().unwrap();
        demo_install(&c).unwrap();
        c
    }

    #[test]
    fn demo_install_seeds_tool_sop_schedule() {
        let c = setup();
        assert_eq!(store::known_tools(&c).unwrap().len(), 1);
        assert_eq!(store::list_sops(&c, false).unwrap().len(), 1);
        assert_eq!(store::dashboard(&c).unwrap().schedules.len(), 1);
    }

    // 内置 browser 遇登录/验证码 → 工具返回 requires_human → 插人工门、run 转 wait_human。
    #[test]
    fn requires_human_tool_inserts_gate_and_waits() {
        struct HumanTool;
        impl ToolRunner for HumanTool {
            fn run(&self, tool: &str, _args: &Value, _scope: &Value) -> AppResult<Value> {
                assert_eq!(tool, "builtin:browser-navigate");
                Ok(json!({"content":[{"type":"text","text":
                    "{\"requires_human\":true,\"reason\":\"login\",\"note\":\"需人工登录\"}"}]}))
            }
        }
        let c = setup();
        let def = json!({
            "version":1,
            "nodes":[
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"mcp_tool","config":{"tool":"builtin:browser-navigate","args":{"url":"https://x"}}},
                {"id":"n2","type":"end","config":{}}
            ],
            "edges":[{"from":"n0","to":"n1"},{"from":"n1","to":"n2"}]
        });
        let p = crate::models::SopUpsertPayload {
            id: Some("sop-human".into()), name: "转人工".into(),
            trigger_json: "{}".into(), risk: "low".into(), owner_policy: "agent".into(),
            definition: def, expect_version: None,
        };
        store::upsert_sop(&c, &p).unwrap();
        // 注册工具（gateway 需已知工具才能放行）
        store::upsert_tool(&c, "builtin:browser-navigate", "builtin-browser", "mcp", "内置浏览器", "low",
            &json!({"kind":"mcp","server":"builtin-browser","tool":"navigate","read_only":true,"plane":"public"}).to_string(), true).unwrap();
        store::upsert_tool_grant(&c, "builtin:browser-navigate", Mode::Build, true, &["public".into()]).unwrap();

        let run_id = start_run(&c, "sop-human", "build", "manual", &json!({}), &HumanTool, &MockProvider).unwrap();
        let run = store::get_run(&c, &run_id).unwrap();
        assert_eq!(run.status, "wait_human");
        let gates = store::list_gates(&c, true).unwrap();
        assert!(gates.iter().any(|g| g.reason == "requires_human"), "应插入 requires_human 门");
    }

    #[test]
    fn run_stops_at_gate_and_cannot_advance() {
        let c = setup();
        let run_id = start_run(&c, "sop-demo-daily-anomaly", "build", "manual", &json!({}), &DemoTools, &MockProvider).unwrap();
        let run = store::get_run(&c, &run_id).unwrap();
        assert_eq!(run.status, "wait_human", "应在人工门停住");
        // 再次推进不应越过 wait_human（不变量 1）。
        let s = advance(&c, &run_id, &DemoTools, &MockProvider).unwrap();
        assert_eq!(s, "wait_human");
        let gates = store::list_gates(&c, true).unwrap();
        assert_eq!(gates.len(), 1);
    }

    #[test]
    fn approve_gate_resumes_to_done() {
        let c = setup();
        let run_id = start_run(&c, "sop-demo-daily-anomaly", "build", "manual", &json!({}), &DemoTools, &MockProvider).unwrap();
        let gate = &store::list_gates(&c, true).unwrap()[0];
        store::decide_gate(&c, &gate.id, "approve", None).unwrap();
        let s = resume_after_gate(&c, &run_id, &DemoTools, &MockProvider).unwrap();
        assert_eq!(s, "done");
        let run = store::get_run(&c, &run_id).unwrap();
        assert_eq!(run.verdict.as_deref(), Some("pass"));
        let acc = store::list_acceptance(&c, &run_id).unwrap();
        assert_eq!(acc.len(), 1);
        assert!(acc[0].passed);
        // 留了脱敏后的日志。
        let steps = store::list_steps(&c, &run_id).unwrap();
        assert!(steps.iter().any(|s| s.output_json.is_some()));
    }

    #[test]
    fn reject_gate_fails_run() {
        let c = setup();
        let run_id = start_run(&c, "sop-demo-daily-anomaly", "build", "manual", &json!({}), &DemoTools, &MockProvider).unwrap();
        let gate = &store::list_gates(&c, true).unwrap()[0];
        store::decide_gate(&c, &gate.id, "reject", None).unwrap();
        let s = resume_after_gate(&c, &run_id, &DemoTools, &MockProvider).unwrap();
        assert_eq!(s, "failed");
    }

    #[test]
    fn high_risk_tool_requires_gate() {
        let c = setup();
        // 把 demo 工具改成 high，独立 SOP 单步直调。
        c.execute("UPDATE tools SET risk='high' WHERE id='demo:check_anomalies'", []).unwrap();
        let def = json!({
            "version":1,
            "nodes":[
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"mcp_tool","config":{"tool":"demo:check_anomalies","args":{"anomalies":1}}},
                {"id":"n2","type":"end","config":{}}
            ],
            "edges":[{"from":"n0","to":"n1"},{"from":"n1","to":"n2"}]
        });
        let p = crate::models::SopUpsertPayload {
            id: Some("sop-high".into()),
            name: "高风险直调".into(),
            trigger_json: "{}".into(),
            risk: "high".into(),
            owner_policy: "human".into(),
            definition: def,
            expect_version: None,
        };
        store::upsert_sop(&c, &p).unwrap();
        let run_id = start_run(&c, "sop-high", "build", "manual", &json!({}), &DemoTools, &MockProvider).unwrap();
        assert_eq!(store::get_run(&c, &run_id).unwrap().status, "wait_human", "高风险必须先过人工门");
    }

    #[test]
    fn cancel_terminal_guard() {
        let c = setup();
        let run_id = start_run(&c, "sop-demo-daily-anomaly", "build", "manual", &json!({}), &DemoTools, &MockProvider).unwrap();
        let s = control(&c, &run_id, "cancel", None, None, &DemoTools, &MockProvider).unwrap();
        assert_eq!(s, "cancelled");
        assert!(control(&c, &run_id, "cancel", None, None, &DemoTools, &MockProvider).is_err(), "终态不可再取消");
    }
}

#[cfg(test)]
mod agent_step_llm_tests {
    use super::*;
    use crate::domain::llm::{LlmProvider, MockProvider};
    use crate::infra::db;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct SpyProvider {
        calls: Arc<AtomicUsize>,
        inner: MockProvider,
    }
    impl LlmProvider for SpyProvider {
        fn name(&self) -> &str { "spy-mock" }
        fn is_mock(&self) -> bool { true }
        fn generate(&self, prompt: &str) -> AppResult<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner.generate(prompt)
        }
    }

    /// 评审用：总是返回通过的 JSON 判定（非 mock）。
    struct PassVerdictProvider;
    impl LlmProvider for PassVerdictProvider {
        fn name(&self) -> &str { "verdict-pass" }
        fn is_mock(&self) -> bool { false }
        fn generate(&self, _prompt: &str) -> AppResult<String> {
            Ok(r#"{"passed":true,"note":"评审核实通过"}"#.into())
        }
    }

    #[test]
    fn agent_step_invokes_llm_and_records_provider() {
        let c = db::open_in_memory().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let spy = SpyProvider { calls: calls.clone(), inner: MockProvider };
        let def: serde_json::Value = serde_json::from_value(json!({
            "version":1,
            "nodes":[
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"agent_step","config":{"goal":"总结异常"}},
                {"id":"n2","type":"end","config":{}}
            ],
            "edges":[{"from":"n0","to":"n1"},{"from":"n1","to":"n2"}]
        })).unwrap();
        let p = crate::models::SopUpsertPayload {
            id: Some("sop-agent".into()), name: "agent 推理".into(),
            trigger_json: "{}".into(), risk: "low".into(), owner_policy: "agent".into(),
            definition: def, expect_version: None,
        };
        store::upsert_sop(&c, &p).unwrap();
        let run_id = start_run(&c, "sop-agent", "build", "manual", &json!({}), &DemoTools, &spy).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "agent_step 应调用 LLM 一次");
        let run = store::get_run(&c, &run_id).unwrap();
        assert_eq!(run.status, "done");
        let steps = store::list_steps(&c, &run_id).unwrap();
        let out: serde_json::Value = serde_json::from_str(steps[1].output_json.as_deref().unwrap()).unwrap();
        assert_eq!(out["provider"], "spy-mock", "产物应标注实际 provider");
        // 审计日志应记下 provider 名。
        assert_eq!(
            c.query_row::<String, _, _>(
                "SELECT tool_command FROM step_logs WHERE step_id=?1",
                rusqlite::params![steps[1].id], |r| r.get(0)
            ).unwrap_or_default(),
            "spy-mock"
        );
    }

    #[test]
    fn llm_mode_acceptance_uses_reviewer() {
        let c = db::open_in_memory().unwrap();
        demo_install(&c).unwrap();
        // 建一条 SOP：start → agent_step → acceptance(mode=llm) → end。
        let def = serde_json::json!({
            "version": 1,
            "nodes": [
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"agent_step","config":{"goal":"核对数据"}},
                {"id":"n2","type":"acceptance","config":{"criteria":[
                    {"criterion":"数据与后台一致","mode":"llm"}
                ]}},
                {"id":"n3","type":"end","config":{}}
            ],
            "edges": [
                {"from":"n0","to":"n1"},{"from":"n1","to":"n2"},{"from":"n2","to":"n3"}
            ]
        });
        let sop_id = "sop-review-llm";
        c.execute(
            "INSERT INTO sops(id,name,version,trigger_json,risk,owner_policy,enabled,created_at,updated_at,definition) \
             VALUES(?1,'评审测试',1,'{}','low','agent',1,0,0,?2)",
            rusqlite::params![sop_id, def.to_string()],
        ).unwrap();
        let run_id = start_run(&c, sop_id, "build", "manual", &serde_json::json!({}), &DemoTools, &PassVerdictProvider).unwrap();
        assert_eq!(store::get_run(&c, &run_id).unwrap().status, "done");
        let acc = store::list_acceptance(&c, &run_id).unwrap();
        assert_eq!(acc.len(), 1);
        assert!(acc[0].passed, "LLM 评审通过应判 pass");
        assert_eq!(acc[0].checked_by, "llm", "LLM 模式验收须记 checked_by=llm");
    }
}

/// 自然语言 → SOP AST 草稿（§3.1 sop_parse_text）：只返回草稿，不落库、不放行。
/// 草稿仍需人工确认后走 sop_upsert；validate 检查结构合法性，未知工具Warnings 一并返回给前端提示。
pub fn parse_sop_text(
    text: &str,
    llm: &dyn LlmProvider,
) -> AppResult<serde_json::Value> {
    if text.trim().is_empty() {
        return Err(AppError::InvalidInput("描述不得为空".into()));
    }
    let prompt = format!(
        "SOP_DRAFT: 把下面的运营需求解析成 abiFlow SOP AST（JSON）。\
         节点类型只用 start/agent_step/mcp_tool/human_gate/condition/acceptance/end。\
         资金/不可逆动作必须配 human_gate。需求：{}",
        text
    );
    let raw = llm.generate(&prompt)?;
    // 尝试从模型输出里截取 JSON（容忍前后有说明文字）。
    let json_str = extract_json_object(&raw)
        .ok_or_else(|| AppError::LlmError("LLM 未返回合法 JSON 草稿".into()))?;
    let draft: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| AppError::LlmError(format!("草稿 JSON 非法：{}", e)))?;
    // 结构校验（不校验工具是否已注册——草稿里可能用占位工具，交人工确认时再改）。
    let def: SopDefinition = serde_json::from_value(draft.clone())
        .map_err(|e| AppError::LlmError(format!("草稿结构不符合 AST：{}", e)))?;
    let _ = def; // 解析成功即结构基本合法
    Ok(draft)
}

fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(&s[start..=end])
}

#[cfg(test)]
mod sop_parse_tests {
    use super::*;
    use crate::domain::llm::MockProvider;
    use crate::infra::db;

    #[test]
    fn parse_text_returns_structural_draft_not_persisted() {
        let c = db::open_in_memory().unwrap();
        let before = store::list_sops(&c, false).unwrap().len();
        let draft = parse_sop_text("每日检查异常订单", &MockProvider).unwrap();
        assert_eq!(draft["version"], 1);
        assert!(draft["nodes"].as_array().unwrap().len() >= 3);
        let after = store::list_sops(&c, false).unwrap().len();
        assert_eq!(before, after, "草稿不得落库");
        assert_eq!(draft["_mock"], true);
    }

    #[test]
    fn empty_input_rejected() {
        assert!(parse_sop_text("  ", &MockProvider).is_err());
    }

    // ---- 验收打回两轮 → 人工仲裁（§5 状态机）----

    /// 建一条 SOP：start → demo 工具(anomalies=3) → acceptance(要求 ==99，规则通道必失败) → end。
    fn install_reject_sop(c: &Connection) {
        demo_install(c).unwrap(); // 注册 demo 工具与三模式授权
        let def = json!({
            "version": 1,
            "nodes": [
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"mcp_tool","config":{"tool":"demo:check_anomalies","args":{"anomalies":3}}},
                {"id":"n2","type":"acceptance","config":{"criteria":["steps.n1.anomalies == 99"]}},
                {"id":"n3","type":"end","config":{}}
            ],
            "edges": [
                {"from":"n0","to":"n1"},{"from":"n1","to":"n2"},{"from":"n2","to":"n3"}
            ]
        });
        let p = crate::models::SopUpsertPayload {
            id: Some("sop-reject".into()), name: "打回测试".into(),
            trigger_json: "{}".into(), risk: "low".into(), owner_policy: "agent".into(),
            definition: def, expect_version: None,
        };
        store::upsert_sop(c, &p).unwrap();
    }

    #[test]
    fn first_acceptance_failure_marks_rejected() {
        let c = db::open_in_memory().unwrap();
        install_reject_sop(&c);
        let run_id = start_run(&c, "sop-reject", "build", "manual", &json!({}), &DemoTools, &MockProvider).unwrap();
        let run = store::get_run(&c, &run_id).unwrap();
        assert_eq!(run.status, "rejected", "第 1 轮验收不过应打回");
        assert_eq!(run.reject_count, 1, "打回计数应为 1");
        let acc = store::list_acceptance(&c, &run_id).unwrap();
        assert!(acc.iter().all(|a| !a.passed), "评审不过的验收项应记 passed=false");
    }

    #[test]
    fn second_acceptance_failure_goes_to_review() {
        let c = db::open_in_memory().unwrap();
        install_reject_sop(&c);
        let run_id = start_run(&c, "sop-reject", "build", "manual", &json!({}), &DemoTools, &MockProvider).unwrap();
        // 第 1 轮打回后重试：游标回起点、清空本轮验收，再跑一次仍不过 → 第 2 轮 → review。
        let s = retry_rejected(&c, &run_id, &DemoTools, &MockProvider).unwrap();
        assert_eq!(s, "rejected", "状态沿用 rejected，轮次靠 reject_count 区分");
        let run = store::get_run(&c, &run_id).unwrap();
        assert_eq!(run.status, "rejected");
        assert_eq!(run.reject_count, 2, "第 2 轮打回计数为 2（待仲裁）");
        assert!(run.note.unwrap().contains("仲裁"), "note 标注待人工仲裁");
    }

    #[test]
    fn arbitrate_resume_clears_and_retries() {
        let c = db::open_in_memory().unwrap();
        install_reject_sop(&c);
        let run_id = start_run(&c, "sop-reject", "build", "manual", &json!({}), &DemoTools, &MockProvider).unwrap();
        retry_rejected(&c, &run_id, &DemoTools, &MockProvider).unwrap(); // → 打回两轮待仲裁
        assert_eq!(store::get_run(&c, &run_id).unwrap().reject_count, 2);
        // 仲裁放行：清零计数与验收留痕后重跑；规则仍不过 → 重新从第 1 轮打回开始。
        let s = arbitrate(&c, &run_id, "resume", Some("仲裁放行重跑"), &DemoTools, &MockProvider).unwrap();
        assert_eq!(s, "rejected", "放行后重跑仍不过，回到第 1 轮打回");
        let run = store::get_run(&c, &run_id).unwrap();
        assert_eq!(run.reject_count, 1, "仲裁放行应清零后再计第 1 轮");
        assert!(run.note.unwrap().contains("仲裁放行"), "仲裁说明应留痕到 run.note");
    }

    #[test]
    fn arbitrate_fail_marks_run_failed() {
        let c = db::open_in_memory().unwrap();
        install_reject_sop(&c);
        let run_id = start_run(&c, "sop-reject", "build", "manual", &json!({}), &DemoTools, &MockProvider).unwrap();
        retry_rejected(&c, &run_id, &DemoTools, &MockProvider).unwrap(); // → 打回两轮待仲裁
        let s = arbitrate(&c, &run_id, "fail", Some("证据不足判失败"), &DemoTools, &MockProvider).unwrap();
        assert_eq!(s, "failed");
        let run = store::get_run(&c, &run_id).unwrap();
        assert_eq!(run.status, "failed");
        assert_eq!(run.verdict.as_deref(), Some("fail"));
        assert!(run.note.unwrap().contains("证据不足判失败"));
    }

    #[test]
    fn arbitrate_rejects_illegal_state_and_decision() {
        let c = db::open_in_memory().unwrap();
        install_reject_sop(&c);
        let run_id = start_run(&c, "sop-reject", "build", "manual", &json!({}), &DemoTools, &MockProvider).unwrap();
        assert_eq!(store::get_run(&c, &run_id).unwrap().status, "rejected");
        // 非 review 状态不可仲裁
        assert!(arbitrate(&c, &run_id, "resume", None, &DemoTools, &MockProvider).is_err());
        // review 状态下非法决定被拒
        retry_rejected(&c, &run_id, &DemoTools, &MockProvider).unwrap(); // → review
        assert!(arbitrate(&c, &run_id, "bogus", None, &DemoTools, &MockProvider).is_err());
    }

    #[test]
    fn high_risk_tool_still_gated_on_retry() {
        // 资金/不可逆动作不因打回重试自动放行：把 demo 工具设为 high，
        // 第一次跑停在人工门（wait_human），不会自动进入验收打回。
        let c = db::open_in_memory().unwrap();
        install_reject_sop(&c);
        c.execute("UPDATE tools SET risk='high' WHERE id='demo:check_anomalies'", []).unwrap();
        let run_id = start_run(&c, "sop-reject", "build", "manual", &json!({}), &DemoTools, &MockProvider).unwrap();
        let s = store::get_run(&c, &run_id).unwrap().status;
        assert_eq!(s, "wait_human", "高风险工具必须停人工门，不得因打回机制绕过");
    }

    // ---- 熔断：低置信度 / 原地打转转人工（§5.2 判定③）----

    /// 固定返回一段 JSON 的 provider（非 mock），用于构造低置信度/打转产物。
    struct FixedJsonProvider {
        answer: String,
    }
    impl LlmProvider for FixedJsonProvider {
        fn name(&self) -> &str { "fixed-json" }
        fn is_mock(&self) -> bool { false }
        fn generate(&self, _prompt: &str) -> AppResult<String> { Ok(self.answer.clone()) }
    }

    fn install_agent_sop(c: &Connection, nodes: serde_json::Value, edges: serde_json::Value) {
        let def = json!({"version": 1, "nodes": nodes, "edges": edges});
        let p = crate::models::SopUpsertPayload {
            id: Some("sop-breaker".into()), name: "熔断测试".into(),
            trigger_json: "{}".into(), risk: "low".into(), owner_policy: "agent".into(),
            definition: def, expect_version: None,
        };
        store::upsert_sop(c, &p).unwrap();
    }

    #[test]
    fn low_confidence_stops_at_human_gate() {
        let c = db::open_in_memory().unwrap();
        install_agent_sop(&c,
            json!([
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"agent_step","config":{"goal":"核对"}},
                {"id":"n2","type":"end","config":{}}
            ]),
            json!([{"from":"n0","to":"n1"},{"from":"n1","to":"n2"}]));
        let llm = FixedJsonProvider { answer: r#"{"confidence":0.2,"text":"不确定"}"#.into() };
        let run_id = start_run(&c, "sop-breaker", "build", "manual", &json!({}), &DemoTools, &llm).unwrap();
        let run = store::get_run(&c, &run_id).unwrap();
        assert_eq!(run.status, "wait_human", "低置信度应停人工门");
        let gates = store::list_gates(&c, true).unwrap();
        assert!(gates.iter().any(|g| g.reason == "low_confidence"), "应有低置信度门");
        // 产物未写入 outputs：验收门/后续步不应拿到这次无进展结论。
        let steps = store::list_steps(&c, &run_id).unwrap();
        assert!(steps.iter().any(|s| s.status == "wait_human"), "当前步应待人工");
    }

    #[test]
    fn loop_stops_at_human_gate_with_custom_threshold_ignored() {
        // 两步 agent_step 产出逐字相同（规范化后）→ 第二步触发原地打转门。
        // provider/mock 标注会被加入产物，两次相同 provider 不影响比对。
        let c = db::open_in_memory().unwrap();
        install_agent_sop(&c,
            json!([
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"agent_step","config":{"goal":"第一步"}},
                {"id":"n2","type":"agent_step","config":{"goal":"第二步"}},
                {"id":"n3","type":"end","config":{}}
            ]),
            json!([{"from":"n0","to":"n1"},{"from":"n1","to":"n2"},{"from":"n2","to":"n3"}]));
        let llm = FixedJsonProvider { answer: r#"{"text":"永远同一句"}"#.into() };
        let run_id = start_run(&c, "sop-breaker", "build", "manual", &json!({}), &DemoTools, &llm).unwrap();
        assert_eq!(store::get_run(&c, &run_id).unwrap().status, "wait_human");
        let gates = store::list_gates(&c, true).unwrap();
        assert!(gates.iter().any(|g| g.reason == "loop"), "应触发原地打转门");
    }

    #[test]
    fn normal_confidence_passes_without_gate() {
        let c = db::open_in_memory().unwrap();
        install_agent_sop(&c,
            json!([
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"agent_step","config":{"goal":"核对"}},
                {"id":"n2","type":"end","config":{}}
            ]),
            json!([{"from":"n0","to":"n1"},{"from":"n1","to":"n2"}]));
        let llm = FixedJsonProvider { answer: r#"{"confidence":0.9,"text":"确定"}"#.into() };
        let run_id = start_run(&c, "sop-breaker", "build", "manual", &json!({}), &DemoTools, &llm).unwrap();
        assert_eq!(store::get_run(&c, &run_id).unwrap().status, "done", "正常产物应直接完成");
        assert!(store::list_gates(&c, true).unwrap().is_empty(), "不应产生人工门");
    }

    #[test]
    fn circuit_breaker_override_resumes_run() {
        let c = db::open_in_memory().unwrap();
        install_agent_sop(&c,
            json!([
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"agent_step","config":{"goal":"核对"}},
                {"id":"n2","type":"end","config":{}}
            ]),
            json!([{"from":"n0","to":"n1"},{"from":"n1","to":"n2"}]));
        let llm = FixedJsonProvider { answer: r#"{"confidence":0.1,"text":"不确定"}"#.into() };
        let run_id = start_run(&c, "sop-breaker", "build", "manual", &json!({}), &DemoTools, &llm).unwrap();
        let gate = &store::list_gates(&c, true).unwrap()[0];
        assert_eq!(gate.reason, "low_confidence");
        // 人批准 → 续跑。因产物未入 outputs，重跑仍低置信度会再次停门；这里只验裁决链路通畅。
        store::decide_gate(&c, &gate.id, "approve", Some("人工确认采纳")).unwrap();
        let s = resume_after_gate(&c, &run_id, &DemoTools, &llm).unwrap();
        assert_eq!(s, "wait_human", "批准后重跑仍低置信度，再次停门待裁决");
    }

    // ---- 人工干预：暂停 / 恢复 / 接管 / 改路径（§3 v1、§5.2）----

    /// 建一条纯工具 SOP（无人工门），跑完拿到 run 后手动回退状态为 running 供干预测试。
    fn install_plain_tool_sop(c: &Connection) {
        demo_install(c).unwrap();
        let def = json!({
            "version": 1,
            "nodes": [
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"mcp_tool","config":{"tool":"demo:check_anomalies","args":{"anomalies":3}}},
                {"id":"n2","type":"end","config":{}}
            ],
            "edges": [{"from":"n0","to":"n1"},{"from":"n1","to":"n2"}]
        });
        let p = crate::models::SopUpsertPayload {
            id: Some("sop-control".into()), name: "干预测试".into(),
            trigger_json: "{}".into(), risk: "low".into(), owner_policy: "agent".into(),
            definition: def, expect_version: None,
        };
        store::upsert_sop(c, &p).unwrap();
    }

    fn running_run(c: &Connection) -> String {
        install_plain_tool_sop(c);
        let run_id = start_run(c, "sop-control", "build", "manual", &json!({}), &DemoTools, &MockProvider).unwrap();
        assert_eq!(store::get_run(c, &run_id).unwrap().status, "done", "基线应跑完");
        // 测试用：回退到 running 并停在 n1（End 节点会清空游标），保留产物，模拟"运行中暂停"。
        c.execute("UPDATE runs SET status='running', current_node='n1' WHERE id=?1", params![run_id]).unwrap();
        run_id
    }

    #[test]
    fn pause_keeps_cursor_and_goes_wait_human() {
        let c = db::open_in_memory().unwrap();
        let run_id = running_run(&c);
        let before = store::get_run(&c, &run_id).unwrap();
        let s = control(&c, &run_id, "pause", None, None, &DemoTools, &MockProvider).unwrap();
        assert_eq!(s, "wait_human");
        let after = store::get_run(&c, &run_id).unwrap();
        assert_eq!(after.status, "wait_human");
        assert_eq!(after.current_node, before.current_node, "暂停不改游标（保留上下文）");
        assert_eq!(after.outputs_json, before.outputs_json, "暂停不改产物");
        let gates = store::list_gates(&c, true).unwrap();
        assert!(gates.iter().any(|g| g.run_id == run_id && g.reason == "pause"));
    }

    #[test]
    fn resume_only_releases_pause_gate_and_finishes() {
        let c = db::open_in_memory().unwrap();
        let run_id = running_run(&c);
        control(&c, &run_id, "pause", None, None, &DemoTools, &MockProvider).unwrap();
        let s = control(&c, &run_id, "resume", None, None, &DemoTools, &MockProvider).unwrap();
        assert_eq!(s, "done", "恢复后从当前步续跑到完成");
        assert!(store::list_gates(&c, true).unwrap().iter().all(|g| g.run_id != run_id), "pause 门已裁决");
    }

    #[test]
    fn resume_refuses_to_bypass_real_human_gate() {
        let c = db::open_in_memory().unwrap();
        install_plain_tool_sop(&c);
        let run_id = start_run(&c, "sop-control", "build", "manual", &json!({}), &DemoTools, &MockProvider).unwrap();
        c.execute("UPDATE runs SET status='wait_human' WHERE id=?1", params![run_id]).unwrap();
        // 插一个真实人工门（非 pause）
        let sid = store::step_id_for_node(&c, &run_id, "n1").unwrap().unwrap();
        store::insert_gate(&c, &run_id, &sid, "approval", "高风险确认", None).unwrap();
        // resume 必须拒绝，不能绕过真实门
        let r = control(&c, &run_id, "resume", None, None, &DemoTools, &MockProvider);
        assert!(r.is_err(), "存在真实人工门时 resume 必须拒绝");
        assert!(r.unwrap_err().to_string().contains("真实人工门"));
    }

    #[test]
    fn takeover_marks_step_human_and_logs() {
        let c = db::open_in_memory().unwrap();
        let run_id = running_run(&c);
        let s = control(&c, &run_id, "takeover", None, Some("我来接手"), &DemoTools, &MockProvider).unwrap();
        assert_eq!(s, "wait_human");
        let sid = store::step_id_for_node(&c, &run_id, "n1").unwrap().unwrap();
        let owner: String = c.query_row("SELECT owner FROM steps WHERE id=?1", params![sid], |r| r.get(0)).unwrap();
        assert_eq!(owner, "human", "接管后当前步 owner=human");
        let gates = store::list_gates(&c, true).unwrap();
        assert!(gates.iter().any(|g| g.run_id == run_id && g.reason == "takeover"));
        let human_logs: i64 = c.query_row(
            "SELECT COUNT(*) FROM step_logs WHERE step_id=?1 AND actor='human'", params![sid], |r| r.get(0),
        ).unwrap();
        assert!(human_logs >= 1, "接管须写 actor=human 日志");
    }

    #[test]
    fn change_path_validates_target_and_keeps_context() {
        let c = db::open_in_memory().unwrap();
        let run_id = running_run(&c);
        // 非法目标节点被拒
        assert!(control(&c, &run_id, "change_path", Some("n99"), None, &DemoTools, &MockProvider).is_err());
        // 合法目标：改游标到 n2（end），outputs 保留
        let before = store::get_run(&c, &run_id).unwrap();
        let s = control(&c, &run_id, "change_path", Some("n2"), Some("跳过工具"), &DemoTools, &MockProvider).unwrap();
        assert_eq!(s, "done", "改路径到 end 后完成");
        let after = store::get_run(&c, &run_id).unwrap();
        // 比较解析后的 JSON 值而非字符串：HashMap 序列化时 key 顺序不保证稳定。
        let before_val: serde_json::Value = serde_json::from_str(&before.outputs_json).unwrap();
        let after_val: serde_json::Value = serde_json::from_str(&after.outputs_json).unwrap();
        assert_eq!(after_val, before_val, "改路径不改产物（上下文保留）");
    }
}
