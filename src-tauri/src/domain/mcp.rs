//! MCP client 协议栈（实现设计 §3.2、系统 §5.3）：JSON-RPC 2.0 + 两种传输。
//!
//! 同步实现（ToolRunner::run 是同步的、ureq 是同步 HTTP）；不引入 tokio。
//! - StdioTransport：子进程 command+args，stdin/stdout 换行分隔 JSON-RPC（stderr 不混入）。
//! - HttpTransport：ureq POST 到 url，响应可能是 JSON 或 SSE（text/event-stream）。
//! McpClient：initialize 握手 → notifications/initialized → tools/list → tools/call。
//!
//! 外围（gateway 授权 / keystore / 审计 redact）已在别处就绪，本模块只做协议与传输。

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

use crate::models::{AppError, AppResult};

const PROTOCOL_VERSION: &str = "2025-06-18";

// ---- JSON-RPC 2.0 ----

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

/// 从响应里取 result；JSON-RPC error 或缺 result 都报错（不静默）。
fn result_of(resp: &Value) -> AppResult<Value> {
    if let Some(err) = resp.get("error") {
        if !err.is_null() {
            return Err(AppError::McpUnavailable(format!(
                "MCP error：{}",
                err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown")
            )));
        }
    }
    resp.get("result")
        .cloned()
        .ok_or_else(|| AppError::McpUnavailable("MCP 响应缺 result".into()))
}

/// 把响应文本解析成 JSON-RPC 响应。支持纯 JSON 与 SSE（取第一个 data: 行的 JSON）。
fn parse_response(text: &str) -> AppResult<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Ok(v);
    }
    // SSE：逐行找 "data: {...}"，取第一条能解析为带 id/result/error 的 JSON。
    for line in t.lines() {
        let line = line.trim();
        let payload = line.strip_prefix("data:").or_else(|| line.strip_prefix("data: ")).unwrap_or(line);
        if let Ok(v) = serde_json::from_str::<Value>(payload.trim()) {
            if v.get("jsonrpc").is_some() || v.get("id").is_some() || v.get("result").is_some() || v.get("error").is_some() {
                return Ok(v);
            }
        }
    }
    Err(AppError::McpUnavailable(format!("MCP 响应既非 JSON 也非有效 SSE：{}", &t[..t.len().min(120)])))
}

// ---- 传输 trait ----

pub trait McpTransport: Send {
    /// 发一条 JSON-RPC 请求并返回响应；notification（无 id）返回 Ok(Null)。
    fn send(&mut self, msg: &Value, is_notification: bool) -> AppResult<Value>;
}

// ---- stdio 传输 ----

pub struct StdioTransport {
    child: Child,
}

impl StdioTransport {
    pub fn spawn(command: &str, args: &[String]) -> AppResult<Self> {
        Self::spawn_with_env(command, args, &[])
    }

    /// 带环境变量透传启动子进程。`env` 是 (变量名, 值) 列表，值来自进程环境
    /// （由 secrets.env 或 shell 提供），不写进配置文件、不进日志。
    pub fn spawn_with_env(command: &str, args: &[String], env: &[(String, String)]) -> AppResult<Self> {
        let mut child = Command::new(command)
            .args(args)
            .envs(env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()) // stderr 不混入 stdout，避免污染 JSON-RPC
            .spawn()
            .map_err(|e| AppError::McpUnavailable(format!("启动 MCP server 失败：{}", e)))?;
        // 确认子进程没立刻退出。
        if let Some(status) = child.try_wait().map_err(|e| AppError::McpUnavailable(e.to_string()))? {
            return Err(AppError::McpUnavailable(format!("MCP server 启动即退出：{}", status)));
        }
        Ok(Self { child })
    }
}

impl McpTransport for StdioTransport {
    fn send(&mut self, msg: &Value, is_notification: bool) -> AppResult<Value> {
        let mut stdin = self.child.stdin.take()
            .ok_or_else(|| AppError::McpUnavailable("MCP stdin 不可用".into()))?;
        writeln!(stdin, "{}", msg)
            .map_err(|e| AppError::McpUnavailable(format!("写 MCP stdin 失败：{}", e)))?;
        stdin.flush().map_err(|e| AppError::McpUnavailable(e.to_string()))?;
        self.child.stdin = Some(stdin);

        if is_notification {
            return Ok(Value::Null);
        }

        // 逐行读 stdout，跳过非 JSON 行（如 server 的日志直接写到了 stdout），直到拿到带 id 的响应。
        let mut stdout = self.child.stdout.take()
            .ok_or_else(|| AppError::McpUnavailable("MCP stdout 不可用".into()))?;
        let mut reader = BufReader::new(&mut stdout);
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line)
                .map_err(|e| AppError::McpUnavailable(format!("读 MCP stdout 失败：{}", e)))?;
            if n == 0 {
                self.child.stdout = Some(stdout);
                return Err(AppError::McpUnavailable("MCP server 关闭了 stdout".into()));
            }
            let t = line.trim();
            if t.is_empty() { continue; }
            if let Ok(v) = serde_json::from_str::<Value>(t) {
                if v.get("jsonrpc").is_some() && (v.get("result").is_some() || v.get("error").is_some()) {
                    self.child.stdout = Some(stdout);
                    return Ok(v);
                }
            }
        }
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---- 进程内传输（内置 server：filesystem / shell）----

/// 在 Rust 进程内处理 JSON-RPC，不走子进程。用于内置 MCP server（domain/builtin_mcp.rs）。
/// 走同一个 McpClient 握手/发现/调用流程，只是 send 直接分派到内置实现。
pub struct InProcessTransport {
    server_id: String,
    app_data_dir: std::path::PathBuf,
}

impl InProcessTransport {
    pub fn new(server_id: &str, app_data_dir: std::path::PathBuf) -> Self {
        Self { server_id: server_id.to_string(), app_data_dir }
    }
}

impl McpTransport for InProcessTransport {
    fn send(&mut self, msg: &Value, is_notification: bool) -> AppResult<Value> {
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        // notification（如 initialized）无需响应。
        if is_notification { return Ok(Value::Null); }
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let result = match method {
            "initialize" => json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": self.server_id, "version": "0.1.0" }
            }),
            "tools/list" => crate::domain::builtin_mcp::tools_list(&self.server_id)?,
            "tools/call" => {
                let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                crate::domain::builtin_mcp::tools_call(&self.server_id, name, &args, &self.app_data_dir)?
            }
            other => return Err(AppError::McpUnavailable(format!("内置 server 不支持方法：{}", other))),
        };
        Ok(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }
}

// ---- HTTP 传输 ----

pub struct HttpTransport {
    url: String,
    timeout: Duration,
}

impl HttpTransport {
    pub fn new(url: &str) -> Self {
        Self { url: url.trim_end_matches('/').to_string(), timeout: Duration::from_secs(60) }
    }
}

impl McpTransport for HttpTransport {
    fn send(&mut self, msg: &Value, is_notification: bool) -> AppResult<Value> {
        let resp = ureq::post(&self.url)
            .timeout(self.timeout)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream")
            .send_string(&msg.to_string())
            .map_err(|e| AppError::McpUnavailable(format!("MCP HTTP 请求失败：{}", e)))?;
        // notification（如 initialized）通常返回 204 空体，无需解析。
        if is_notification {
            return Ok(Value::Null);
        }
        let text = resp.into_string()
            .map_err(|e| AppError::McpUnavailable(format!("读 MCP HTTP 响应失败：{}", e)))?;
        parse_response(&text)
    }
}

// ---- Client ----

pub struct McpClient<T: McpTransport> {
    transport: T,
    next_id: i64,
}

impl<T: McpTransport> McpClient<T> {
    pub fn new(transport: T) -> Self { Self { transport, next_id: 1 } }

    fn call_method(&mut self, method: &str, params: Value) -> AppResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let resp = self.transport.send(&request(id, method, params), false)?;
        result_of(&resp)
    }

    /// 握手：initialize + notifications/initialized。
    pub fn initialize(&mut self) -> AppResult<Value> {
        let result = self.call_method("initialize", json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "abiFlow", "version": "0.1.0"}
        }))?;
        self.transport.send(
            &notification("notifications/initialized", json!({})),
            true,
        )?;
        Ok(result)
    }

    /// 工具发现：tools/list → [{name, description, inputSchema}, ...]。
    pub fn list_tools(&mut self) -> AppResult<Vec<Value>> {
        let result = self.call_method("tools/list", json!({}))?;
        result.get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .ok_or_else(|| AppError::McpUnavailable("tools/list 响应缺 tools 数组".into()))
    }

    /// 调工具：tools/call {name, arguments} → content 结果。
    pub fn call_tool(&mut self, name: &str, args: &Value) -> AppResult<Value> {
        self.call_method("tools/call", json!({"name": name, "arguments": args}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonrpc_message_shape() {
        let r = request(7, "tools/list", json!({}));
        assert_eq!(r["jsonrpc"], "2.0");
        assert_eq!(r["id"], 7);
        assert_eq!(r["method"], "tools/list");
        let n = notification("notifications/initialized", json!({}));
        assert!(n.get("id").is_none(), "notification 不应有 id");
    }

    #[test]
    fn result_of_handles_error_and_missing() {
        let ok = json!({"result": {"tools": []}});
        assert!(result_of(&ok).is_ok());
        let err = json!({"error": {"message": "boom"}});
        assert!(result_of(&err).is_err());
        let none = json!({});
        assert!(result_of(&none).is_err());
    }

    #[test]
    fn parse_response_json_and_sse() {
        let j = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        assert!(parse_response(j).is_ok());
        let sse = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[]}}\n\n";
        let v = parse_response(sse).unwrap();
        assert_eq!(v["result"]["tools"].as_array().unwrap().len(), 0);
    }

    // ---- 端到端：本地 fake MCP server（stdio / http）----

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().join("tests").join("fixtures").join(name)
    }

    #[test]
    fn stdio_passes_env_to_child() {
        // 子进程把透传进来的 env 值回显在 JSON-RPC result 里；不打印、不落盘。
        let script = "import sys,os,json\nfor line in sys.stdin:\n  try: m=json.loads(line)\n  except Exception: continue\n  print(json.dumps({'jsonrpc':'2.0','id':m.get('id',1),'result':os.environ.get('ABI_TEST_ENV','MISSING')}),flush=True)\n";
        let args = vec!["-c".to_string(), script.to_string()];
        let env = vec![("ABI_TEST_ENV".to_string(), "shopify-token-test".to_string())];
        let mut t = StdioTransport::spawn_with_env("python3", &args, &env).unwrap();
        let resp = t.send(&request(1, "ping", json!({})), false).unwrap();
        assert_eq!(resp["result"], "shopify-token-test", "子进程应看到透传的 env 值");
    }

    #[test]
    fn stdio_end_to_end_with_fake_server() {
        // fake server 往 stderr 写噪音；StdioTransport 只从 stdout 读 JSON-RPC。
        let mut client = McpClient::new(
            StdioTransport::spawn("python3", &[fixture("fake_mcp_stdio.py").to_string_lossy().to_string()]).unwrap()
        );
        client.initialize().unwrap();
        let tools = client.list_tools().unwrap();
        assert_eq!(tools.len(), 2, "fake server 应提供 2 个工具");
        let res = client.call_tool("add", &json!({"a": 2, "b": 3})).unwrap();
        let text = res["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["sum"], 5, "tools/call 应返回真实结果（非协议解析占位）");
    }

    fn free_port() -> u16 {
        let s = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        s.local_addr().unwrap().port()
    }

    #[test]
    fn http_end_to_end_with_fake_server_json_and_sse() {
        for sse in [false, true] {
            let port = free_port();
            let mut args = vec![fixture("fake_mcp_http.py").to_string_lossy().to_string(), "--port".into(), port.to_string()];
            if sse { args.push("--sse".into()); }
            let mut child = std::process::Command::new("python3").args(&args).spawn().unwrap();
            std::thread::sleep(Duration::from_millis(400)); // 等 server 起来

            let url = format!("http://127.0.0.1:{}", port);
            let mut client = McpClient::new(HttpTransport::new(&url));
            client.initialize().unwrap();
            let tools = client.list_tools().unwrap();
            assert_eq!(tools.len(), 2, "fake http server 应提供 2 个工具");
            let res = client.call_tool("multiply", &json!({"a": 3, "b": 4})).unwrap();
            let text = res["content"][0]["text"].as_str().unwrap();
            let parsed: Value = serde_json::from_str(text).unwrap();
            assert_eq!(parsed["product"], 12, "SSE={} 下 tools/call 应返回结果", sse);

            let _ = child.kill();
            let _ = child.wait();
        }
    }

    #[test]
    fn in_process_transport_filesystem_roundtrip() {
        let root = std::env::temp_dir().join("abiflow-inproc-fs-test");
        let _ = std::fs::create_dir_all(&root);
        // InProcessTransport 的 app_data_dir 参数是"data_dir"，fs_root = data_dir/abiflow/builtin-fs。
        let data_dir = root.clone();
        let fs_root = crate::domain::builtin_mcp::fs_root(&data_dir);
        let _ = std::fs::create_dir_all(&fs_root);
        std::fs::write(fs_root.join("hello.txt"), "hello-inproc").unwrap();

        let mut client = McpClient::new(InProcessTransport::new(
            crate::domain::builtin_mcp::FS_SERVER, data_dir.clone(),
        ));
        client.initialize().unwrap();
        let tools = client.list_tools().unwrap();
        assert!(tools.iter().any(|t| t["name"] == "read_file"));
        let res = client.call_tool("read_file", &json!({"path":"hello.txt"})).unwrap();
        let text = res["content"][0]["text"].as_str().unwrap();
        assert_eq!(text, "hello-inproc");
        // 路径穿越被拒（InProcessTransport 会把 InvalidInput 作为 JSON-RPC error 抛出）
        assert!(client.call_tool("read_file", &json!({"path":"../secret"})).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
