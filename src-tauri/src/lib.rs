pub mod commands;
pub mod domain;
pub mod infra;
pub mod models;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;

use crate::domain::executor::DemoTools;
use crate::domain::keystore::{KeyStore, MemoryKeyStore};
use crate::domain::scheduler;
use crate::infra::db;

struct SchedulerStop(Arc<AtomicBool>);
/// 密钥链实例（框架期为进程内存实现；接真实店铺时替换为 OS 密钥链，trait 不变）。
pub struct AppKeyStore(pub Arc<dyn KeyStore>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let stop = Arc::new(AtomicBool::new(false));
            app.manage(SchedulerStop(stop.clone()));
            app.manage(AppKeyStore(Arc::new(MemoryKeyStore::new())));
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                if let Ok(dir) = handle.path().app_data_dir() {
                    let path = dir.join("abi-pilot").join("abi.db");
                    if let Ok(conn) = db::open(&path) {
                        let _ = scheduler::tick(&conn, &DemoTools);
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
            commands::execution::report_get,
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
        assert_eq!(infra::db::user_table_count(&conn).expect("统计"), 13);
    }
}
