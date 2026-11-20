pub mod commands;
pub mod domain;
pub mod infra;
pub mod models;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;

use crate::domain::keystore::{KeyStore, MemoryKeyStore};
use crate::domain::llm::{self, LlmProvider};
use crate::domain::scheduler;
use crate::infra::db;
use crate::infra::tool_engine::ToolEngine;

struct SchedulerStop(Arc<AtomicBool>);
/// 密钥链实例（框架期为进程内存实现；接真实店铺时替换为 OS 密钥链，trait 不变）。
pub struct AppKeyStore(pub Arc<dyn KeyStore>);
/// LLM Provider（默认 Mock；本机未装 Ollama，装好后改配置即可切换，核心不动）。
pub struct AppLlm(pub Arc<dyn LlmProvider>);

/// 从环境变量读 LLM 配置：ABI_LLM_PROVIDER=ollama + ABI_LLM_HOST + ABI_LLM_MODEL。
fn build_llm() -> Arc<dyn LlmProvider> {
    let provider = std::env::var("ABI_LLM_PROVIDER").unwrap_or_default();
    let host = std::env::var("ABI_LLM_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    let model = std::env::var("ABI_LLM_MODEL").unwrap_or_else(|_| "qwen2.5:3b".into());
    llm::from_config(&provider, &host, &model)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let stop = Arc::new(AtomicBool::new(false));
            app.manage(SchedulerStop(stop.clone()));
            let ks: Arc<dyn KeyStore> = Arc::new(MemoryKeyStore::new());
            app.manage(AppKeyStore(ks.clone()));
            let llm: Arc<dyn LlmProvider> = build_llm();
            app.manage(AppLlm(llm.clone()));
            let engine = ToolEngine::new(ks);
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                if let Ok(dir) = handle.path().app_data_dir() {
                    let path = dir.join("abi-pilot").join("abi.db");
                    if let Ok(conn) = db::open(&path) {
                        let _ = scheduler::tick(&conn, &engine, llm.as_ref());
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|app, event| {
            if let tauri::WindowEvent::Destroyed = event {
                if let Some(state) = app.try_state::<SchedulerStop>() {
                    state.0.store(true, Ordering::Relaxed);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::app_info,
            commands::execution::sop_list,
            commands::execution::sop_upsert,
            commands::execution::sop_set_enabled,
            commands::execution::schedule_set,
            commands::execution::run_start,
            commands::execution::run_get,
            commands::execution::run_list,
            commands::execution::run_control,
            commands::execution::gate_list,
            commands::execution::gate_decide,
            commands::execution::dashboard_get,
            commands::execution::scheduler_tick,
            commands::execution::demo_install,
            commands::execution::tool_list,
            commands::execution::tool_grant_list,
            commands::execution::tool_grant_set,
            commands::execution::credential_list,
            commands::execution::credential_set,
            commands::execution::credential_remove,
            commands::execution::template_create,
            commands::execution::template_list,
            commands::execution::template_export,
            commands::execution::template_import,
            commands::execution::correction_add,
            commands::execution::correction_list,
            commands::execution::upgrade_candidate_list,
            commands::execution::upgrade_approve,
            commands::execution::owner_decision_list,
            commands::execution::report_get,
            commands::execution::tool_upsert,
            commands::execution::tool_remove,
            commands::execution::example_tools_install,
            commands::execution::maintenance_sop_install,
            commands::execution::delegation_demo_install,
            commands::execution::run_as_agent,
            commands::execution::sop_parse_text,
            commands::execution::llm_status,
        ])
        .run(tauri::generate_context!())
        .expect("ABI-Pilot 启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 冒烟：领域层建表在内存库可用，不需要 GUI 或真实数据目录。
    #[test]
    fn smoke_schema_ready() {
        let conn = infra::db::open_in_memory().expect("建表应成功");
        assert_eq!(infra::db::user_table_count(&conn).expect("统计"), 14);
        // v0.5：放权演示 SOP 可安装，n1 为 human-owned（可放权对象）。
        let id = domain::examples::install_delegation_demo(&conn).expect("安装演示 SOP");
        assert_eq!(id, "sop-delegation-demo");
    }
}
