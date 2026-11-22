use serde_json::{json, Value};

use crate::models::{AppError, AppResult};

/// LLM Provider 抽象（设计 §7：Provider 可插拔，云 API / 本地 Ollama 不改核心）。
/// 框架期本机未装 Ollama，默认 Mock；装好模型后只换实现，调用方不动。
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn is_mock(&self) -> bool;
    fn generate(&self, prompt: &str) -> AppResult<String>;
}

/// Mock provider：确定性、离线、输出明确标注 mock。
/// - prompt 含 `SOP_DRAFT:` 时返回一份**合法 SOP AST 草稿**（供 sop_parse_text 用）
/// - 否则返回一段标注 mock 的推理摘要（供 agent_step 用）
pub struct MockProvider;

impl LlmProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }
    fn is_mock(&self) -> bool {
        true
    }
    fn generate(&self, prompt: &str) -> AppResult<String> {
        if prompt.contains("SOP_DRAFT:") {
            let draft = json!({
                "version": 1,
                "nodes": [
                    {"id":"n0","type":"start","config":{}},
                    {"id":"n1","type":"mcp_tool","config":{"tool":"PLACEHOLDER_TOOL","args":{}}},
                    {"id":"n2","type":"human_gate","config":{"reason":"review","prompt":"LLM 生成的草稿，请确认后再启用"}},
                    {"id":"n3","type":"acceptance","config":{"criteria":["steps.n1.status == 200"]}},
                    {"id":"n4","type":"end","config":{}}
                ],
                "edges": [
                    {"from":"n0","to":"n1"},
                    {"from":"n1","to":"n2"},
                    {"from":"n2","to":"n3"},
                    {"from":"n3","to":"n4"}
                ],
                "_mock": true,
                "_hint": "Mock 草稿：把 PLACEHOLDER_TOOL 换成已注册工具，并按需求增删节点"
            });
            return Ok(draft.to_string());
        }
        Ok(json!({
            "mock": true,
            "provider": "mock",
            "summary": "（Mock 推理，未接真实模型）基于上游工具结果，未发现需在本次自动处理的资金/不可逆动作；异常项建议转人工确认。",
            "prompt_chars": prompt.chars().count()
        })
        .to_string())
    }
}

/// Ollama provider：HTTP 调本地 `/api/generate`（localhost:11434）。
/// 本机当前未装 Ollama/模型，留接口；装好后把配置 provider 切到 ollama 即可，核心不动。
pub struct OllamaProvider {
    pub host: String,
    pub model: String,
}

impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }
    fn is_mock(&self) -> bool {
        false
    }
    fn generate(&self, prompt: &str) -> AppResult<String> {
        let url = format!(
            "{}/api/generate",
            self.host.trim_end_matches('/')
        );
        let body = json!({
            "model": self.model,
            "prompt": prompt,
            "stream": false
        });
        let resp = ureq::post(&url)
            .timeout(std::time::Duration::from_secs(120))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(|e| AppError::LlmError(format!("Ollama 请求失败：{}", e)))?;
        let text = resp
            .into_string()
            .map_err(|e| AppError::LlmError(format!("读取 Ollama 响应失败：{}", e)))?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| AppError::LlmError(format!("Ollama 响应非法：{}", e)))?;
        v.get("response")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| AppError::LlmError("Ollama 响应缺 response 字段".into()))
    }
}

/// OpenAI 兼容 provider：POST {host}/chat/completions，Bearer key。
/// host 是 base URL（如 https://open.bigmodel.cn/api/paas/v4），不含 /chat/completions。
/// 适配智谱 GLM / DeepSeek / 通义等任何 OpenAI Chat Completions 端点；key 只进 header，
/// 错误文案与日志都不包含它。api_key 为空时不选此 provider（回落 Mock，安全默认）。
pub struct OpenAiCompatibleProvider {
    pub host: String,
    pub model: String,
    pub api_key: String,
}

impl LlmProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        "openai-compatible"
    }
    fn is_mock(&self) -> bool {
        false
    }
    fn generate(&self, prompt: &str) -> AppResult<String> {
        let url = format!("{}/chat/completions", self.host.trim_end_matches('/'));
        let body = json!({
            "model": self.model,
            "messages": [{ "role": "user", "content": prompt }],
            "stream": false
        });
        let bearer = format!("Bearer {}", self.api_key);
        let resp = ureq::post(&url)
            .timeout(std::time::Duration::from_secs(120))
            .set("Content-Type", "application/json")
            .set("Authorization", &bearer)
            .send_string(&body.to_string())
            .map_err(|e| AppError::LlmError(format!("OpenAI 兼容端点请求失败：{}", e)))?;
        let text = resp
            .into_string()
            .map_err(|e| AppError::LlmError(format!("读取端点响应失败：{}", e)))?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| AppError::LlmError(format!("端点响应非法：{}", e)))?;
        v.get("choices")
            .and_then(|a| a.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| AppError::LlmError("端点响应缺 choices[0].message.content".into()))
    }
}

/// 从配置构造 provider：
/// - provider=openai-compatible 且 host/model/api_key 齐全 → OpenAI 兼容
/// - provider=ollama 且 host/model 齐全 → Ollama
/// - 其余 → Mock（安全默认，无 key 不误连）
pub fn from_config(provider: &str, host: &str, model: &str, api_key: &str) -> std::sync::Arc<dyn LlmProvider> {
    if provider.eq_ignore_ascii_case("openai-compatible")
        && !host.is_empty() && !model.is_empty() && !api_key.is_empty()
    {
        std::sync::Arc::new(OpenAiCompatibleProvider {
            host: host.to_string(),
            model: model.to_string(),
            api_key: api_key.to_string(),
        })
    } else if provider.eq_ignore_ascii_case("ollama") && !host.is_empty() && !model.is_empty() {
        std::sync::Arc::new(OllamaProvider {
            host: host.to_string(),
            model: model.to_string(),
        })
    } else {
        std::sync::Arc::new(MockProvider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_is_labeled_and_offline() {
        let m = MockProvider;
        assert!(m.is_mock());
        let out = m.generate("总结异常").unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mock"], true);
        assert_eq!(v["provider"], "mock");
    }

    #[test]
    fn mock_returns_valid_sop_draft() {
        let m = MockProvider;
        let out = m.generate("SOP_DRAFT: 每日检查异常订单").unwrap();
        let draft: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(draft["version"], 1);
        assert!(draft["nodes"].as_array().unwrap().len() >= 3);
        assert!(draft["edges"].as_array().unwrap().len() >= 2);
        assert_eq!(draft["_mock"], true);
    }

    #[test]
    fn config_selects_provider() {
        assert!(from_config("mock", "", "", "").is_mock());
        assert!(from_config("", "http://x", "m", "k").is_mock());
        let o = from_config("ollama", "http://127.0.0.1:11434", "qwen2.5:3b", "");
        assert!(!o.is_mock());
        assert_eq!(o.name(), "ollama");
    }

    #[test]
    fn openai_compatible_needs_key_else_mock() {
        // 缺 key 不误连，回落 Mock（安全默认）
        assert!(from_config("openai-compatible", "https://open.bigmodel.cn/api/paas/v4", "glm-4.7-flash", "").is_mock());
        let p = from_config("openai-compatible", "https://open.bigmodel.cn/api/paas/v4", "glm-4.7-flash", "fake-key");
        assert!(!p.is_mock());
        assert_eq!(p.name(), "openai-compatible");
    }

    // 假 HTTP server 端到端：验证请求形状（路径/Authorization/messages）与响应解析，
    // 不真调 bigmodel.cn，也不需要真 key。
    #[test]
    fn openai_compatible_request_and_parse() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = r#"{"choices":[{"message":{"role":"assistant","content":"OK-FROM-FAKE"}}]}"#;
            write!(sock,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body).unwrap();
            sock.flush().unwrap();
            req
        });
        let p = OpenAiCompatibleProvider {
            host: format!("http://127.0.0.1:{}", port),
            model: "glm-4.7-flash".into(),
            api_key: "fake-key-123".into(),
        };
        let out = p.generate("TEST-PROMPT").unwrap();
        assert_eq!(out, "OK-FROM-FAKE");
        let req = srv.join().unwrap();
        assert!(req.contains("POST /chat/completions"), "path wrong: {}", req);
        assert!(req.contains("Authorization: Bearer fake-key-123"), "auth header missing");
        assert!(req.contains("\"model\":\"glm-4.7-flash\""), "model missing");
        assert!(req.contains("TEST-PROMPT"), "prompt missing");
        assert!(req.contains("\"messages\""), "messages missing");
    }
}
