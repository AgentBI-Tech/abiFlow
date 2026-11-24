//! 用户管理与登录（本地多用户 + 角色 + 密码 hash）。
//!
//! 纪律（2026-11-24 拍板）：
//! - 用户名唯一；每个用户一个角色（content/ops/admin/agent），角色决定可见数据面（见 gateway::ROLE_PLANES）。
//! - 密码不存明文：password_hash = "salt$hash"，salt 为 getrandom 随机 16 字节的十六进制，
//!   hash = SHA-256(salt_bytes || 明文密码字节) 的十六进制。明文只从登录请求进内存，不落盘/不进日志/不进 DB。
//! - 当前登录用户是进程内存状态：重启需重新登录（密码登录语义）。未登录时 run 回退 agent，保持既有行为。
//! - 只依赖 rusqlite Connection，domain 层可单测（open_in_memory）。

use std::sync::{Mutex, OnceLock};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::{AppError, AppResult};

/// 合法角色，与 gateway::ROLE_PLANES 的键一致。
pub const ROLES: &[&str] = &["content", "ops", "admin", "agent"];

/// 列表中返回的用户行（不含密码 hash）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub role: String,
    pub created_at: i64,
}

/// 当前登录用户（进程内存；重启清空）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentUser {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub role: String,
}

static CURRENT: OnceLock<Mutex<Option<CurrentUser>>> = OnceLock::new();

fn current_slot() -> &'static Mutex<Option<CurrentUser>> {
    CURRENT.get_or_init(|| Mutex::new(None))
}

pub fn current_user() -> Option<CurrentUser> {
    current_slot().lock().ok().and_then(|g| g.clone())
}

pub fn set_current_user(u: CurrentUser) -> AppResult<()> {
    let mut g = current_slot().lock().map_err(|_| AppError::Db("登录状态锁中毒".into()))?;
    *g = Some(u);
    Ok(())
}

pub fn clear_current_user() -> AppResult<()> {
    let mut g = current_slot().lock().map_err(|_| AppError::Db("登录状态锁中毒".into()))?;
    *g = None;
    Ok(())
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn valid_role(role: &str) -> AppResult<()> {
    if ROLES.contains(&role) {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "未知角色：{}（合法：{}）",
            role,
            ROLES.join("/")
        )))
    }
}

/// 生成 salt$hash：salt 16 字节随机，hash = SHA-256(salt_bytes || 明文)。
pub fn hash_password(password: &str) -> AppResult<String> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| AppError::Db(format!("随机盐生成失败：{}", e)))?;
    let salt_hex: String = salt.iter().map(|b| format!("{:02x}", b)).collect();
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(password.as_bytes());
    let hash_hex: String = hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect();
    Ok(format!("{}${}", salt_hex, hash_hex))
}

/// 校验密码：从 salt$hash 解析盐，重算比较（常量时间比较避免时序侧信道）。
fn verify_password(password: &str, stored: &str) -> bool {
    let (salt_hex, hash_hex) = match stored.split_once('$') {
        Some(p) => p,
        None => return false,
    };
    let salt = match hex_decode(salt_hex) {
        Some(s) => s,
        None => return false,
    };
    let expected = match hex_decode(hash_hex) {
        Some(h) => h,
        None => return false,
    };
    let mut hasher = Sha256::new();
    hasher.update(&salt);
    hasher.update(password.as_bytes());
    let actual = hasher.finalize();
    // 常量时间比较（长度固定 32 字节）。
    let mut diff = 0u8;
    for (a, b) in actual.iter().zip(expected.iter()) {
        diff |= a ^ b;
    }
    diff == 0 && expected.len() == 32
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

fn row_from_db(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserRow> {
    Ok(UserRow {
        id: row.get(0)?,
        username: row.get(1)?,
        display_name: row.get(2)?,
        role: row.get(3)?,
        created_at: row.get(4)?,
    })
}

/// 建用户。username 唯一，重复返回 InvalidInput；密码 hash 后存库。
pub fn create_user(
    conn: &Connection,
    username: &str,
    display_name: &str,
    role: &str,
    password: &str,
) -> AppResult<UserRow> {
    valid_role(role)?;
    if username.trim().is_empty() || password.is_empty() {
        return Err(AppError::InvalidInput("用户名与密码不能为空".into()));
    }
    let exists: i64 = conn
        .query_row("SELECT COUNT(*) FROM users WHERE username=?1", params![username], |r| r.get(0))
        .map_err(|e| AppError::Db(e.to_string()))?;
    if exists > 0 {
        return Err(AppError::InvalidInput(format!("用户名已存在：{}", username)));
    }
    // 用统一 id 生成器（纳秒时间戳 + 原子计数），避免同秒多用户撞 UNIQUE。
    let id = crate::models::new_id("usr");
    let ph = hash_password(password)?;
    conn.execute(
        "INSERT INTO users(id,username,display_name,role,password_hash,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![id, username, display_name, role, ph, now_secs()],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(UserRow {
        id,
        username: username.to_string(),
        display_name: display_name.to_string(),
        role: role.to_string(),
        created_at: now_secs(),
    })
}

pub fn list_users(conn: &Connection) -> AppResult<Vec<UserRow>> {
    let mut stmt = conn
        .prepare("SELECT id,username,display_name,role,created_at FROM users ORDER BY created_at,id")
        .map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map([], row_from_db)
        .map_err(|e| AppError::Db(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

fn find_user(conn: &Connection, username: &str) -> AppResult<Option<(UserRow, String)>> {
    let mut stmt = conn
        .prepare("SELECT id,username,display_name,role,created_at,password_hash FROM users WHERE username=?1")
        .map_err(|e| AppError::Db(e.to_string()))?;
    let mut rows = stmt
        .query_map(params![username], |row| {
            Ok((
                UserRow {
                    id: row.get(0)?,
                    username: row.get(1)?,
                    display_name: row.get(2)?,
                    role: row.get(3)?,
                    created_at: row.get(4)?,
                },
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|e| AppError::Db(e.to_string()))?;
    match rows.next() {
        Some(r) => r.map(Some).map_err(|e| AppError::Db(e.to_string())),
        None => Ok(None),
    }
}

/// 登录：用户名+密码正确则设为当前用户并返回；错误密码返回 InvalidInput（不区分用户不存在，防枚举）。
pub fn login(conn: &Connection, username: &str, password: &str) -> AppResult<CurrentUser> {
    let found = find_user(conn, username)?;
    let (row, ph) = found.ok_or_else(|| AppError::InvalidInput("用户名或密码错误".into()))?;
    if !verify_password(password, &ph) {
        return Err(AppError::InvalidInput("用户名或密码错误".into()));
    }
    let cur = CurrentUser {
        id: row.id,
        username: row.username,
        display_name: row.display_name,
        role: row.role,
    };
    set_current_user(cur.clone())?;
    Ok(cur)
}

/// 改用户角色（权限设置的核心）。
pub fn set_role(conn: &Connection, user_id: &str, role: &str) -> AppResult<UserRow> {
    valid_role(role)?;
    let n = conn
        .execute("UPDATE users SET role=?1 WHERE id=?2", params![role, user_id])
        .map_err(|e| AppError::Db(e.to_string()))?;
    if n == 0 {
        return Err(AppError::NotFound(format!("用户不存在：{}", user_id)));
    }
    // 若改的是当前登录用户，同步内存里的角色。
    if let Some(mut cur) = current_user() {
        if cur.id == user_id {
            cur.role = role.to_string();
            set_current_user(cur)?;
        }
    }
    let mut stmt = conn
        .prepare("SELECT id,username,display_name,role,created_at FROM users WHERE id=?1")
        .map_err(|e| AppError::Db(e.to_string()))?;
    stmt.query_row(params![user_id], row_from_db)
        .map_err(|e| AppError::Db(e.to_string()))
}

/// 删除用户（不能删当前登录用户）。
pub fn delete_user(conn: &Connection, user_id: &str) -> AppResult<()> {
    if current_user().map(|u| u.id == user_id).unwrap_or(false) {
        return Err(AppError::InvalidInput("不能删除当前登录用户".into()));
    }
    let n = conn
        .execute("DELETE FROM users WHERE id=?1", params![user_id])
        .map_err(|e| AppError::Db(e.to_string()))?;
    if n == 0 {
        return Err(AppError::NotFound(format!("用户不存在：{}", user_id)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::db;

    fn fresh() -> Connection {
        let c = db::open_in_memory().unwrap();
        clear_current_user().unwrap();
        c
    }

    #[test]
    fn hash_is_salted_and_verifies() {
        let h1 = hash_password("secret").unwrap();
        let h2 = hash_password("secret").unwrap();
        assert_ne!(h1, h2, "同密码不同盐应产生不同 hash");
        assert!(h1.contains('$') && h2.contains('$'));
    }

    #[test]
    fn create_list_login_wrong_password() {
        let c = fresh();
        create_user(&c, "alice", "Alice", "ops", "pw1").unwrap();
        create_user(&c, "bob", "Bob", "content", "pw2").unwrap();
        let users = list_users(&c).unwrap();
        assert_eq!(users.len(), 2);
        assert!(users.iter().any(|u| u.username == "alice" && u.role == "ops"));

        // 错误密码不登录。
        assert!(login(&c, "alice", "wrong").is_err());
        assert!(current_user().is_none());
        // 正确密码登录并设为当前用户。
        let cur = login(&c, "alice", "pw1").unwrap();
        assert_eq!(cur.username, "alice");
        assert_eq!(cur.role, "ops");
        assert_eq!(current_user().unwrap().username, "alice");

        // 用户名重复被拒。
        assert!(create_user(&c, "alice", "A2", "admin", "pw3").is_err());
        // 非法角色被拒。
        assert!(create_user(&c, "carol", "C", "superadmin", "pw").is_err());
    }

    #[test]
    fn set_role_syncs_current_user() {
        let c = fresh();
        create_user(&c, "u1", "U1", "content", "pw").unwrap();
        login(&c, "u1", "pw").unwrap();
        set_role(&c, &current_user().unwrap().id, "admin").unwrap();
        assert_eq!(current_user().unwrap().role, "admin");
    }

    #[test]
    fn cannot_delete_current_user() {
        let c = fresh();
        let u = create_user(&c, "u1", "U1", "agent", "pw").unwrap();
        login(&c, "u1", "pw").unwrap();
        assert!(delete_user(&c, &u.id).is_err());
        clear_current_user().unwrap();
        delete_user(&c, &u.id).unwrap();
        assert_eq!(list_users(&c).unwrap().len(), 0);
    }
}
