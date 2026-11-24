use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::domain::executor::ToolRunner;
use crate::domain::keystore::KeyStore;
use crate::domain::mcp::{HttpTransport, McpClient, StdioTransport};
use crate::infra::app_config;
use crate::models::{AppError, AppResult};

/// 配置驱动的工具引擎：按 tools.scope_json 的 `kind` 分派。
/// - `builtin`：内置只读 demo（合成数据）。
/// - `http`：通用 HTTP 工具（method/url/query/headers/body 均由配置 + 占位符决定）。
/// - `mcp`：MCP server 工具（server 指向 mcp_servers.json 的配置，tool 是 tools/list 里的名字）。
///   stdio 按 command+args 启子进程、http 按 url POST；协议走 domain/mcp.rs。
///   **当前为协议栈框架**：需真实 MCP server 才能真正调用，内置 fake server 用于端到端验证。
///
/// 不绑定任何具体站点：谁都能改 scope_json 指向自己的服务。密钥只从 keystore 按 ref 取，
/// 占位符值只进请求、不进任何日志/DB。
pub struct ToolEngine {
    ks: Arc<dyn KeyStore>,
}

impl ToolEngine {
    pub fn new(ks: Arc<dyn KeyStore>) -> Self {
        Self { ks }
    }
}

impl ToolRunner for ToolEngine {
    fn run(&self, tool: &str, args: &Value, scope: &Value) -> AppResult<Value> {
        let kind = scope.get("kind").and_then(|v| v.as_str()).unwrap_or("builtin");
        match kind {
            "builtin" => run_builtin(tool, args),
            "http" => run_http(args, scope, self.ks.as_ref()),
            "http-sse" => run_http_sse(args, scope, self.ks.as_ref()),
            "mcp" => run_mcp(args, scope, self.ks.as_ref()),
            other => Err(AppError::McpUnavailable(format!("未知工具 kind：{}", other))),
        }
    }
}

fn run_builtin(tool: &str, args: &Value) -> AppResult<Value> {
    match tool {
        "demo:check_anomalies" => {
            let n = args.get("anomalies").and_then(|v| v.as_i64()).unwrap_or(0);
            Ok(json!({"synthetic": true, "anomalies": n, "note": "演示数据（非真实店铺）"}))
        }
        _ => Err(AppError::McpUnavailable(format!("无此内置工具：{}", tool))),
    }
}

/// 解析占位符：`{{arg:name}}` 从 args 取，`{{cred:ref_id}}` 从 keystore 取（仅内存）。
pub fn resolve_placeholders(input: &str, args: &Value, ks: &dyn KeyStore) -> AppResult<String> {
    let mut out = input.to_string();
    // cred 先替换（值不回显，直接进请求）。
    while let Some(start) = out.find("{{cred:") {
        let end = out[start..]
            .find("}}")
            .map(|i| start + i + 2)
            .ok_or_else(|| AppError::InvalidInput("占位符未闭合：{{cred:".into()))?;
        let ref_id = out[start + 7..end - 2].trim();
        let val = ks
            .get(ref_id)?
            .ok_or_else(|| AppError::NotFound(format!("密钥引用不存在：{}", ref_id)))?;
        out.replace_range(start..end, &val);
    }
    // arg 后替换。
    while let Some(start) = out.find("{{arg:") {
        let end = out[start..]
            .find("}}")
            .map(|i| start + i + 2)
            .ok_or_else(|| AppError::InvalidInput("占位符未闭合：{{arg:".into()))?;
        let name = out[start + 6..end - 2].trim();
        let val = args.get(name).and_then(|v| v.as_str()).unwrap_or("");
        out.replace_range(start..end, val);
    }
    Ok(out)
}

/// 递归解析 Value 内的所有字符串占位符。
fn resolve_value(v: &Value, args: &Value, ks: &dyn KeyStore) -> AppResult<Value> {
    match v {
        Value::String(s) => Ok(Value::String(resolve_placeholders(s, args, ks)?)),
        Value::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, val) in m {
                out.insert(k.clone(), resolve_value(val, args, ks)?);
            }
            Ok(Value::Object(out))
        }
        Value::Array(a) => {
            let mut out = Vec::with_capacity(a.len());
            for val in a {
                out.push(resolve_value(val, args, ks)?);
            }
            Ok(Value::Array(out))
        }
        other => Ok(other.clone()),
    }
}

/// MCP 工具调用：scope = {kind:"mcp", server:"<id>", tool:"<name>"}。
/// 1) 按 server id 取配置（mcp_servers.json）→ 2) 建 transport → 3) initialize 握手
/// → 4) tools/call(server 侧工具名, args) → 5) 返回 content 结果。
/// args 里的 {{cred:ref}} 占位符先经 keystore 解析（值只进请求、不进日志）。
fn run_mcp(args: &Value, scope: &Value, ks: &dyn KeyStore) -> AppResult<Value> {
    let server_id = scope.get("server").and_then(|v| v.as_str())
        .ok_or_else(|| AppError::InvalidInput("MCP 工具缺 server 配置".into()))?;
    let tool_name = scope.get("tool").and_then(|v| v.as_str())
        .ok_or_else(|| AppError::InvalidInput("MCP 工具缺 tool 配置".into()))?;
    let servers = app_config::mcp_list()?;
    let server = servers.into_iter().find(|s| s.id == server_id)
        .ok_or_else(|| AppError::McpUnavailable(format!("MCP server 未配置：{}", server_id)))?;

    // 解析 args 里的占位符（密钥只从 keystore 取、不回显）。
    let safe_args = resolve_value(args, args, ks)?;

    let result = if server.transport == "http" {
        if server.url.is_empty() {
            return Err(AppError::McpUnavailable("MCP http server 缺 url".into()));
        }
        let mut client = McpClient::new(HttpTransport::new(&server.url));
        client.initialize()?;
        client.call_tool(tool_name, &safe_args)?
    } else {
        // 默认 stdio：command + args 启动子进程。
        let mut client = McpClient::new(StdioTransport::spawn(&server.command, &server.args)?);
        client.initialize()?;
        client.call_tool(tool_name, &safe_args)?
    };

    // tools/call 结果是 {content:[{type:"text",text:"..."}], isError?:bool}；拍平成可读文本。
    if result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false) {
        let msg = result.get("content").and_then(|v| v.as_array())
            .and_then(|a| a.first()).and_then(|c| c.get("text")).and_then(|t| t.as_str())
            .unwrap_or("MCP 工具返回错误");
        return Err(AppError::McpUnavailable(format!("MCP 工具错误：{}", msg)));
    }
    let text = result.get("content").and_then(|v| v.as_array())
        .and_then(|a| a.first()).and_then(|c| c.get("text")).and_then(|t| t.as_str());
    match text {
        Some(t) => Ok(serde_json::from_str(t).unwrap_or_else(|_| json!({"text": t}))),
        None => Ok(result),
    }
}

fn run_http(args: &Value, scope: &Value, ks: &dyn KeyStore) -> AppResult<Value> {
    let method = scope.get("method").and_then(|v| v.as_str()).unwrap_or("GET").to_uppercase();
    let url_tpl = scope
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::InvalidInput("HTTP 工具缺 url 配置".into()))?;
    let url = resolve_placeholders(url_tpl, args, ks)?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(AppError::InvalidInput("url 必须是 http/https".into()));
    }
    let timeout_s = scope.get("timeout_s").and_then(|v| v.as_u64()).unwrap_or(15).min(120);

    let mut req = ureq::request(&method, &url).timeout(Duration::from_secs(timeout_s));

    // headers：占位符可能引用 keystore。
    if let Some(headers) = scope.get("headers").and_then(|v| v.as_object()) {
        for (k, v) in headers {
            let raw = v.as_str().unwrap_or("");
            let val = resolve_placeholders(raw, args, ks)?;
            req = req.set(k, &val);
        }
    }
    // query：字符串化后作为查询参数。
    if let Some(query) = scope.get("query").and_then(|v| v.as_object()) {
        for (k, v) in query {
            let raw = match v.as_str() {
                Some(s) => s.to_string(),
                None => v.to_string(),
            };
            let val = resolve_placeholders(&raw, args, ks)?;
            req = req.query(k, &val);
        }
    }

    let resp = if method == "GET" || method == "HEAD" {
        req.call()
    } else {
        let body = scope
            .get("body")
            .map(|b| resolve_value(b, args, ks))
            .transpose()?
            .unwrap_or_else(|| args.clone());
        req = req.set("Content-Type", "application/json");
        req.send_string(&body.to_string())
    };

    let response = resp.map_err(|e| AppError::McpUnavailable(format!("HTTP 请求失败：{}", e)))?;
    let status = response.status();
    let text = response
        .into_string()
        .map_err(|e| AppError::McpUnavailable(format!("读取响应失败：{}", e)))?;

    // 优先当 JSON 解析；非 JSON 则包一层文本。
    let value = match serde_json::from_str::<Value>(&text) {
        Ok(v) => v,
        Err(_) => json!({"text": text, "truncated": text.len() > 2000}),
    };
    Ok(json!({"status": status, "body": value}))
}

/// http-sse 工具：把「发起异步任务 + 立刻连流拿结果」合成一次工具调用。
/// 用于 CodeBuddy `--serve` 这类 HTTP API：POST /runs 拿 runId → 立刻 GET
/// {url}/{runId}/stream 逐行读 SSE → 取 status=completed 的 content.markdown。
///
/// 口径（2026-11-23 拍板）：
/// - stream_url 从 url 自动派生，工具只填一个 url；
/// - headers/body 的 {{arg}}/{{cred}} 占位符经 resolve_value 解析，密钥只进请求、不进日志；
/// - 任务瞬间完成导致 stream 报 RUN_NOT_FOUND：直接报错，不写假结果（不加结果缓存回退）。
///
/// 负向一律返回 AppError，绝不返回合成/成功的假结果。
fn run_http_sse(args: &Value, scope: &Value, ks: &dyn KeyStore) -> AppResult<Value> {
    use std::io::{BufRead, BufReader};

    let url_tpl = scope
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::InvalidInput("http-sse 工具缺 url 配置".into()))?;
    let url = resolve_placeholders(url_tpl, args, ks)?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(AppError::InvalidInput("url 必须是 http/https".into()));
    }
    let timeout_s = scope.get("timeout_s").and_then(|v| v.as_u64()).unwrap_or(120).min(300);
    let timeout = Duration::from_secs(timeout_s);

    // 所有请求需带的头（如 x-codebuddy-request: 1，或 Authorization={{cred:...}}）。
    // 占位符在这里解析，值只进请求、不进任何日志/DB。
    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(h) = scope.get("headers").and_then(|v| v.as_object()) {
        for (k, v) in h {
            let raw = v.as_str().unwrap_or("");
            headers.push((k.clone(), resolve_placeholders(raw, args, ks)?));
        }
    }

    // 1) 发起任务：POST url，body 由配置 + 占位符决定（缺省用 args）。
    let body = scope
        .get("body")
        .map(|b| resolve_value(b, args, ks))
        .transpose()?
        .unwrap_or_else(|| args.clone());
    let mut req = ureq::post(&url).timeout(timeout);
    for (k, v) in &headers {
        req = req.set(k, v);
    }
    req = req.set("Content-Type", "application/json");
    let resp = req
        .send_string(&body.to_string())
        .map_err(|e| AppError::McpUnavailable(format!("http-sse 发起任务失败：{}", e)))?;
    let post_status = resp.status();
    let post_text = resp
        .into_string()
        .map_err(|e| AppError::McpUnavailable(format!("http-sse 读发起响应失败：{}", e)))?;
    if post_status < 200 || post_status >= 300 {
        return Err(AppError::McpUnavailable(format!(
            "http-sse 发起任务 HTTP {}：{}",
            post_status,
            post_text.chars().take(200).collect::<String>()
        )));
    }
    let post_json: Value = serde_json::from_str(&post_text)
        .map_err(|e| AppError::McpUnavailable(format!("http-sse 发起响应非 JSON：{}", e)))?;
    // CodeBuddy 实测形状 {"data":{"runId":"...","status":"accepted"}}；
    // 也兼容顶层 runId，避免绑死一层嵌套。
    let run_id = post_json
        .get("data")
        .and_then(|d| d.get("runId"))
        .or_else(|| post_json.get("runId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::McpUnavailable("http-sse 发起响应缺 runId".into()))?
        .to_string();

    // 2) 立刻连流：stream_url 从 url 派生（去掉尾部 /，拼 /{runId}/stream）。
    let stream_url = format!("{}/{}/stream", url.trim_end_matches('/'), run_id);
    let mut stream_req = ureq::get(&stream_url).timeout(timeout);
    for (k, v) in &headers {
        stream_req = stream_req.set(k, v);
    }
    stream_req = stream_req.set("Accept", "text/event-stream");
    let stream_resp = stream_req
        .call()
        .map_err(|e| AppError::McpUnavailable(format!("http-sse 连 stream 失败：{}", e)))?;
    let stream_status = stream_resp.status();
    if stream_status < 200 || stream_status >= 300 {
        return Err(AppError::McpUnavailable(format!(
            "http-sse stream HTTP {}",
            stream_status
        )));
    }

    // 3) 逐行读 SSE：取最后一个 status=completed 的 content.markdown。
    // 行格式是 `event: xxx` / `data: {...}`；空行分隔事件。
    let reader = BufReader::new(stream_resp.into_reader());
    let mut markdown: Option<String> = None;
    let mut agent: Option<Value> = None;
    for line in reader.lines() {
        let line = line.map_err(|e| AppError::McpUnavailable(format!("http-sse 读流失败：{}", e)))?;
        let line = line.trim_end_matches('\r');
        let data = match line.strip_prefix("data:") {
            Some(d) => d.trim_start(),
            None => continue,
        };
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let event: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue, // 非 JSON 的 data 行忽略，不据此编结果
        };
        if event.get("status").and_then(|v| v.as_str()) != Some("completed") {
            continue;
        }
        if let Some(content) = event.get("content") {
            if let Some(md) = content.get("markdown").and_then(|v| v.as_str()) {
                markdown = Some(md.to_string());
            }
        }
        if let Some(a) = event.get("agent") {
            agent = Some(a.clone());
        }
    }

    match markdown {
        Some(md) => Ok(json!({
            "markdown": md,
            "run_id": run_id,
            "agent": agent.unwrap_or_else(|| json!({}))
        })),
        // 流读完没有 completed 结果：直接报错，绝不写合成/成功结果。
        None => Err(AppError::McpUnavailable(
            "http-sse 流结束但未拿到 status=completed 的 markdown 结果".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::keystore::MemoryKeyStore;

    #[test]
    fn resolves_arg_placeholders() {
        let ks = MemoryKeyStore::new();
        let args = json!({"url": "https://agentbi.tech"});
        assert_eq!(
            resolve_placeholders("scan {{arg:url}}", &args, &ks).unwrap(),
            "scan https://agentbi.tech"
        );
    }

    #[test]
    fn resolves_cred_placeholders_from_keystore() {
        let ks = MemoryKeyStore::new();
        ks.set("k1", "secret-value").unwrap();
        let out = resolve_placeholders("Bearer {{cred:k1}}", &json!({}), &ks).unwrap();
        assert_eq!(out, "Bearer secret-value");
    }

    #[test]
    fn missing_cred_is_error_not_panic() {
        let ks = MemoryKeyStore::new();
        assert!(resolve_placeholders("{{cred:nope}}", &json!({}), &ks).is_err());
    }

    #[test]
    fn mcp_kind_calls_fake_server_end_to_end() {
        // 配置一个 stdio fake server，工具 scope 指向它，验证 ToolEngine 的 mcp 分派。
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().join("tests").join("fixtures").join("fake_mcp_stdio.py")
            .to_string_lossy().to_string();
        let id = format!("te-mcp-{}", std::process::id());
        let _ = app_config::mcp_remove(&id); // 清理可能残留
        app_config::mcp_add(app_config::McpServer {
            id: id.clone(), command: "python3".into(), args: vec![fixture],
            note: "tool_engine 测试".into(), transport: "stdio".into(), url: String::new(),
        }).unwrap();

        let ks = MemoryKeyStore::new();
        let engine = ToolEngine::new(std::sync::Arc::new(ks) as std::sync::Arc<dyn KeyStore>);
        let scope = json!({"kind": "mcp", "server": id, "tool": "add"});
        let out = engine.run("mcp:add", &json!({"a": 10, "b": 5}), &scope).unwrap();
        assert_eq!(out["sum"], 15, "ToolEngine 应经 MCP 协议栈拿到真实结果");

        let _ = app_config::mcp_remove(&id); // 清理
    }

    #[test]
    fn builtin_dispatches_without_scope() {
        let ks: Arc<dyn KeyStore> = Arc::new(MemoryKeyStore::new());
        let eng = ToolEngine::new(ks);
        let r = eng.run("demo:check_anomalies", &json!({"anomalies": 2}), &json!({})).unwrap();
        assert_eq!(r["anomalies"], 2);
        assert_eq!(r["synthetic"], true);
    }
}

#[cfg(test)]
mod http_integration_tests {
    use super::*;
    use crate::domain::executor;
    use crate::domain::llm::MockProvider;
    use crate::infra::db;
    use crate::infra::store;
    use crate::models::SopUpsertPayload;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::thread;

    fn fake_server(body: &'static str) -> (String, std::sync::mpsc::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming() {
                if rx.try_recv().is_ok() { break; }
                let mut s = stream.unwrap();
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });
        (format!("http://{}", addr), tx)
    }

    #[test]
    fn http_tool_runs_inside_sop_end_to_end() {
        let (base, _stop) = fake_server("{\"ok\":true,\"count\":3}");
        let c = db::open_in_memory().unwrap();
        let ks: Arc<dyn KeyStore> = Arc::new(crate::domain::keystore::MemoryKeyStore::new());
        let engine = ToolEngine::new(ks.clone());

        // 自定义 HTTP 工具指向本地 fake server。
        let scope = json!({
            "kind":"http","method":"GET","url":format!("{}/summary", base),"timeout_s":5,"plane":"public"
        });
        store::upsert_tool(&c, "custom:http", "http", "GET", "自定义 HTTP", "low",
            &scope.to_string(), true).unwrap();
        for m in [crate::domain::gateway::Mode::Plan, crate::domain::gateway::Mode::Build, crate::domain::gateway::Mode::Review] {
            store::upsert_tool_grant(&c, "custom:http", m, true, &["public".into()]).unwrap();
        }

        let def = json!({
            "version":1,
            "nodes":[
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"mcp_tool","config":{"tool":"custom:http","args":{}}},
                {"id":"n2","type":"acceptance","config":{"criteria":["steps.n1.status == 200"]}},
                {"id":"n3","type":"end","config":{}}
            ],
            "edges":[{"from":"n0","to":"n1"},{"from":"n1","to":"n2"},{"from":"n2","to":"n3"}]
        });
        let p = SopUpsertPayload {
            id: Some("sop-http".into()), name: "http 测试".into(),
            trigger_json: "{}".into(), risk: "low".into(), owner_policy: "agent".into(),
            definition: def, expect_version: None,
        };
        store::upsert_sop(&c, &p).unwrap();
        let run_id = executor::start_run(&c, "sop-http", "build", "manual", &json!({}), &engine, &MockProvider).unwrap();
        let run = store::get_run(&c, &run_id).unwrap();
        assert_eq!(run.status, "done");
        assert_eq!(run.verdict.as_deref(), Some("pass"));
        // 审计日志里 body 是被解析后的 JSON，status=200。
        let steps = store::list_steps(&c, &run_id).unwrap();
        let out: serde_json::Value = serde_json::from_str(steps[1].output_json.as_deref().unwrap()).unwrap();
        assert_eq!(out["status"], 200);
        assert_eq!(out["body"]["count"], 3);
    }

    /// http-sse 的两阶段 fake server：POST /api/v1/runs 返 runId；
    /// GET /api/v1/runs/{runId}/stream 返一段 SSE 流（data 行 + 可选 done）。
    /// 返回 (base_url, 已收请求路径的共享列表, 停止信号)。
    fn fake_sse_server(
        sse_events: &'static str,
        post_body: &'static str,
    ) -> (String, Arc<Mutex<Vec<String>>>, std::sync::mpsc::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming() {
                if rx.try_recv().is_ok() { break; }
                let mut s = stream.unwrap();
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first = req.lines().next().unwrap_or("");
                let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
                seen2.lock().unwrap().push(path.clone());
                let (ctype, body) = if path.contains("/stream") {
                    ("text/event-stream", sse_events)
                } else {
                    ("application/json", post_body)
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    ctype, body.len(), body
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });
        (format!("http://{}", addr), seen, tx)
    }

    #[test]
    fn http_sse_extracts_markdown_and_derives_stream_url() {
        let sse = "event: message\ndata: {\"status\":\"in_progress\",\"content\":{\"markdown\":\"partial\"}}\n\n\
                   event: message\ndata: {\"status\":\"completed\",\"content\":{\"markdown\":\"最终答案 OK\"},\"agent\":{\"sessionId\":\"s1\",\"toolCalls\":[]}}\n\n\
                   event: done\ndata: {}\n\n";
        let (base, seen, _stop) = fake_sse_server(sse, "{\"data\":{\"runId\":\"run-42\",\"status\":\"accepted\"}}");
        let ks: Arc<dyn KeyStore> = Arc::new(crate::domain::keystore::MemoryKeyStore::new());
        let engine = ToolEngine::new(ks);

        let scope = json!({
            "kind": "http-sse",
            "url": format!("{}/api/v1/runs", base),
            "headers": {"x-codebuddy-request": "1"},
            "body": {"id": "{{arg:task_id}}", "type": "message", "payload": {"text": "{{arg:prompt}}"}},
            "timeout_s": 5
        });
        let out = engine.run("custom:codebuddy", &json!({"task_id": "t-1", "prompt": "只回复 OK"}), &scope).unwrap();

        assert_eq!(out["markdown"], "最终答案 OK");
        assert_eq!(out["run_id"], "run-42");
        assert_eq!(out["agent"]["sessionId"], "s1");
        // 两阶段请求都发生，且 stream 路径由 POST url + runId 自动派生。
        let paths = seen.lock().unwrap().clone();
        assert_eq!(paths, vec!["/api/v1/runs".to_string(), "/api/v1/runs/run-42/stream".to_string()]);
    }

    #[test]
    fn http_sse_without_completed_event_is_error_not_fake_result() {
        // 只有 in_progress、没有 completed：必须报错，绝不写合成结果。
        let sse = "event: message\ndata: {\"status\":\"in_progress\",\"content\":{\"markdown\":\"partial\"}}\n\n\
                   event: done\ndata: {}\n\n";
        let (base, _seen, _stop) = fake_sse_server(sse, "{\"data\":{\"runId\":\"run-9\",\"status\":\"accepted\"}}");
        let ks: Arc<dyn KeyStore> = Arc::new(crate::domain::keystore::MemoryKeyStore::new());
        let engine = ToolEngine::new(ks);
        let scope = json!({"kind": "http-sse", "url": format!("{}/api/v1/runs", base), "timeout_s": 5});
        let r = engine.run("custom:codebuddy", &json!({}), &scope);
        assert!(r.is_err(), "无 completed 事件必须返回错误，不能写假结果");
    }

    #[test]
    fn http_sse_bad_post_response_is_error() {
        // POST 响应缺 runId：报错，不尝试连流。
        let (base, seen, _stop) = fake_sse_server("event: done\ndata: {}\n\n", "{\"data\":{}}");
        let ks: Arc<dyn KeyStore> = Arc::new(crate::domain::keystore::MemoryKeyStore::new());
        let engine = ToolEngine::new(ks);
        let scope = json!({"kind": "http-sse", "url": format!("{}/api/v1/runs", base), "timeout_s": 5});
        assert!(engine.run("custom:codebuddy", &json!({}), &scope).is_err());
        assert_eq!(seen.lock().unwrap().len(), 1, "POST 失败不应连 stream");
    }

    #[test]
    fn http_sse_result_shows_in_output_and_pauses_at_human_gate() {
        // 端到端：http-sse 工具经 SOP 的 mcp_tool 节点跑一次 → markdown 写进 step output_json
        // → 后继 human_gate 让 run 进 wait_human（结果展示 + 人工审核链路）。
        let sse = "event: message\ndata: {\"status\":\"completed\",\"content\":{\"markdown\":\"Agent 结论：站点可被读取\"},\"agent\":{\"toolCalls\":[]}}\n\nevent: done\ndata: {}\n\n";
        let (base, _seen, _stop) = fake_sse_server(sse, "{\"data\":{\"runId\":\"run-sop\",\"status\":\"accepted\"}}");
        let c = db::open_in_memory().unwrap();
        let ks: Arc<dyn KeyStore> = Arc::new(crate::domain::keystore::MemoryKeyStore::new());
        let engine = ToolEngine::new(ks.clone());

        let scope = json!({
            "kind":"http-sse","url":format!("{}/api/v1/runs", base),
            "headers":{"x-codebuddy-request":"1"},
            "body":{"id":"{{arg:task_id}}","type":"message","payload":{"text":"{{arg:prompt}}"}},
            "timeout_s":5,"plane":"public"
        });
        store::upsert_tool(&c, "custom:codebuddy-sse", "http", "POST", "CodeBuddy 任务", "low",
            &scope.to_string(), true).unwrap();
        for m in [crate::domain::gateway::Mode::Plan, crate::domain::gateway::Mode::Build, crate::domain::gateway::Mode::Review] {
            store::upsert_tool_grant(&c, "custom:codebuddy-sse", m, true, &["public".into()]).unwrap();
        }

        let def = json!({
            "version":1,
            "nodes":[
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"mcp_tool","config":{"tool":"custom:codebuddy-sse","args":{"task_id":"t-1","prompt":"分析站点"}}},
                {"id":"n2","type":"human_gate","config":{"prompt":"请审核 CodeBuddy 的执行结果"}},
                {"id":"n3","type":"end","config":{}}
            ],
            "edges":[{"from":"n0","to":"n1"},{"from":"n1","to":"n2"},{"from":"n2","to":"n3"}]
        });
        let p = SopUpsertPayload {
            id: Some("sop-sse".into()), name: "sse 测试".into(),
            trigger_json: "{}".into(), risk: "low".into(), owner_policy: "agent".into(),
            definition: def, expect_version: None,
        };
        store::upsert_sop(&c, &p).unwrap();
        let run_id = executor::start_run(&c, "sop-sse", "build", "manual", &json!({}), &engine, &MockProvider).unwrap();

        // run 应在 human_gate 处暂停，等待人工审核。
        let run = store::get_run(&c, &run_id).unwrap();
        assert_eq!(run.status, "wait_human", "后继 human_gate 应让 run 暂停等人工审核");
        // mcp_tool 步骤的 output_json 含 markdown 结果，供界面展示。
        let steps = store::list_steps(&c, &run_id).unwrap();
        let out: Value = serde_json::from_str(steps[1].output_json.as_deref().unwrap()).unwrap();
        assert_eq!(out["markdown"], "Agent 结论：站点可被读取", "markdown 结果应写入 step 输出供界面展示");
    }

    #[test]
    fn http_sse_cred_placeholder_resolves_from_keystore() {
        // headers 里的 {{cred:ref}} 只进请求、不进日志；缺失则报错。
        let sse = "event: message\ndata: {\"status\":\"completed\",\"content\":{\"markdown\":\"authed\"}}\n\n";
        let (base, _seen, _stop) = fake_sse_server(sse, "{\"data\":{\"runId\":\"run-c\",\"status\":\"accepted\"}}");
        let ks = crate::domain::keystore::MemoryKeyStore::new();
        ks.set("cb_token", "user-secret-token").unwrap();
        let engine = ToolEngine::new(Arc::new(ks) as Arc<dyn KeyStore>);
        let scope = json!({
            "kind": "http-sse", "url": format!("{}/api/v1/runs", base),
            "headers": {"Authorization": "Bearer {{cred:cb_token}}"},
            "timeout_s": 5
        });
        let out = engine.run("custom:codebuddy", &json!({}), &scope).unwrap();
        assert_eq!(out["markdown"], "authed");
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::domain::examples;
    use crate::domain::keystore::MemoryKeyStore;

    /// 手动跑：实调 agentbi.tech 示例工具，确认通用 HTTP 引擎真能维护「自建电商网」。
    /// `cargo test live_agentbi -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_agentbi_example_tools() {
        let ks: Arc<dyn KeyStore> = Arc::new(MemoryKeyStore::new());
        let eng = ToolEngine::new(ks);
        for t in examples::agentbi_example_tools() {
            let scope = serde_json::to_value(&t.scope).unwrap();
            let args = if t.id.ends_with(":check") {
                json!({"url": "https://agentbi.tech"})
            } else {
                json!({})
            };
            match eng.run(&t.id, &args, &scope) {
                Ok(v) => println!("OK {} -> status={} body_keys={:?}", t.id, v["status"],
                    v["body"].as_object().map(|m| m.keys().cloned().collect::<Vec<_>>())),
                Err(e) => println!("FAIL {} -> {}", t.id, e),
            }
        }
    }
}
