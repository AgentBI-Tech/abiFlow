use std::path::Path;
use rusqlite::Connection;
use crate::models::{AppError, AppResult};

const MIGRATION_V1: &str = include_str!("../../migrations/001_init.sql");
const SCHEMA_VERSION: i64 = 1;

/// 打开（或创建）数据库并执行幂等迁移。
pub fn open(path: &Path) -> AppResult<Connection> {
    // SQLite 不会替我们建父目录；app_info 传入的 <app_data_dir>/abi-pilot/ 首次可能不存在。
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::Db(format!("创建数据目录失败：{}", e)))?;
    }
    let conn = Connection::open(path).map_err(|e| AppError::Db(e.to_string()))?;
    migrate(&conn)?;
    Ok(conn)
}

/// 在内存库上建表——供单元测试用，不碰磁盘。
pub fn open_in_memory() -> AppResult<Connection> {
    let conn = Connection::open_in_memory().map_err(|e| AppError::Db(e.to_string()))?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(MIGRATION_V1)
        .map_err(|e| AppError::Db(format!("迁移失败：{}", e)))?;
    let v: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_meta", [], |r| r.get(0))
        .map_err(|e| AppError::Db(e.to_string()))?;
    if v < SCHEMA_VERSION {
        return Err(AppError::Db(format!(
            "schema 版本过低：{} < {}", v, SCHEMA_VERSION
        )));
    }
    Ok(())
}

/// 统计用户表数量（排除 sqlite_ 内部表），app_info 用。
pub fn user_table_count(conn: &Connection) -> AppResult<usize> {
    let mut stmt = conn
        .prepare("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .map_err(|e| AppError::Db(e.to_string()))?;
    let n: i64 = stmt
        .query_row([], |r| r.get(0))
        .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(n as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_creates_all_tables() {
        let conn = open_in_memory().expect("内存库迁移应成功");
        let n = user_table_count(&conn).expect("统计应成功");
        // 12 张业务表（含 schema_meta）
        assert_eq!(n, 12, "应建 12 张表，实得 {}", n);
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = open_in_memory().expect("首次迁移");
        // 重放同一脚本不应报错（IF NOT EXISTS + INSERT OR IGNORE）
        conn.execute_batch(MIGRATION_V1).expect("重放迁移应成功");
        let n = user_table_count(&conn).expect("统计应成功");
        assert_eq!(n, 12);
    }

    #[test]
    fn credentials_store_only_ref_not_secret() {
        let conn = open_in_memory().expect("迁移");
        // 验证表结构：只有 ref_id / storage_key / provider / label / 时间戳，没有 value 字段。
        let mut stmt = conn
            .prepare("PRAGMA table_info(credentials)")
            .expect("pragma");
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .expect("query")
            .map(|c| c.expect("col"))
            .collect();
        assert!(cols.contains(&"ref_id".to_string()));
        assert!(cols.contains(&"storage_key".to_string()));
        assert!(!cols.iter().any(|c| c == "value" || c == "secret"), "密钥值不得入库");
    }

    #[test]
    fn open_creates_nested_parent_dir() {
        // 回归测试：app_info 传 <data>/abi-pilot/abi.db，父目录首次不存在，open 必须自建。
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("level1").join("level2").join("abi.db");
        assert!(!nested.parent().unwrap().exists(), "父目录起初不应存在");
        let conn = open(&nested).expect("open 应创建父目录并建表");
        assert!(nested.exists(), "db 文件应生成");
        assert_eq!(user_table_count(&conn).expect("统计"), 12);
    }

    #[test]
    fn foreign_keys_enforced() {
        let conn = open_in_memory().expect("迁移");
        let fk: i64 = conn
            .pragma_query_value(None, "foreign_keys", |r| r.get(0))
            .expect("pragma");
        assert_eq!(fk, 1, "外键约束应开启");
    }
}
