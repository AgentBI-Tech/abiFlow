use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::domain::audit::redact_value;
use crate::domain::gateway::{self, Actor, Decision, Mode};
use crate::domain::sop::{eval_condition, NodeType, SopDefinition};
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

/// 启动一条 run：固定定义快照 + 建 step 行 + 进入 running 并推进。
pub fn start_run(
    conn: &Connection,
    sop_id: &str,
    mode: &str,
    trigger_by: &str,
    input: &Value,
    tools: &dyn ToolRunner,
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
    for (i, n) in def.nodes.iter().enumerate() {
        let owner = match n.node_type {
            NodeType::HumanGate | NodeType::Acceptance => "human",
            _ => "agent",
        };
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

    advance(conn, &run_id, tools)?;
    Ok(run_id)
}

/// 推进一条 run 直到：完成 / 失败 / 等人工。返回最终状态字符串。
pub fn advance(conn: &Connection, run_id: &str, tools: &dyn ToolRunner) -> AppResult<String> {
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
                let mode = Mode::parse(&run.mode)?;
                let ctx = store::call_ctx(conn, run_id, Actor::agent(), mode, tool)?;
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
                if let Some(id) = step_id {
                    let out = json!({"note": "v0.2 占位：Agent 推理在 v0.3 接入"});
                    let out_str = serde_json::to_string(&out).unwrap_or_default();
                    store::update_step(conn, &id, "done", None, Some(&out_str))?;
                }
                outputs.insert(current.clone(), json!({"note": "v0.2 agent placeholder"}));
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
                for c in &criteria {
                    let expr = c.as_str().unwrap_or("");
                    let passed = eval_condition(expr, &outputs).unwrap_or(false);
                    store::insert_acceptance(conn, run_id, expr, passed, "rule", None)?;
                }
                // 不变量 4：所有 acceptance 必须 passed，否则终态 partial。
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
pub fn resume_after_gate(conn: &Connection, run_id: &str, tools: &dyn ToolRunner) -> AppResult<String> {
    let run = store::get_run(conn, run_id)?;
    if run.status != "wait_human" {
        return Ok(run.status);
    }
    store::update_run_status(conn, run_id, "running", None, None)?;
    advance(conn, run_id, tools)
}

/// 暂停/恢复/取消/接管。v0.2 只真正实现 cancel 与 takeover 改状态，其余返回提示。
pub fn control(conn: &Connection, run_id: &str, action: &str) -> AppResult<String> {
    let run = store::get_run(conn, run_id)?;
    match action {
        "cancel" => {
            if matches!(run.status.as_str(), "done" | "failed" | "cancelled") {
                return Err(AppError::InvalidInput("终态 run 不可取消".into()));
            }
            store::update_run_status(conn, run_id, "cancelled", None, Some("用户取消"))?;
            Ok("cancelled".into())
        }
        "pause" | "resume" | "change_path" => Err(AppError::InvalidInput(format!(
            "{} 将在 v0.3+ 实现", action
        ))),
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

    #[test]
    fn run_stops_at_gate_and_cannot_advance() {
        let c = setup();
        let run_id = start_run(&c, "sop-demo-daily-anomaly", "build", "manual", &json!({}), &DemoTools).unwrap();
        let run = store::get_run(&c, &run_id).unwrap();
        assert_eq!(run.status, "wait_human", "应在人工门停住");
        // 再次推进不应越过 wait_human（不变量 1）。
        let s = advance(&c, &run_id, &DemoTools).unwrap();
        assert_eq!(s, "wait_human");
        let gates = store::list_gates(&c, true).unwrap();
        assert_eq!(gates.len(), 1);
    }

    #[test]
    fn approve_gate_resumes_to_done() {
        let c = setup();
        let run_id = start_run(&c, "sop-demo-daily-anomaly", "build", "manual", &json!({}), &DemoTools).unwrap();
        let gate = &store::list_gates(&c, true).unwrap()[0];
        store::decide_gate(&c, &gate.id, "approve", None).unwrap();
        let s = resume_after_gate(&c, &run_id, &DemoTools).unwrap();
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
        let run_id = start_run(&c, "sop-demo-daily-anomaly", "build", "manual", &json!({}), &DemoTools).unwrap();
        let gate = &store::list_gates(&c, true).unwrap()[0];
        store::decide_gate(&c, &gate.id, "reject", None).unwrap();
        let s = resume_after_gate(&c, &run_id, &DemoTools).unwrap();
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
        let run_id = start_run(&c, "sop-high", "build", "manual", &json!({}), &DemoTools).unwrap();
        assert_eq!(store::get_run(&c, &run_id).unwrap().status, "wait_human", "高风险必须先过人工门");
    }

    #[test]
    fn cancel_terminal_guard() {
        let c = setup();
        let run_id = start_run(&c, "sop-demo-daily-anomaly", "build", "manual", &json!({}), &DemoTools).unwrap();
        let s = control(&c, &run_id, "cancel").unwrap();
        assert_eq!(s, "cancelled");
        assert!(control(&c, &run_id, "cancel").is_err(), "终态不可再取消");
    }
}
