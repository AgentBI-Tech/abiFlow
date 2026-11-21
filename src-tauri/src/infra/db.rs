use std::path::Path;
use rusqlite::Connection;
use crate::models::{AppError, AppResult};

const MIGRATION_V1: &str = include_str!("../../migrations/001_init.sql");
const MIGRATION_V3: &str = include_str!("../../migrations/003_experience.sql");
const MIGRATION_V4: &str = include_str!("../../migrations/004_owner_delegation.sql");
const SCHEMA_VERSION: i64 = 5;

/// v2 要新增的列（table, column, ALTER 语句）。逐列按存在性执行，保证幂等。
const V2_COLUMNS: &[(&str, &str, &str)] = &[
    ("runs", "definition_snapshot", "ALTER TABLE runs ADD COLUMN definition_snapshot TEXT"),
    ("runs", "current_node", "ALTER TABLE runs ADD COLUMN current_node TEXT"),
    ("runs", "outputs_json", "ALTER TABLE runs ADD COLUMN outputs_json TEXT NOT NULL DEFAULT '{}'"),
    ("runs", "branch_skipped", "ALTER TABLE runs ADD COLUMN branch_skipped TEXT NOT NULL DEFAULT '[]'"),
    ("steps", "node_id", "ALTER TABLE steps ADD COLUMN node_id TEXT"),
    ("schedules", "interval_secs", "ALTER TABLE schedules ADD COLUMN interval_secs INTEGER NOT NULL DEFAULT 86400"),
];

/// 打开（或创建）数据库并执行幂等迁移。
pub fn open(path: &Path) -> AppResult<Connection> {
    // SQLite 不会替我们建父目录；app_info 传入的 <app_data_dir>/abiflow/ 首次可能不存在。
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
        .map_err(|e| AppError::Db(format!("迁移 v1 失败：{}", e)))?;
    // v2: 逐列 ADD COLUMN（SQLite 无 IF NOT EXISTS），按列存在性守卫，保证幂等重放。
    for (table, column, ddl) in V2_COLUMNS {
        if !column_exists(conn, table, column)? {
            conn.execute(ddl, [])
                .map_err(|e| AppError::Db(format!("迁移 v2 失败（{}）：{}", column, e)))?;
        }
    }
    conn.execute(
        "INSERT OR IGNORE INTO schema_meta(version, applied_at) VALUES (2, CAST(strftime('%s','now') AS INTEGER))",
        [],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    // v3: corrections 表（全 IF NOT EXISTS，可直接重放）。
    conn.execute_batch(MIGRATION_V3)
        .map_err(|e| AppError::Db(format!("迁移 v3 失败：{}", e)))?;
    conn.execute(
        "INSERT OR IGNORE INTO schema_meta(version, applied_at) VALUES (3, CAST(strftime('%s','now') AS INTEGER))",
        [],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    // v4: owner_decisions 表（全 IF NOT EXISTS，可直接重放）。
    conn.execute_batch(MIGRATION_V4)
        .map_err(|e| AppError::Db(format!("迁移 v4 失败：{}", e)))?;
    conn.execute(
        "INSERT OR IGNORE INTO schema_meta(version, applied_at) VALUES (4, CAST(strftime('%s','now') AS INTEGER))",
        [],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    // v5: schedules.cron_expr（NULL = 旧的 interval 模式，向后兼容；非 NULL = cron 模式）。
    // SQLite ALTER 无 IF NOT EXISTS，按列存在性守卫保证幂等重放。
    if !column_exists(conn, "schedules", "cron_expr")? {
        conn.execute(
            "ALTER TABLE schedules ADD COLUMN cron_expr TEXT",
            [],
        )
        .map_err(|e| AppError::Db(format!("迁移 v5 失败（cron_expr）：{}", e)))?;
    }
    conn.execute(
        "INSERT OR IGNORE INTO schema_meta(version, applied_at) VALUES (5, CAST(strftime('%s','now') AS INTEGER))",
        [],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
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

fn column_exists(conn: &Connection, table: &str, column: &str) -> AppResult<bool> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", table))
        .map_err(|e| AppError::Db(e.to_string()))?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| AppError::Db(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(names.iter().any(|n| n == column))
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
        // 14 张业务表（含 schema_meta）
        assert_eq!(n, 14, "应建 14 张表，实得 {}", n);
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = open_in_memory().expect("首次迁移");
        // 重放不应报错：v1/v3 全 IF NOT EXISTS，v2 逐列存在性守卫。
        conn.execute_batch(MIGRATION_V1).expect("重放迁移 v1 应成功");
        for (table, column, ddl) in V2_COLUMNS {
            if !column_exists(&conn, table, column).expect("查列") {
                conn.execute(ddl, []).expect("重放迁移 v2 应成功");
            }
        }
        conn.execute_batch(MIGRATION_V3).expect("重放迁移 v3 应成功");
        conn.execute_batch(MIGRATION_V4).expect("重放迁移 v4 应成功");
        let n = user_table_count(&conn).expect("统计应成功");
        assert_eq!(n, 14);
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
        // 回归测试：app_info 传 <data>/abiflow/abi.db，父目录首次不存在，open 必须自建。
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("level1").join("level2").join("abi.db");
        assert!(!nested.parent().unwrap().exists(), "父目录起初不应存在");
        let conn = open(&nested).expect("open 应创建父目录并建表");
        assert!(nested.exists(), "db 文件应生成");
        assert_eq!(user_table_count(&conn).expect("统计"), 14);
    }

    /// v5：schedules.cron_expr 列存在（NULL = interval 模式）。
    #[test]
    fn schedules_cron_expr_column_ready() {
        let conn = open_in_memory().expect("迁移");
        assert!(column_exists(&conn, "schedules", "cron_expr").expect("查列"), "缺 cron_expr 列");
        let v: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_meta", [], |r| r.get(0))
            .expect("查版本");
        assert_eq!(v, 5, "schema 版本应为 5");
    }

    /// owner_decisions 表存在且字段齐全（v0.5 实际放权审计）。
    #[test]
    fn owner_decisions_table_ready() {
        let conn = open_in_memory().expect("迁移");
        let mut stmt = conn
            .prepare("PRAGMA table_info(owner_decisions)")
            .expect("pragma");
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .expect("query")
            .map(|c| c.expect("col"))
            .collect();
        for need in ["sop_id", "node_id", "from_owner", "to_owner", "decision", "correction_count"] {
            assert!(cols.iter().any(|c| c == need), "缺列 {}", need);
        }
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
