//! 应用配置与 MCP server 配置：文件存储，不写库、不存密钥值（设计 §3.1）。
//!
//! - config.json：非敏感应用配置（如 llm.provider / llm.host / llm.model）。
//! - mcp_servers.json：MCP server 配置（**为 MCP client 协议栈预备，当前不真连**）。
//! 两个文件都在 <app_data_dir>/abiflow/ 下，与 abi.db 同目录；原子写（temp + rename）避免半截文件。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::models::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub note: String,
    /// 传输方式：stdio（默认，按 command+args 启子进程）或 http（按 url POST）。旧配置无此字段→默认 stdio。
    #[serde(default = "default_transport")]
    pub transport: String,
    /// http 传输的目标 url（stdio 时忽略）。
    #[serde(default)]
    pub url: String,
}

fn default_transport() -> String { "stdio".into() }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpServers {
    #[serde(default)]
    pub servers: Vec<McpServer>,
}

fn dir() -> AppResult<PathBuf> {
    // 单测无 AppHandle：用系统临时目录下固定子目录，避免污染全局路径。
    #[cfg(test)]
    {
        let p = std::env::temp_dir().join("abiflow-app-config-test");
        std::fs::create_dir_all(&p).map_err(|e| AppError::Db(e.to_string()))?;
        return Ok(p);
    }
    #[cfg(not(test))]
    {
        // 由 setup 注入（见 set_data_dir）；未注入时返回错误。
        DATA_DIR.get().cloned().ok_or_else(|| AppError::Db("配置目录未初始化".into()))
    }
}

use std::sync::OnceLock;
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// setup 时注入数据目录（<app_data_dir>/abiflow）。
pub fn set_data_dir(p: PathBuf) {
    let _ = DATA_DIR.set(p);
}

fn config_path() -> AppResult<PathBuf> { Ok(dir()?.join("config.json")) }
fn mcp_path() -> AppResult<PathBuf> { Ok(dir()?.join("mcp_servers.json")) }
fn llm_env_path() -> AppResult<PathBuf> { Ok(dir()?.join("llm.env")) }

/// 把 <data_dir>/llm.env 里的键注入进程环境（给 build_llm 读）。
/// 文件不存在不算错（首次启动/只用 Mock 时没有它）；已存在的真实 env 变量不被覆盖。
/// 只读键名/路径，绝不回显值。
pub fn load_llm_env() {
    if let Ok(path) = llm_env_path() {
        if path.exists() {
            // from_path 不覆盖已设置的变量 → shell 里 export 的真值优先于文件。
            let _ = dotenvy::from_path(&path);
        }
    }
}

/// env 文件路径（给 UI/日志展示用，仅路径不含值）。
pub fn llm_env_path_display() -> String {
    llm_env_path().map(|p| p.display().to_string()).unwrap_or_default()
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> AppResult<T> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| AppError::Db(format!("配置解析失败：{}", e))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(AppError::Db(e.to_string())),
    }
}

fn write_json<T: Serialize>(path: &Path, val: &T) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::Db(e.to_string()))?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::File::create(&tmp).map_err(|e| AppError::Db(e.to_string()))?;
    f.write_all(serde_json::to_string_pretty(val).map_err(|e| AppError::Db(e.to_string()))?.as_bytes())
        .map_err(|e| AppError::Db(e.to_string()))?;
    f.sync_all().map_err(|e| AppError::Db(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

// ---- config ----
pub fn config_get_all() -> AppResult<BTreeMap<String, String>> {
    Ok(read_json::<AppConfig>(&config_path()?)?.values)
}

/// 写入一个非敏感配置项。拒绝密钥类键名（值不落明文是硬红线）。
pub fn config_set(key: &str, value: &str) -> AppResult<()> {
    let lower = key.to_ascii_lowercase();
    if lower.contains("secret") || lower.contains("password") || lower.contains("token")
        || lower.contains("key") || lower.contains("credential") {
        return Err(AppError::InvalidInput("拒绝写入密钥类配置项（密钥只进密钥链）".into()));
    }
    let mut cfg = read_json::<AppConfig>(&config_path()?)?;
    cfg.values.insert(key.to_string(), value.to_string());
    write_json(&config_path()?, &cfg)
}

/// 读取单个配置项（供 build_llm 用）。
pub fn config_value(key: &str) -> Option<String> {
    config_get_all().ok()?.get(key).cloned()
}

// ---- mcp servers（协议栈预备，当前不真连）----
pub fn mcp_list() -> AppResult<Vec<McpServer>> {
    Ok(read_json::<McpServers>(&mcp_path()?)?.servers)
}

pub fn mcp_add(server: McpServer) -> AppResult<Vec<McpServer>> {
    let mut doc = read_json::<McpServers>(&mcp_path()?)?;
    if doc.servers.iter().any(|s| s.id == server.id) {
        return Err(AppError::InvalidInput(format!("MCP server id 重复：{}", server.id)));
    }
    doc.servers.push(server);
    write_json(&mcp_path()?, &doc)?;
    Ok(doc.servers)
}

pub fn mcp_remove(id: &str) -> AppResult<Vec<McpServer>> {
    let mut doc = read_json::<McpServers>(&mcp_path()?)?;
    doc.servers.retain(|s| s.id != id);
    write_json(&mcp_path()?, &doc)?;
    Ok(doc.servers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip_and_rejects_secret_keys() {
        let k = format!("test.{}", std::process::id());
        config_set(&k, "v1").unwrap();
        assert_eq!(config_value(&k).as_deref(), Some("v1"));
        assert!(config_set("llm.secret", "x").is_err());
        assert!(config_set("db.password", "x").is_err());
        // 清理
        let mut cfg = read_json::<AppConfig>(&config_path().unwrap()).unwrap();
        cfg.values.remove(&k);
        write_json(&config_path().unwrap(), &cfg).unwrap();
    }

    #[test]
    fn mcp_crud() {
        let id = format!("srv-{}", std::process::id());
        let s = McpServer { id: id.clone(), command: "npx".into(), args: vec!["x".into()], note: "预备".into(), transport: "stdio".into(), url: String::new() };
        let list = mcp_add(s).unwrap();
        assert!(list.iter().any(|x| x.id == id));
        let list = mcp_remove(&id).unwrap();
        assert!(!list.iter().any(|x| x.id == id));
    }
}
