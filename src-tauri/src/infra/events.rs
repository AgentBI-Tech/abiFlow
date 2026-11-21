//! 后端 → 前端事件推送（实现设计 §3.1）。
//!
//! 设计口径：事件只是「UI 该刷新了」的提示，不是真相源；状态一律以 run_get / dashboardGet 拉取为准。
//! 这里进程级持有 Tauri AppHandle（setup 时初始化），store 层状态变更后调用 emit。
//! 未初始化时（cargo test 等无窗口环境）emit 自动 no-op，不影响单测。

use std::sync::OnceLock;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// setup 时初始化一次；重复调用忽略。
pub fn init(handle: AppHandle) {
    let _ = APP_HANDLE.set(handle);
}

fn handle() -> Option<&'static AppHandle> {
    APP_HANDLE.get()
}

/// 发一个事件到前端；无窗口环境静默跳过。
pub fn emit<S: Serialize + Clone>(event: &str, payload: S) {
    if let Some(h) = handle() {
        let _ = h.emit(event, payload);
    }
}

// ---- 事件名（与前端订阅保持一致）----
pub const RUN_STATUS: &str = "run:status";
pub const STEP_UPDATE: &str = "step:update";
pub const GATE_REQUESTED: &str = "gate:requested";
pub const REVIEW_RESULT: &str = "review:result";
pub const TOOL_AUDIT: &str = "tool:audit";

// ---- payload 结构 ----
#[allow(non_snake_case)]
#[derive(Debug, Clone, Serialize)]
pub struct RunStatus {
    pub runId: String,
    pub status: String,
}
#[allow(non_snake_case)]
#[derive(Debug, Clone, Serialize)]
pub struct StepUpdate {
    pub runId: String,
    pub stepId: String,
    pub status: String,
}
#[allow(non_snake_case)]
#[derive(Debug, Clone, Serialize)]
pub struct GateRequested {
    pub gateId: String,
    pub runId: String,
    pub reason: String,
}
#[allow(non_snake_case)]
#[derive(Debug, Clone, Serialize)]
pub struct ReviewResult {
    pub runId: String,
    pub passed: bool,
}
#[allow(non_snake_case)]
#[derive(Debug, Clone, Serialize)]
pub struct ToolAudit {
    pub runId: String,
    pub stepId: String,
    pub tool: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_is_noop_without_app_handle() {
        // 单测环境无窗口，init 未调用 → emit 不应 panic。
        emit(RUN_STATUS, RunStatus { runId: "x".into(), status: "running".into() });
    }
}
