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
#[serde(rename_all = "camelCase")]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub role: String,
    pub created_at: i64,
}

/// 当前登录用户（进程内存；重启清空）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// 管理权限门：仅 admin 可行；users 表为空时放行（让首个用户能自定角色建立 admin）。
/// 注意前端隐藏只是体验，这里才是真正的边界。
pub fn require_admin(conn: &Connection) -> AppResult<()> {
    if current_user().map(|u| u.role == "admin").unwrap_or(false) {
        return Ok(());
    }
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
        .map_err(|e| AppError::Db(e.to_string()))?;
    if n == 0 {
        return Ok(());
    }
    Err(AppError::ToolDenied("仅管理员可管理用户（建/改/删/改角色/重置密码）".into()))
}

/// 防止把系统里最后一个管理员降级——否则再无 admin 能建/改/删用户，系统锁死。
/// 只拦「当前是 admin 且要改成非 admin 且系统 admin 数 ≤ 1」这一种情况。
fn ensure_not_last_admin(conn: &Connection, user_id: &str, new_role: &str) -> AppResult<()> {
    if new_role == "admin" {
        return Ok(());
    }
    let current_role: String = conn
        .query_row("SELECT role FROM users WHERE id=?1", params![user_id], |r| r.get(0))
        .map_err(|e| AppError::Db(e.to_string()))?;
    if current_role != "admin" {
        return Ok(());
    }
    let admins: i64 = conn
        .query_row("SELECT COUNT(*) FROM users WHERE role='admin'", [], |r| r.get(0))
        .map_err(|e| AppError::Db(e.to_string()))?;
    if admins <= 1 {
        return Err(AppError::InvalidInput(
            "不能把最后一个管理员降级，否则无人能再管理用户".into(),
        ));
    }
    Ok(())
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
    require_admin(conn)?;
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
    // 首个用户强制 admin：否则建成非 admin 后既无管理员、表又非空，将永久锁死无法再管用户。
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
        .map_err(|e| AppError::Db(e.to_string()))?;
    let effective_role = if total == 0 { "admin" } else { role };
    // 用统一 id 生成器（纳秒时间戳 + 原子计数），避免同秒多用户撞 UNIQUE。
    let id = crate::models::new_id("usr");
    let ph = hash_password(password)?;
    conn.execute(
        "INSERT INTO users(id,username,display_name,role,password_hash,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![id, username, display_name, effective_role, ph, now_secs()],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(UserRow {
        id,
        username: username.to_string(),
        display_name: display_name.to_string(),
        role: effective_role.to_string(),
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
    require_admin(conn)?;
    valid_role(role)?;
    ensure_not_last_admin(conn, user_id, role)?;
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
    require_admin(conn)?;
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

/// 改用户显示名 + 角色（管理员操作；改的是当前登录用户时同步内存态）。
pub fn update_user(conn: &Connection, user_id: &str, display_name: &str, role: &str) -> AppResult<UserRow> {
    require_admin(conn)?;
    valid_role(role)?;
    ensure_not_last_admin(conn, user_id, role)?;
    if display_name.trim().is_empty() {
        return Err(AppError::InvalidInput("显示名不能为空".into()));
    }
    let n = conn
        .execute(
            "UPDATE users SET display_name=?1, role=?2 WHERE id=?3",
            params![display_name.trim(), role, user_id],
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
    if n == 0 {
        return Err(AppError::NotFound(format!("用户不存在：{}", user_id)));
    }
    if let Some(mut cur) = current_user() {
        if cur.id == user_id {
            cur.display_name = display_name.trim().to_string();
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

/// 重置用户密码（管理员操作，不需旧密码）。
pub fn reset_password(conn: &Connection, user_id: &str, new_password: &str) -> AppResult<()> {
    require_admin(conn)?;
    if new_password.is_empty() {
        return Err(AppError::InvalidInput("新密码不能为空".into()));
    }
    let ph = hash_password(new_password)?;
    let n = conn
        .execute("UPDATE users SET password_hash=?1 WHERE id=?2", params![ph, user_id])
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
    use std::sync::Mutex;

    // 当前登录用户是进程全局态：测试必须串行，避免并行时互相 clear/login 串扰。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn fresh() -> Connection {
        let c = db::open_in_memory().unwrap();
        clear_current_user().unwrap();
        c
    }

    /// 建库并登录一个 admin（首个用户，表空时允许自定 admin）。
    fn with_admin() -> Connection {
        let c = fresh();
        create_user(&c, "admin", "管理员", "admin", "adminpw").unwrap();
        login(&c, "admin", "adminpw").unwrap();
        c
    }

    #[test]
    fn hash_is_salted_and_verifies() {
        let _guard = TEST_LOCK.lock().unwrap();
        let h1 = hash_password("secret").unwrap();
        let h2 = hash_password("secret").unwrap();
        assert_ne!(h1, h2, "同密码不同盐应产生不同 hash");
        assert!(h1.contains('$') && h2.contains('$'));
    }

    #[test]
    fn first_user_is_forced_admin() {
        let _guard = TEST_LOCK.lock().unwrap();
        let c = fresh();
        // 表空时即使传 content，也强制为 admin——避免首个非 admin 把系统锁死。
        let first = create_user(&c, "first", "首个", "content", "pw").unwrap();
        assert_eq!(first.role, "admin", "首个用户必须强制 admin");
        login(&c, "first", "pw").unwrap();
        // 现在 admin 可建普通用户（传 content 保持 content）。
        let bob = create_user(&c, "bob", "Bob", "content", "pw2").unwrap();
        assert_eq!(bob.role, "content");
        assert_eq!(list_users(&c).unwrap().len(), 2);
    }

    #[test]
    fn non_admin_cannot_manage() {
        let _guard = TEST_LOCK.lock().unwrap();
        let c = with_admin();
        create_user(&c, "bob", "Bob", "content", "pw2").unwrap();
        // 换成普通用户登录。
        clear_current_user().unwrap();
        login(&c, "bob", "pw2").unwrap();
        assert!(create_user(&c, "carol", "C", "admin", "pw").is_err());
        assert!(set_role(&c, "bob", "admin").is_err());
        assert!(update_user(&c, "bob", "B2", "admin").is_err());
        assert!(reset_password(&c, "bob", "newpw").is_err());
        assert!(delete_user(&c, "bob").is_err());
    }

    #[test]
    fn login_wrong_password_and_duplicate() {
        let _guard = TEST_LOCK.lock().unwrap();
        let c = with_admin();
        assert!(login(&c, "admin", "wrong").is_err());
        assert!(current_user().is_some(), "错误密码不改变当前登录态");
        assert!(create_user(&c, "admin", "A2", "admin", "pw3").is_err(), "用户名重复被拒");
        assert!(create_user(&c, "carol", "C", "superadmin", "pw").is_err(), "非法角色被拒");
    }

    #[test]
    fn update_and_reset_password() {
        let _guard = TEST_LOCK.lock().unwrap();
        let c = with_admin();
        let u = create_user(&c, "bob", "Bob", "content", "oldpw").unwrap();
        let updated = update_user(&c, &u.id, "鲍勃", "ops").unwrap();
        assert_eq!(updated.display_name, "鲍勃");
        assert_eq!(updated.role, "ops");
        reset_password(&c, &u.id, "newpw").unwrap();
        clear_current_user().unwrap();
        assert!(login(&c, "bob", "oldpw").is_err(), "旧密码应失效");
        assert_eq!(login(&c, "bob", "newpw").unwrap().role, "ops", "新密码应可用且角色已更新");
    }

    #[test]
    fn set_role_syncs_current_user() {
        let _guard = TEST_LOCK.lock().unwrap();
        let c = with_admin();
        let id = current_user().unwrap().id;
        // 再加一个 admin，使当前用户（id）不再是唯一管理员，降级才被允许。
        create_user(&c, "b", "B", "admin", "pw").unwrap();
        set_role(&c, &id, "ops").unwrap();
        assert_eq!(current_user().unwrap().role, "ops", "改的是当前登录用户时内存应同步");
    }

    #[test]
    fn last_admin_cannot_be_demoted() {
        let _guard = TEST_LOCK.lock().unwrap();
        let c = fresh();
        let a = create_user(&c, "a", "A", "admin", "pw").unwrap(); // 首个强制 admin
        login(&c, "a", "pw").unwrap();
        // 系统只有一个 admin，改自己为 content 应被拒（set_role 与 update_user 都拦）。
        assert!(set_role(&c, &a.id, "content").is_err());
        assert!(update_user(&c, &a.id, "A2", "content").is_err());
        assert_eq!(current_user().unwrap().role, "admin");

        // 再加一个 admin 后，可把其中一个降级（系统仍剩一个 admin）。
        let b = create_user(&c, "b", "B", "admin", "pw").unwrap();
        update_user(&c, &b.id, "B2", "content").unwrap();
        assert_eq!(list_users(&c).unwrap().iter().filter(|u| u.role == "admin").count(), 1);
    }

    #[test]
    fn cannot_delete_current_user() {
        let _guard = TEST_LOCK.lock().unwrap();
        let c = with_admin();
        let id = current_user().unwrap().id;
        assert!(delete_user(&c, &id).is_err(), "不能删当前登录用户");
        let bob = create_user(&c, "bob", "Bob", "content", "pw").unwrap();
        delete_user(&c, &bob.id).unwrap();
        assert_eq!(list_users(&c).unwrap().len(), 1);
    }
}
