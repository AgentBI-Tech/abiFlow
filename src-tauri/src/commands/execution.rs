use std::path::PathBuf;

use serde_json::json;
use tauri::Manager;

use crate::domain::executor::{self, DemoTools};
use crate::domain::scheduler;
use crate::domain::sop::SopDefinition;
use crate::infra::{db, store};
use crate::models::{
    AppError, AppResult, DashboardData, GateDecidePayload, GateRow, RunControlPayload,
    RunDetail, RunRow, RunStartPayload, RunStartResult, ScheduleSetPayload, SchedulerTickResult,
    SopRow, SopUpsertPayload, SopUpsertResult,
};

pub(crate) fn db_path(app: &tauri::AppHandle) -> AppResult<PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(dir.join("abi-pilot").join("abi.db"))
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

#[tauri::command]
pub fn sop_set_enabled(app: tauri::AppHandle, id: String, enabled: bool) -> AppResult<()> {
    let conn = open_db(&app)?;
    store::set_sop_enabled(&conn, &id, enabled)
}

#[tauri::command]
pub fn schedule_set(app: tauri::AppHandle, payload: ScheduleSetPayload) -> AppResult<()> {
    let conn = open_db(&app)?;
    if payload.enabled {
        let secs = payload.interval_secs.unwrap_or(3600).max(1);
        store::upsert_schedule(&conn, &payload.sop_id, secs)
    } else {
        store::remove_schedule(&conn, &payload.sop_id)
    }
}

// ---- Run ----

#[tauri::command]
pub fn run_start(
    app: tauri::AppHandle,
    payload: RunStartPayload,
) -> AppResult<RunStartResult> {
    let conn = open_db(&app)?;
    let mode = payload.mode.as_deref().unwrap_or("build");
    let run_id = executor::start_run(
        &conn,
        &payload.sop_id,
        mode,
        "manual",
        payload.input.as_ref().unwrap_or(&json!({})),
        &DemoTools,
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
    payload: RunControlPayload,
) -> AppResult<String> {
    let conn = open_db(&app)?;
    executor::control(&conn, &payload.run_id, &payload.action)
}

// ---- Gate ----

#[tauri::command]
pub fn gate_list(app: tauri::AppHandle, pending_only: Option<bool>) -> AppResult<Vec<GateRow>> {
    let conn = open_db(&app)?;
    store::list_gates(&conn, pending_only.unwrap_or(true))
}

#[tauri::command]
pub fn gate_decide(app: tauri::AppHandle, payload: GateDecidePayload) -> AppResult<()> {
    let conn = open_db(&app)?;
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
            executor::resume_after_gate(&conn, &gate.run_id, &DemoTools)?;
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
pub fn scheduler_tick(app: tauri::AppHandle) -> AppResult<SchedulerTickResult> {
    let conn = open_db(&app)?;
    scheduler::tick(&conn, &DemoTools)
}

#[tauri::command]
pub fn demo_install(app: tauri::AppHandle) -> AppResult<String> {
    let conn = open_db(&app)?;
    executor::demo_install(&conn)
}
