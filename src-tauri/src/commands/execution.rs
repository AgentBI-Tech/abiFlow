use std::path::PathBuf;

use serde_json::json;
use tauri::Manager;

use crate::AppEventSource;
use crate::AppKeyStore;
use crate::AppLlm;
use crate::domain::executor::{self};
use crate::domain::examples;
use crate::domain::experience;
use crate::domain::template_reuse;
use crate::domain::gateway::Mode;
use crate::domain::scheduler;
use crate::domain::sop::SopDefinition;
use crate::infra::{app_config, db, store, template_zip, tool_engine::ToolEngine};
use crate::models::{
    now_secs, AppError, AppResult, DashboardData, GateDecidePayload, GateRow, RunArbitratePayload,
    RunControlPayload, RunDetail, RunRow, RunStartPayload, RunStartResult, ScheduleSetPayload, SchedulerTickResult,
    SopRow, SopUpsertPayload, SopUpsertResult, ToolGrantRow, ToolGrantSetPayload, ToolRow,
    CredentialSetPayload, CredentialRef, CorrectionInput, CorrectionRow, EfficiencyReport,
    TemplateBundle, TemplateCreatePayload, TemplateRow, TemplateReusePayload, UpgradeApprovePayload, UpgradeCandidate,
    OwnerDecisionRow, ToolDefPayload, SopDetail, SopVersion, ConfigSetPayload,
    McpServerPayload, McpServerRow,
};

pub(crate) fn db_path(app: &tauri::AppHandle) -> AppResult<PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(dir.join("abiflow").join("abi.db"))
}

fn open_db(app: &tauri::AppHandle) -> AppResult<rusqlite::Connection> {
    db::open(&db_path(app)?)
}

// ---- SOP ----

#[tauri::command]
pub fn sop_list(app: tauri::AppHandle, enabled_only: Option<bool>) -> AppResult<Vec<SopRow>> {
    let conn = open_db(&app)?;
    store::list_sops(&conn, enabled_only.unwrap_or(false))
}

#[tauri::command]
pub fn sop_upsert(
    app: tauri::AppHandle,
    payload: SopUpsertPayload,
) -> AppResult<SopUpsertResult> {
    let conn = open_db(&app)?;
    // 校验 AST：结构合法 + 工具已注册（§4）。
    let def: SopDefinition = serde_json::from_value(payload.definition.clone())
        .map_err(|e| AppError::InvalidInput(format!("definition 非法：{}", e)))?;
    let known = store::known_tools(&conn)?;
    def.validate(&known)?;
    let (id, version) = store::upsert_sop(&conn, &payload)?;
    Ok(SopUpsertResult { id, version })
}

/// 取 SOP 定义 + 版本历史（历史来自 run 快照；sops 表原地升版本无历史行）。
#[tauri::command]
pub fn sop_get(app: tauri::AppHandle, id: String) -> AppResult<SopDetail> {
    let conn = open_db(&app)?;
    let sop = store::get_sop(&conn, &id)?;
    let versions = store::sop_versions(&conn, &id)?
        .into_iter()
        .map(|(v, def_json, ran_at)| {
            let definition = serde_json::from_str(&def_json).unwrap_or(serde_json::Value::Null);
            SopVersion { version: v, definition, ran_at }
        })
        .collect();
    Ok(SopDetail { sop, versions })
}

/// 应用配置（非敏感）；不返回密钥值（密钥只进密钥链）。
#[tauri::command]
pub fn config_get() -> AppResult<std::collections::BTreeMap<String, String>> {
    app_config::config_get_all()
}

#[tauri::command]
pub fn config_set(payload: ConfigSetPayload) -> AppResult<()> {
    app_config::config_set(&payload.key, &payload.value)
}

fn mcp_row(s: app_config::McpServer) -> McpServerRow {
    McpServerRow { id: s.id, command: s.command, args: s.args, note: s.note, transport: s.transport, url: s.url, env: s.env }
}

/// MCP server 配置（为 MCP client 协议栈预备；stdio 子进程可透传 env 变量名，值来自 secrets.env / shell）。
#[tauri::command]
pub fn mcp_server_list() -> AppResult<Vec<McpServerRow>> {
    Ok(app_config::mcp_list()?.into_iter().map(mcp_row).collect())
}

#[tauri::command]
pub fn mcp_server_add(payload: McpServerPayload) -> AppResult<Vec<McpServerRow>> {
    let server = app_config::McpServer {
        id: payload.id, command: payload.command, args: payload.args, note: payload.note,
        transport: payload.transport, url: payload.url, env: payload.env,
    };
    Ok(app_config::mcp_add(server)?.into_iter().map(mcp_row).collect())
}

#[tauri::command]
pub fn mcp_server_remove(id: String) -> AppResult<Vec<McpServerRow>> {
    Ok(app_config::mcp_remove(&id)?.into_iter().map(mcp_row).collect())
}

/// secrets.env 路径（仅路径、不含值），供 UI 提示用户在哪填 token。
#[tauri::command]
pub fn secrets_env_path() -> String {
    app_config::secrets_env_path_display()
}

#[tauri::command]
pub fn sop_set_enabled(app: tauri::AppHandle, id: String, enabled: bool) -> AppResult<()> {
    let conn = open_db(&app)?;
    store::set_sop_enabled(&conn, &id, enabled)
}

#[tauri::command]
pub fn schedule_set(app: tauri::AppHandle, payload: ScheduleSetPayload) -> AppResult<()> {
    let conn = open_db(&app)?;
    if !payload.enabled {
        return store::remove_schedule(&conn, &payload.sop_id);
    }
    if let Some(expr) = payload.cron_expr.as_deref() {
        if expr.trim().is_empty() {
            return Err(AppError::InvalidInput("cron 表达式不得为空".into()));
        }
        // 先校验表达式合法性，再算首次触发时间，避免非法表达式落库。
        let cron = crate::domain::cron::CronExpr::parse(expr)?;
        let next = cron.next_after(now_secs())?;
        store::upsert_cron_schedule(&conn, &payload.sop_id, cron.raw.as_str(), next)
    } else {
        let secs = payload.interval_secs.unwrap_or(3600).max(1);
        store::upsert_schedule(&conn, &payload.sop_id, secs)
    }
}

// ---- Run ----

#[tauri::command]
pub fn run_start(
    app: tauri::AppHandle,
    ks: tauri::State<'_, AppKeyStore>,
    llmst: tauri::State<'_, AppLlm>,
    payload: RunStartPayload,
) -> AppResult<RunStartResult> {
    let conn = open_db(&app)?;
    let engine = ToolEngine::new(ks.0.clone());
    let mode = payload.mode.as_deref().unwrap_or("build");
    let run_id = executor::start_run(
        &conn,
        &payload.sop_id,
        mode,
        "manual",
        payload.input.as_ref().unwrap_or(&json!({})),
        &engine,
        llmst.0.as_ref(),
    )?;
    Ok(RunStartResult { run_id })
}

#[tauri::command]
pub fn run_get(app: tauri::AppHandle, run_id: String) -> AppResult<RunDetail> {
    let conn = open_db(&app)?;
    store::run_detail(&conn, &run_id)
}

#[tauri::command]
pub fn run_list(
    app: tauri::AppHandle,
    sop_id: Option<String>,
    status: Option<String>,
    limit: Option<i64>,
) -> AppResult<Vec<RunRow>> {
    let conn = open_db(&app)?;
    store::list_runs(
        &conn,
        sop_id.as_deref(),
        status.as_deref(),
        limit.unwrap_or(30).max(1),
    )
}

#[tauri::command]
pub fn run_control(
    app: tauri::AppHandle,
    ks: tauri::State<'_, AppKeyStore>,
    llmst: tauri::State<'_, AppLlm>,
    payload: RunControlPayload,
) -> AppResult<String> {
    let conn = open_db(&app)?;
    let engine = ToolEngine::new(ks.0.clone());
    executor::control(
        &conn, &payload.run_id, &payload.action,
        payload.value.as_deref(), payload.note.as_deref(),
        &engine, llmst.0.as_ref(),
    )
}

/// 第 1 轮打回后重试（清空本轮验收留痕、游标回起点重跑；reject_count 不重置）。
#[tauri::command]
pub fn run_retry(
    app: tauri::AppHandle,
    ks: tauri::State<'_, AppKeyStore>,
    llmst: tauri::State<'_, AppLlm>,
    payload: RunControlPayload,
) -> AppResult<String> {
    let conn = open_db(&app)?;
    let engine = ToolEngine::new(ks.0.clone());
    executor::retry_rejected(&conn, &payload.run_id, &engine, llmst.0.as_ref())
}

/// 人工仲裁（review 状态）：resume 放行重跑 / fail 判失败。
#[tauri::command]
pub fn run_arbitrate(
    app: tauri::AppHandle,
    ks: tauri::State<'_, AppKeyStore>,
    llmst: tauri::State<'_, AppLlm>,
    payload: RunArbitratePayload,
) -> AppResult<String> {
    let conn = open_db(&app)?;
    let engine = ToolEngine::new(ks.0.clone());
    executor::arbitrate(
        &conn, &payload.run_id, &payload.decision, payload.note.as_deref(),
        &engine, llmst.0.as_ref(),
    )
}

// ---- Gate ----

#[tauri::command]
pub fn gate_list(app: tauri::AppHandle, pending_only: Option<bool>) -> AppResult<Vec<GateRow>> {
    let conn = open_db(&app)?;
    store::list_gates(&conn, pending_only.unwrap_or(true))
}

#[tauri::command]
pub fn gate_decide(
    app: tauri::AppHandle,
    ks: tauri::State<'_, AppKeyStore>,
    llmst: tauri::State<'_, AppLlm>,
    payload: GateDecidePayload,
) -> AppResult<()> {
    let conn = open_db(&app)?;
    let engine = ToolEngine::new(ks.0.clone());
    let gate = store::get_gate(&conn, &payload.gate_id)?;
    if gate.decided_at.is_some() {
        return Err(AppError::InvalidInput("人工门已裁决".into()));
    }
    let decision = payload.decision.as_str();
    if !matches!(decision, "approve" | "reject" | "input" | "takeover" | "cancel") {
        return Err(AppError::InvalidInput(format!("非法裁决：{}", decision)));
    }
    store::decide_gate(&conn, &payload.gate_id, decision, payload.note.as_deref())?;

    match decision {
        "reject" | "cancel" => {
            let verdict = if decision == "reject" { "fail" } else { "partial" };
            store::update_run_status(&conn, &gate.run_id, "failed", Some(verdict), Some("人工驳回/取消"))?;
        }
        _ => {
            executor::resume_after_gate(&conn, &gate.run_id, &engine, llmst.0.as_ref())?;
        }
    }
    Ok(())
}

// ---- Dashboard / Scheduler / Demo ----

#[tauri::command]
pub fn dashboard_get(app: tauri::AppHandle) -> AppResult<DashboardData> {
    let conn = open_db(&app)?;
    store::dashboard(&conn)
}

#[tauri::command]
pub fn scheduler_tick(app: tauri::AppHandle, ks: tauri::State<'_, AppKeyStore>, llmst: tauri::State<'_, AppLlm>, ev: tauri::State<'_, AppEventSource>) -> AppResult<SchedulerTickResult> {
    let conn = open_db(&app)?;
    let engine = ToolEngine::new(ks.0.clone());
    scheduler::tick(&conn, ev.0.as_ref(), &engine, llmst.0.as_ref())
}

/// 注入一个假事件到进程内事件源（事件订阅触发的测试/演示入口）。
/// 真事件源（MCP/webhook）以后替换实现即可，命令层不变。
#[tauri::command]
pub fn emit_event(ev: tauri::State<'_, AppEventSource>, event_type: String, payload_json: Option<String>) -> AppResult<()> {
    let payload = match payload_json {
        Some(s) if !s.trim().is_empty() => serde_json::from_str(&s)
            .map_err(|e| AppError::InvalidInput(format!("payload_json 非法：{}", e)))?,
        _ => json!({}),
    };
    ev.0.emit(&event_type, payload);
    Ok(())
}

#[tauri::command]
pub fn demo_install(app: tauri::AppHandle) -> AppResult<String> {
    let conn = open_db(&app)?;
    executor::demo_install(&conn)
}

// ---- v0.3 工具与授权 ----

/// 工具清单：按 role + mode 过滤可见性（内容角色看不到 cost 面工具）。
#[tauri::command]
pub fn tool_list(
    app: tauri::AppHandle,
    role: Option<String>,
    mode: Option<String>,
) -> AppResult<Vec<ToolRow>> {
    let conn = open_db(&app)?;
    let role = role.unwrap_or_else(|| "agent".into());
    let mode = Mode::parse(mode.as_deref().unwrap_or("build"))?;
    let specs = store::list_tool_specs(&conn)?;
    let mut out = Vec::with_capacity(specs.len());
    for s in specs {
        let visible = store::tool_visible(&conn, &s.id, &role, mode)?;
        let (title, server) = store::tool_meta(&conn, &s.id)?;
        out.push(ToolRow {
            id: s.id.clone(),
            title,
            server,
            risk: s.risk,
            plane: s.plane,
            requires_approval: s.requires_approval,
            irreversible: s.irreversible,
            visible,
        });
    }
    Ok(out)
}

/// 列出全部授权（管理视图，不受可见性过滤）。
#[tauri::command]
pub fn tool_grant_list(app: tauri::AppHandle) -> AppResult<Vec<ToolGrantRow>> {
    let conn = open_db(&app)?;
    let mut stmt = conn
        .prepare("SELECT tool_id,mode,allowed,planes_json FROM tool_grants ORDER BY tool_id,mode")
        .map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            let planes: String = r.get(3)?;
            Ok(ToolGrantRow {
                tool_id: r.get(0)?,
                mode: r.get(1)?,
                allowed: r.get::<_, i64>(2)? != 0,
                planes: serde_json::from_str(&planes).unwrap_or_default(),
            })
        })
        .map_err(|e| AppError::Db(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))
}

/// 改授权（需管理员模式；框架阶段不鉴权调用者，仅提供入口，UI 层标管理员）。
#[tauri::command]
pub fn tool_grant_set(app: tauri::AppHandle, payload: ToolGrantSetPayload) -> AppResult<()> {
    let conn = open_db(&app)?;
    let mode = Mode::parse(&payload.mode)?;
    store::upsert_tool_grant(&conn, &payload.tool_id, mode, payload.allowed, &payload.planes)
}

// ---- v0.3 密钥引用（值只进 keystore，不落库不回显）----

/// 只列引用元数据（storage_key/provider/label），绝不返回值。
#[tauri::command]
pub fn credential_list(app: tauri::AppHandle) -> AppResult<Vec<CredentialRef>> {
    let conn = open_db(&app)?;
    store::list_credentials(&conn)
}

/// 写值走 keystore（内存传值），DB 只存引用元数据；value 不进库、不进日志、不回显。
#[tauri::command]
pub fn credential_set(
    app: tauri::AppHandle,
    ks: tauri::State<'_, AppKeyStore>,
    payload: CredentialSetPayload,
) -> AppResult<()> {
    if payload.ref_id.is_empty() || payload.value.is_empty() {
        return Err(AppError::InvalidInput("ref_id 与 value 不得为空".into()));
    }
    ks.0.set(&payload.ref_id, &payload.value)?; // 值只进 keystore
    let conn = open_db(&app)?;
    store::upsert_credential_ref(
        &conn,
        &payload.ref_id,
        &payload.provider,
        payload.label.as_deref(),
    )
}

/// 删除：同时清 keystore 值与库内引用元数据。
#[tauri::command]
pub fn credential_remove(
    app: tauri::AppHandle,
    ks: tauri::State<'_, AppKeyStore>,
    ref_id: String,
) -> AppResult<()> {
    ks.0.delete(&ref_id)?;
    let conn = open_db(&app)?;
    store::remove_credential_ref(&conn, &ref_id)
}

// ---- v0.4 经验沉淀 ----

/// 从一条 run 捕获经验模板（仅在实测通过时 validated=1）。
#[tauri::command]
pub fn template_create(
    app: tauri::AppHandle,
    payload: TemplateCreatePayload,
) -> AppResult<String> {
    let conn = open_db(&app)?;
    let detail = store::run_detail(&conn, &payload.run_id)?;
    let corrections = store::list_corrections(&conn, None)?;
    let acc: Vec<serde_json::Value> = detail
        .acceptance
        .iter()
        .map(|a| serde_json::json!({"criterion": a.criterion, "passed": a.passed}))
        .collect();
    let bundle = experience::capture_template(
        &detail.run.status,
        detail.run.verdict.as_deref(),
        &payload.name,
        detail.run.definition_snapshot.as_deref().unwrap_or("{}"),
        &detail.steps,
        &acc,
        &corrections,
        payload.source_shop.as_deref(),
    )?;
    store::save_template(&conn, &bundle)
}

#[tauri::command]
pub fn template_list(app: tauri::AppHandle) -> AppResult<Vec<TemplateRow>> {
    let conn = open_db(&app)?;
    store::list_templates(&conn)
}

/// 导出指定模板为 zip 字节（前端触发下载）。
#[tauri::command]
pub fn template_export(app: tauri::AppHandle, id: String) -> AppResult<Vec<u8>> {
    let conn = open_db(&app)?;
    let rows = store::list_templates(&conn)?;
    let t = rows
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| AppError::NotFound(format!("模板 {}", id)))?;
    let bundle = TemplateBundle {
        manifest_schema: experience::MANIFEST_SCHEMA,
        name: t.name,
        validated: t.validated,
        sop_snapshot: t.sop_snapshot,
        trajectory_json: t.trajectory_json,
        corrections_json: t.corrections_json,
        source_shop: t.source_shop,
    };
    template_zip::export_zip(&bundle)
}

/// 导入 zip：解包 + schema/validated 校验（未实测通过拒绝），通过后落库。
#[tauri::command]
pub fn template_import(app: tauri::AppHandle, bytes: Vec<u8>) -> AppResult<String> {
    let bundle = template_zip::import_zip(&bytes)?;
    experience::validate_import(&bundle)?;
    let conn = open_db(&app)?;
    store::save_template(&conn, &bundle)
}

/// 取模板定义里声明的可换参数 {{param:*}} 列表，供前端生成输入框。
#[tauri::command]
pub fn template_params(app: tauri::AppHandle, template_id: String) -> AppResult<Vec<String>> {
    let conn = open_db(&app)?;
    let t = store::list_templates(&conn)?
        .into_iter()
        .find(|t| t.id == template_id)
        .ok_or_else(|| AppError::NotFound(format!("模板 {}", template_id)))?;
    let def: serde_json::Value = serde_json::from_str(&t.sop_snapshot)
        .map_err(|e| AppError::InvalidInput(format!("模板定义非法：{}", e)))?;
    Ok(template_reuse::collect_params(&def))
}

/// 换参复用：从 validated 模板 + 参数生成一条新 SOP（换店铺只换参数，不重编流程）。
/// 未实测通过的模板不得复用；资金/不可逆动作不因换参自动放行（owner_policy 继承模板，不提权）。
#[tauri::command]
pub fn template_reuse(
    app: tauri::AppHandle,
    payload: TemplateReusePayload,
) -> AppResult<SopUpsertResult> {
    if payload.name.trim().is_empty() {
        return Err(AppError::InvalidInput("新 SOP 名称不得为空".into()));
    }
    let conn = open_db(&app)?;
    let t = store::list_templates(&conn)?
        .into_iter()
        .find(|t| t.id == payload.template_id)
        .ok_or_else(|| AppError::NotFound(format!("模板 {}", payload.template_id)))?;
    if !t.validated {
        return Err(AppError::InvalidInput("模板未实测通过（validated=0），不得复用上岗".into()));
    }
    let def: serde_json::Value = serde_json::from_str(&t.sop_snapshot)
        .map_err(|e| AppError::InvalidInput(format!("模板定义非法：{}", e)))?;
    // 替换 {{param:*}}；缺参/未闭合由 apply_params 报错。
    let params_map: serde_json::Map<String, serde_json::Value> = payload.params.into_iter().collect();
    let new_def = template_reuse::apply_params(&def, &params_map)?;
    let upsert = SopUpsertPayload {
        id: None, // 生成新 id，不动原模板/原 SOP
        name: payload.name,
        trigger_json: "{}".into(),
        risk: "low".into(),
        owner_policy: "hybrid".into(),
        definition: new_def,
        expect_version: None,
    };
    let (id, version) = store::upsert_sop(&conn, &upsert)?;
    Ok(SopUpsertResult { id, version })
}

#[tauri::command]
pub fn correction_add(app: tauri::AppHandle, input: CorrectionInput) -> AppResult<i64> {
    let conn = open_db(&app)?;
    store::add_correction(&conn, &input.step_id, &input)
}

#[tauri::command]
pub fn correction_list(app: tauri::AppHandle, step_id: Option<String>) -> AppResult<Vec<CorrectionRow>> {
    let conn = open_db(&app)?;
    store::list_corrections(&conn, step_id.as_deref())
}

/// owner 升级候选（不自动放权；资金/不可逆恒不可升级）。
#[tauri::command]
pub fn upgrade_candidate_list(app: tauri::AppHandle) -> AppResult<Vec<UpgradeCandidate>> {
    let conn = open_db(&app)?;
    let steps = store::all_steps(&conn)?;
    let corrections = store::list_corrections(&conn, None)?;
    let unsafe_tools: Vec<String> = store::list_tool_specs(&conn)?
        .into_iter()
        .filter(|s| s.irreversible || s.risk == "high" || s.requires_approval)
        .map(|s| s.id)
        .collect();
    let sops = store::run_sop_map(&conn)?;
    Ok(experience::upgrade_candidates(
        &steps,
        &corrections,
        &unsafe_tools,
        &sops,
        experience::UPGRADE_THRESHOLD,
    ))
}

/// 人确认/驳回升级候选：确认时真正把 SOP 节点 config.owner 从 human 改为 agent（版本递增），
/// 并留审计；驳回只留审计。资金/不可逆动作在候选计算与 plan 两次硬拦，永不放权。
#[tauri::command]
pub fn upgrade_approve(app: tauri::AppHandle, payload: UpgradeApprovePayload) -> AppResult<()> {
    let conn = open_db(&app)?;
    // 重新计算候选，防止绕过 UI 伪造一个不存在的候选。
    let steps = store::all_steps(&conn)?;
    let corrections = store::list_corrections(&conn, None)?;
    let unsafe_tools: Vec<String> = store::list_tool_specs(&conn)?
        .into_iter()
        .filter(|s| s.irreversible || s.risk == "high" || s.requires_approval)
        .map(|s| s.id)
        .collect();
    let sops = store::run_sop_map(&conn)?;
    let cand = experience::upgrade_candidates(
        &steps, &corrections, &unsafe_tools, &sops, experience::UPGRADE_THRESHOLD,
    )
    .into_iter()
    .find(|c| c.sop_id == payload.sop_id && c.node_id == payload.node_id)
    .ok_or_else(|| AppError::InvalidInput("该节点当前不是有效升级候选".into()))?;
    if !cand.eligible {
        return Err(AppError::RiskBlocked("资金/不可逆动作永不升级为 agent".into()));
    }

    if !payload.approve {
        store::insert_owner_decision(
            &conn, &payload.sop_id, &payload.node_id, &cand.node_type,
            "human", "human", "reject", cand.correction_count as i64, None,
        )?;
        return Ok(());
    }

    // 确认放权：plan（纯校验）产出新定义 → 落库并递增版本 → 留审计。
    let sop = store::get_sop(&conn, &payload.sop_id)?;
    let new_def = experience::plan_owner_upgrade(&sop.definition, &payload.node_id, &unsafe_tools)?;
    store::set_node_owner(&conn, &payload.sop_id, &new_def)?;
    store::insert_owner_decision(
        &conn, &payload.sop_id, &payload.node_id, &cand.node_type,
        "human", "agent", "approve", cand.correction_count as i64, None,
    )?;
    Ok(())
}

/// owner 决策审计列表（某 SOP 或全部）。
#[tauri::command]
pub fn owner_decision_list(app: tauri::AppHandle, sop_id: Option<String>) -> AppResult<Vec<OwnerDecisionRow>> {
    let conn = open_db(&app)?;
    store::list_owner_decisions(&conn, sop_id.as_deref())
}

#[tauri::command]
pub fn report_get(app: tauri::AppHandle) -> AppResult<EfficiencyReport> {
    let conn = open_db(&app)?;
    let runs_total: i64 = conn
        .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
        .map_err(|e| AppError::Db(e.to_string()))?;
    let completed = store::count_completed_runs(&conn)?;
    let steps = store::all_steps(&conn)?;
    let interventions: i64 = conn
        .query_row("SELECT COUNT(*) FROM human_gates WHERE decision IS NOT NULL", [], |r| r.get(0))
        .map_err(|e| AppError::Db(e.to_string()))?;
    let checks = store::count_acceptance_checks(&conn)?;
    Ok(experience::efficiency_report(
        runs_total as usize,
        completed,
        &steps,
        interventions as usize,
        checks,
    ))
}

// ---- v0.5 自定义工具（工具即数据）----

/// 新增/更新一个自定义 HTTP 工具；scope 决定行为，改配置即改工具，无需动核心。
#[tauri::command]
pub fn tool_upsert(app: tauri::AppHandle, payload: ToolDefPayload) -> AppResult<()> {
    if payload.id.is_empty() || !matches!(payload.scope.get("kind").and_then(|v| v.as_str()), Some("http") | Some("builtin")) {
        return Err(AppError::InvalidInput("工具 id 不得为空，kind 需为 http/builtin".into()));
    }
    let conn = open_db(&app)?;
    let scope = serde_json::to_string(&payload.scope)
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;
    store::upsert_tool(
        &conn, &payload.id, &payload.server, &payload.command, &payload.title,
        &payload.risk, &scope, payload.enabled,
    )
}

#[tauri::command]
pub fn tool_remove(app: tauri::AppHandle, id: String) -> AppResult<()> {
    let conn = open_db(&app)?;
    store::remove_tool(&conn, &id)
}

/// 安装 agentbi.tech 示例工具组（用户可改可删），用于「第一个使用者」的每日维护。
#[tauri::command]
pub fn example_tools_install(app: tauri::AppHandle) -> AppResult<Vec<String>> {
    let conn = open_db(&app)?;
    examples::install_agentbi_examples(&conn)
}

/// 幂等安装 Shopify 只读工具组 + 两条 SOP（不建调度，不自动对真店跑）。
#[tauri::command]
pub fn shopify_tools_install(app: tauri::AppHandle) -> AppResult<Vec<String>> {
    let conn = open_db(&app)?;
    examples::install_shopify_readonly(&conn)
}

/// 安装放权演示 SOP（human-owned agent_step，本地 Mock，无外部依赖）。
#[tauri::command]
pub fn delegation_demo_install(app: tauri::AppHandle) -> AppResult<String> {
    let conn = open_db(&app)?;
    examples::install_delegation_demo(&conn)
}

/// 安装「每日维护 agentbi.tech」示例 SOP（可改可删，作为模板导出）。
#[tauri::command]
pub fn maintenance_sop_install(app: tauri::AppHandle) -> AppResult<String> {
    let conn = open_db(&app)?;
    examples::install_maintenance_sop(&conn)
}

/// 以 agent/scheduler 身份立即触发一次（与手动运行区分 trigger_by）。
#[tauri::command]
pub fn run_as_agent(
    app: tauri::AppHandle,
    ks: tauri::State<'_, AppKeyStore>,
    llmst: tauri::State<'_, AppLlm>,
    sop_id: String,
) -> AppResult<RunStartResult> {
    let conn = open_db(&app)?;
    let engine = ToolEngine::new(ks.0.clone());
    let run_id = executor::start_run(&conn, &sop_id, "build", "scheduler", &json!({}), &engine, llmst.0.as_ref())?;
    Ok(RunStartResult { run_id })
}

/// 自然语言 → SOP AST 草稿（只返回，不落库）。人工确认后再调 sop_upsert。
#[tauri::command]
pub fn sop_parse_text(llmst: tauri::State<'_, AppLlm>, text: String) -> AppResult<serde_json::Value> {
    executor::parse_sop_text(&text, llmst.0.as_ref())
}

/// 当前 LLM provider 状态（名称 / 是否 mock），供 UI 如实展示。
#[tauri::command]
pub fn llm_status(llmst: tauri::State<'_, AppLlm>) -> serde_json::Value {
    serde_json::json!({
        "name": llmst.0.name(),
        "mock": llmst.0.is_mock(),
    })
}

// ---- 用户管理与权限（2026-11-24）----

#[tauri::command]
pub fn user_list(app: tauri::AppHandle) -> AppResult<Vec<crate::domain::user::UserRow>> {
    let conn = open_db(&app)?;
    crate::domain::user::list_users(&conn)
}

#[tauri::command]
pub fn user_current() -> Option<crate::domain::user::CurrentUser> {
    crate::domain::user::current_user()
}

#[tauri::command]
pub fn user_add(
    app: tauri::AppHandle,
    username: String,
    display_name: String,
    role: String,
    password: String,
) -> AppResult<crate::domain::user::UserRow> {
    let conn = open_db(&app)?;
    crate::domain::user::create_user(&conn, &username, &display_name, &role, &password)
}

#[tauri::command]
pub fn user_login(
    app: tauri::AppHandle,
    username: String,
    password: String,
) -> AppResult<crate::domain::user::CurrentUser> {
    let conn = open_db(&app)?;
    crate::domain::user::login(&conn, &username, &password)
}

#[tauri::command]
pub fn user_logout() -> AppResult<()> {
    crate::domain::user::clear_current_user()
}

#[tauri::command]
pub fn user_set_role(app: tauri::AppHandle, user_id: String, role: String) -> AppResult<crate::domain::user::UserRow> {
    let conn = open_db(&app)?;
    crate::domain::user::set_role(&conn, &user_id, &role)
}

#[tauri::command]
pub fn user_remove(app: tauri::AppHandle, user_id: String) -> AppResult<()> {
    let conn = open_db(&app)?;
    crate::domain::user::delete_user(&conn, &user_id)
}

#[tauri::command]
pub fn user_update(
    app: tauri::AppHandle,
    user_id: String,
    display_name: String,
    role: String,
) -> AppResult<crate::domain::user::UserRow> {
    let conn = open_db(&app)?;
    crate::domain::user::update_user(&conn, &user_id, &display_name, &role)
}

#[tauri::command]
pub fn user_reset_password(app: tauri::AppHandle, user_id: String, new_password: String) -> AppResult<()> {
    let conn = open_db(&app)?;
    crate::domain::user::reset_password(&conn, &user_id, &new_password)
}
