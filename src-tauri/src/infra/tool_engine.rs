use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::domain::executor::ToolRunner;
use crate::domain::keystore::KeyStore;
use crate::models::{AppError, AppResult};

/// 配置驱动的工具引擎：按 tools.scope_json 的 `kind` 分派。
/// - `builtin`：内置只读 demo（合成数据）。
/// - `http`：通用 HTTP 工具（method/url/query/headers/body 均由配置 + 占位符决定）。
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
    use crate::infra::db;
    use crate::infra::store;
    use crate::models::SopUpsertPayload;
    use std::io::{Read, Write};
    use std::net::TcpListener;
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
        let run_id = executor::start_run(&c, "sop-http", "build", "manual", &json!({}), &engine).unwrap();
        let run = store::get_run(&c, &run_id).unwrap();
        assert_eq!(run.status, "done");
        assert_eq!(run.verdict.as_deref(), Some("pass"));
        // 审计日志里 body 是被解析后的 JSON，status=200。
        let steps = store::list_steps(&c, &run_id).unwrap();
        let out: serde_json::Value = serde_json::from_str(steps[1].output_json.as_deref().unwrap()).unwrap();
        assert_eq!(out["status"], 200);
        assert_eq!(out["body"]["count"], 3);
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::domain::examples;
    use crate::domain::keystore::MemoryKeyStore;
    use crate::infra::store;

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
