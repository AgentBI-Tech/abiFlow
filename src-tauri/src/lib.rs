pub mod commands;
pub mod domain;
pub mod infra;
pub mod models;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;

use crate::domain::event_bus::{EventSource, InMemoryEventSource};
use crate::domain::keystore::{KeyStore, MemoryKeyStore};
use crate::domain::llm::{self, LlmProvider};
use crate::domain::scheduler;
use crate::infra::app_config;
use crate::infra::db;
use crate::infra::events;
use crate::infra::tool_engine::ToolEngine;

struct SchedulerStop(Arc<AtomicBool>);
/// 密钥链实例（框架期为进程内存实现；接真实店铺时替换为 OS 密钥链，trait 不变）。
pub struct AppKeyStore(pub Arc<dyn KeyStore>);
/// LLM Provider（默认 Mock；本机未装 Ollama，装好后改配置即可切换，核心不动）。
pub struct AppLlm(pub Arc<dyn LlmProvider>);
/// 事件源（可插拔；当前进程内假事件源，供事件订阅触发用）。
pub struct AppEventSource(pub Arc<dyn EventSource>);

/// LLM 配置：优先读 config.json（config_get/set 写入），环境变量覆盖；重启生效（AppLlm 在 setup 构建一次）。
fn build_llm() -> Arc<dyn LlmProvider> {
    let cfg = |k: &str| app_config::config_value(k);
    let provider = std::env::var("ABI_LLM_PROVIDER").ok()
        .or_else(|| cfg("llm.provider")).unwrap_or_default();
    let host = std::env::var("ABI_LLM_HOST").ok()
        .or_else(|| cfg("llm.host")).unwrap_or_else(|| "http://127.0.0.1:11434".into());
    let model = std::env::var("ABI_LLM_MODEL").ok()
        .or_else(|| cfg("llm.model")).unwrap_or_else(|| "qwen2.5:3b".into());
    // API key 只从 env/llm.env 读，不进 config.json（config_set 已拒绝密钥类键名）。
    let api_key = std::env::var("ABI_LLM_API_KEY").ok().unwrap_or_default();
    llm::from_config(&provider, &host, &model, &api_key)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            // 后端→前端事件推送：store 状态变更后 emit，前端只当刷新提示。
            events::init(handle.clone());
            // 配置/MCP server 文件与 abi.db 同目录；须在 build_llm 之前注入。
            if let Ok(dir) = handle.path().app_data_dir() {
                app_config::set_data_dir(dir.join("abiflow"));
            }
            // LLM env 文件：在 build_llm 之前加载（键只从进程环境读，值不进 config.json/代码）。
            app_config::load_llm_env();
            // secrets.env：MCP stdio server 的凭据（如 Shopify token）由用户自填，启动时注入进程环境。
            // 值不进 config.json / mcp_servers.json / 代码 / 日志；shell export 的真值优先。
            app_config::load_secrets_env();
            let stop = Arc::new(AtomicBool::new(false));
            app.manage(SchedulerStop(stop.clone()));
            let ks: Arc<dyn KeyStore> = Arc::new(MemoryKeyStore::new());
            app.manage(AppKeyStore(ks.clone()));
            let llm: Arc<dyn LlmProvider> = build_llm();
            app.manage(AppLlm(llm.clone()));
            let events: Arc<dyn EventSource> = Arc::new(InMemoryEventSource::new());
            app.manage(AppEventSource(events.clone()));
            let engine = ToolEngine::new(ks);
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                if let Ok(dir) = handle.path().app_data_dir() {
                    let path = dir.join("abiflow").join("abi.db");
                    if let Ok(conn) = db::open(&path) {
                        let _ = scheduler::tick(&conn, events.as_ref(), &engine, llm.as_ref());
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
            commands::execution::sop_get,
            commands::execution::sop_upsert,
            commands::execution::config_get,
            commands::execution::config_set,
            commands::execution::mcp_server_list,
            commands::execution::mcp_server_add,
            commands::execution::mcp_server_remove,
            commands::execution::secrets_env_path,
            commands::execution::sop_set_enabled,
            commands::execution::schedule_set,
            commands::execution::run_start,
            commands::execution::run_get,
            commands::execution::run_list,
            commands::execution::run_control,
            commands::execution::run_retry,
            commands::execution::run_arbitrate,
            commands::execution::gate_list,
            commands::execution::gate_decide,
            commands::execution::dashboard_get,
            commands::execution::scheduler_tick,
            commands::execution::emit_event,
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
            commands::execution::template_params,
            commands::execution::template_reuse,
            commands::execution::correction_add,
            commands::execution::correction_list,
            commands::execution::upgrade_candidate_list,
            commands::execution::upgrade_approve,
            commands::execution::owner_decision_list,
            commands::execution::report_get,
            commands::execution::tool_upsert,
            commands::execution::tool_remove,
            commands::execution::example_tools_install,
            commands::execution::shopify_tools_install,
            commands::execution::maintenance_sop_install,
            commands::execution::delegation_demo_install,
            commands::execution::run_as_agent,
            commands::execution::sop_parse_text,
            commands::execution::llm_status,
            commands::execution::user_list,
            commands::execution::user_current,
            commands::execution::user_add,
            commands::execution::user_login,
            commands::execution::user_logout,
            commands::execution::user_set_role,
            commands::execution::user_remove,
            commands::execution::user_update,
            commands::execution::user_reset_password,
        ])
        .run(tauri::generate_context!())
        .expect("abiFlow 启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 冒烟：领域层建表在内存库可用，不需要 GUI 或真实数据目录。
    #[test]
    fn smoke_schema_ready() {
        let conn = infra::db::open_in_memory().expect("建表应成功");
        assert_eq!(infra::db::user_table_count(&conn).expect("统计"), 15); // v7 加 users 表（14→15）
        // v0.5：放权演示 SOP 可安装，n1 为 human-owned（可放权对象）。
        let id = domain::examples::install_delegation_demo(&conn).expect("安装演示 SOP");
        assert_eq!(id, "sop-delegation-demo");
    }
}
