//! 内置 MCP server（filesystem / shell）——进程内实现 tools/list 与 tools/call，
//! 经 InProcessTransport 走同一个 McpClient 握手/发现/调用流程（见 domain/mcp.rs）。
//! browser 走 stdio 子进程隔离（tests/fixtures/builtin_browser_mcp.mjs），不在此模块。
//!
//! 安全：filesystem 限制在 sandbox 根目录内（拒绝 .. 与绝对路径穿越）；
//! shell 只放行固定只读命令白名单，参数校验、无网络、杀进程用 Stdio piped。

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

use crate::models::{AppError, AppResult};

/// 内置 server id（写入 mcp_servers.json 与 tools.scope_json）。
pub const FS_SERVER: &str = "builtin-filesystem";
pub const SHELL_SERVER: &str = "builtin-shell";

const SHELL_TIMEOUT_SECS: u64 = 10;
const SHELL_OUTPUT_LIMIT: usize = 64 * 1024;

/// filesystem sandbox 根目录：<app_data_dir>/abiflow/builtin-fs。
/// 由调用方传入（commands 层用 app.path() 取），domain 层不碰 AppHandle。
pub fn fs_root(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("abiflow").join("builtin-fs")
}

/// tools/list：返回该内置 server 的工具元数据（MCP 形状）。
pub fn tools_list(server_id: &str) -> AppResult<Value> {
    let tools = match server_id {
        FS_SERVER => vec![
            tool("read_file", "读取 sandbox 内文本文件", json!({
                "type":"object", "properties":{"path":{"type":"string"}}, "required":["path"]
            })),
            tool("list_dir", "列出 sandbox 内目录", json!({
                "type":"object", "properties":{"path":{"type":"string"}}, "required":["path"]
            })),
            tool("search", "在 sandbox 内按文件名子串搜索", json!({
                "type":"object", "properties":{"path":{"type":"string"},"pattern":{"type":"string"}}, "required":["path","pattern"]
            })),
        ],
        SHELL_SERVER => vec![
            tool("run", "执行白名单只读命令（无写、无网络）", json!({
                "type":"object",
                "properties":{"command":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}},
                "required":["command"]
            })),
            tool("allowlist", "查看当前允许的命令白名单", json!({"type":"object","properties":{}})),
        ],
        other => return Err(AppError::McpUnavailable(format!("未知内置 server：{}", other))),
    };
    Ok(json!({ "tools": tools }))
}

fn tool(name: &str, desc: &str, schema: Value) -> Value {
    json!({ "name": name, "description": desc, "inputSchema": schema })
}

/// tools/call：执行工具，返回 MCP 形状 { content: [{type:"text",text:...}] }。
/// `requires_human` 由上层 tool_engine 在 text 里检测（browser 用），本模块不产生。
pub fn tools_call(server_id: &str, name: &str, args: &Value, app_data_dir: &Path) -> AppResult<Value> {
    match server_id {
        FS_SERVER => fs_call(name, args, &fs_root(app_data_dir)),
        SHELL_SERVER => shell_call(name, args),
        other => Err(AppError::McpUnavailable(format!("未知内置 server：{}", other))),
    }
}

fn text_result(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}

// ---- filesystem ----

/// 把相对路径解析到 sandbox 根内；拒绝绝对路径、..、隐藏穿越。
fn resolve_in_sandbox(root: &Path, rel: &str) -> AppResult<PathBuf> {
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err(AppError::InvalidInput("filesystem 工具不接受绝对路径".into()));
    }
    for c in p.components() {
        if matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)) {
            return Err(AppError::InvalidInput("filesystem 工具拒绝路径穿越".into()));
        }
    }
    let full = root.join(p);
    // 再次确保规范化后仍在 root 内（防符号链接等边界）。
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canon = full.canonicalize().map_err(|_| AppError::InvalidInput(format!("路径不存在：{}", rel)))?;
    if !canon.starts_with(&canon_root) {
        return Err(AppError::InvalidInput("filesystem 工具拒绝越界路径".into()));
    }
    Ok(canon)
}

fn fs_call(name: &str, args: &Value, root: &Path) -> AppResult<Value> {
    // sandbox 根不存在则建（只读工具不创建文件，只建空目录）。
    if !root.exists() {
        std::fs::create_dir_all(root).map_err(|e| AppError::Db(e.to_string()))?;
    }
    let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let full = resolve_in_sandbox(root, rel)?;
    match name {
        "read_file" => {
            if !full.is_file() { return Err(AppError::InvalidInput("不是文件".into())); }
            let meta = std::fs::metadata(&full).map_err(|e| AppError::Db(e.to_string()))?;
            if meta.len() > SHELL_OUTPUT_LIMIT as u64 {
                return Err(AppError::InvalidInput("文件超过 64KB 上限".into()));
            }
            let s = std::fs::read_to_string(&full).map_err(|e| AppError::Db(e.to_string()))?;
            Ok(text_result(s))
        }
        "list_dir" => {
            if !full.is_dir() { return Err(AppError::InvalidInput("不是目录".into())); }
            let mut names = Vec::new();
            for e in std::fs::read_dir(&full).map_err(|e| AppError::Db(e.to_string()))? {
                let e = e.map_err(|e| AppError::Db(e.to_string()))?;
                names.push(format!("{}{}", e.file_name().to_string_lossy(), if e.path().is_dir() { "/" } else { "" }));
            }
            names.sort();
            Ok(text_result(names.join("\n")))
        }
        "search" => {
            let pat = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            if pat.is_empty() { return Err(AppError::InvalidInput("pattern 不能为空".into())); }
            let mut hits = Vec::new();
            collect_matches(&full, pat, &mut hits, 0)?;
            Ok(text_result(hits.join("\n")))
        }
        other => Err(AppError::McpUnavailable(format!("filesystem 无此工具：{}", other))),
    }
}

fn collect_matches(dir: &Path, pat: &str, out: &mut Vec<String>, depth: usize) -> AppResult<()> {
    if depth > 6 || out.len() > 200 { return Ok(()); } // 防递归爆炸
    for e in std::fs::read_dir(dir).map_err(|e| AppError::Db(e.to_string()))? {
        let e = e.map_err(|e| AppError::Db(e.to_string()))?;
        let name = e.file_name().to_string_lossy().into_owned();
        if name.contains(pat) { out.push(e.path().to_string_lossy().into_owned()); }
        if e.path().is_dir() { collect_matches(&e.path(), pat, out, depth + 1)?; }
    }
    Ok(())
}

// ---- shell（只读命令白名单）----

const SHELL_ALLOW: &[&str] = &["ls", "cat", "find", "pwd", "git"];

fn shell_call(name: &str, args: &Value) -> AppResult<Value> {
    if name == "allowlist" {
        return Ok(text_result(SHELL_ALLOW.join(", ")));
    }
    if name != "run" {
        return Err(AppError::McpUnavailable(format!("shell 无此工具：{}", name)));
    }
    let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let argv: Vec<String> = args.get("args")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();

    // 白名单：命令 + 固定子命令（git status/log）。
    if !SHELL_ALLOW.contains(&cmd) {
        return Err(AppError::InvalidInput(format!("命令不在白名单：{}", cmd)));
    }
    if cmd == "git" && !matches!(argv.first().map(String::as_str), Some("status") | Some("log")) {
        return Err(AppError::InvalidInput("git 只允许 status / log".into()));
    }
    // 拒绝任何 shell 元字符（无管道、无重定向、无命令替换）。
    let all = std::iter::once(cmd).chain(argv.iter().map(String::as_str));
    for s in all {
        if s.chars().any(|c| matches!(c, '|' | '&' | ';' | '$' | '`' | '>' | '<' | '\n')) {
            return Err(AppError::InvalidInput("shell 工具拒绝 shell 元字符".into()));
        }
    }

    // 同步执行，stdout/stderr 都 pipe，超时靠 try_wait 轮询（不引入 tokio）。
    let mut child = Command::new(cmd)
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::McpUnavailable(format!("启动命令失败：{}", e)))?;
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| AppError::Db(e.to_string()))? {
            Some(s) => break s,
            None if start.elapsed() > Duration::from_secs(SHELL_TIMEOUT_SECS) => {
                let _ = child.kill();
                return Err(AppError::McpUnavailable("shell 命令超时（10s）".into()));
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let out = child.wait_with_output().map_err(|e| AppError::Db(e.to_string()))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let err_text = String::from_utf8_lossy(&out.stderr);
    if !err_text.is_empty() { text.push_str("\n[stderr]\n"); text.push_str(&err_text); }
    if text.len() > SHELL_OUTPUT_LIMIT { text.truncate(SHELL_OUTPUT_LIMIT); text.push_str("\n…(截断)"); }
    if !status.success() {
        return Ok(text_result(format!("[exit {}] {}", status, text)));
    }
    Ok(text_result(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_rejects_absolute_and_dotdot() {
        let root = Path::new("/tmp/abiflow-fs-test");
        assert!(resolve_in_sandbox(root, "/etc/passwd").is_err());
        assert!(resolve_in_sandbox(root, "../secret").is_err());
        assert!(resolve_in_sandbox(root, "a/../../b").is_err());
    }

    #[test]
    fn shell_rejects_non_allowlist_and_metachars() {
        assert!(shell_call("run", &json!({"command":"rm","args":["-rf","/"]})).is_err());
        assert!(shell_call("run", &json!({"command":"ls","args":["-l; rm -rf /"]})).is_err());
        assert!(shell_call("run", &json!({"command":"git","args":["push"]})).is_err());
        let ok = shell_call("allowlist", &json!({})).unwrap();
        assert!(ok["content"][0]["text"].as_str().unwrap().contains("git"));
    }
}
