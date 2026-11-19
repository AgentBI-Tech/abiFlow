pub mod commands;
pub mod domain;
pub mod infra;
pub mod models;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![commands::app::app_info])
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
        assert_eq!(infra::db::user_table_count(&conn).expect("统计"), 12);
    }
}
