use tauri::Manager;
use crate::infra::db;
use crate::models::{AppInfo, AppResult};

/// 应用信息：v0.1 用它验证「壳能起 + 库建好」。
#[tauri::command]
pub fn app_info(app: tauri::AppHandle) -> AppResult<AppInfo> {
    let db_path = app
        .path()
        .app_data_dir()
        .map_err(|e| crate::models::AppError::Db(e.to_string()))?
        .join("abi-pilot")
        .join("abi.db");
    let conn = db::open(&db_path)?;
    let table_count = db::user_table_count(&conn)?;
    Ok(AppInfo {
        name: "ABI-Pilot".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        db_path: db_path.to_string_lossy().to_string(),
        table_count,
    })
}
