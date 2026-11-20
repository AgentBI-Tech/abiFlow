use std::collections::HashMap;

use serde_json::Value;

use crate::domain::audit::redact_value;
use crate::domain::llm::LlmProvider;
use crate::domain::sop::eval_condition;
use crate::models::AppResult;

/// 评审者的核对结论：passed + 由谁核对 + 说明（对齐 acceptance.checked_by 的 rule/llm/human）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewVerdict {
    pub passed: bool,
    pub checked_by: &'static str, // "rule" | "llm"
    pub note: String,
}

/// 判断一条验收标准是否强制走 LLM：对象形式 `{"criterion":"...","mode":"llm"}`。
fn forced_llm(c: &Value) -> Option<&str> {
    let obj = c.as_object()?;
    let mode = obj.get("mode")?.as_str()?;
    if mode != "llm" {
        return None;
    }
    obj.get("criterion")?.as_str()
}

fn criterion_text(c: &Value) -> Option<&str> {
    if let Some(s) = c.as_str() {
        return Some(s);
    }
    c.as_object()?.get("criterion")?.as_str()
}

/// 从 LLM 回复里解析布尔结论：JSON {"passed":true,...} 或首行 true/false。
fn parse_verdict(text: &str) -> Option<bool> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        if let Some(b) = v.get("passed").and_then(|x| x.as_bool()) {
            return Some(b);
        }
        if let Some(b) = v.as_bool() {
            return Some(b);
        }
    }
    match t.to_ascii_lowercase().as_str() {
        "true" | "pass" | "passed" => Some(true),
        "false" | "fail" | "failed" => Some(false),
        _ => None,
    }
}

/// 评审一条验收标准：
/// - 字符串 / 未指定 mode：先按规则表达式求值（rule）；求不出来（模糊标准）才交 LLM。
/// - 对象且 mode=llm：强制 LLM 核实。
///
/// LLM 通道只喂脱敏后的上游输出，不编数、不下合规判定；结论标注 provider/mock。
pub fn review(
    criterion: &Value,
    outputs: &HashMap<String, Value>,
    llm: &dyn LlmProvider,
) -> AppResult<ReviewVerdict> {
    let text = criterion_text(criterion).unwrap_or("").trim();

    // 1) 规则通道：非强制 LLM 时先试表达式求值。
    if forced_llm(criterion).is_none() && !text.is_empty() {
        if let Ok(passed) = eval_condition(text, outputs) {
            return Ok(ReviewVerdict {
                passed,
                checked_by: "rule",
                note: String::new(),
            });
        }
    }

    // 2) LLM 通道：模糊标准或显式 mode=llm。
    if text.is_empty() {
        return Ok(ReviewVerdict {
            passed: false,
            checked_by: "llm",
            note: "验收标准为空，无法核实".into(),
        });
    }
    let safe = redact_value(&Value::Object(outputs.iter().map(|(k, v)| (k.clone(), v.clone())).collect()));
    let prompt = format!(
        "你是电商运营的验收评审者。请只根据下面的上游步骤输出（JSON，已脱敏）核对该标准是否成立。\n\
         验收标准：{}\n上游输出：{}\n\
         约束：只陈述本地事实，不编造金额、不下合规判定、不跨店比绝对值；证据不足时判 false。\n\
         只用简短 JSON 回答 {{\"passed\": true/false, \"note\": \"...\"}}。",
        text,
        serde_json::to_string(&safe).unwrap_or_else(|_| "{}".into())
    );
    let raw = llm.generate(&prompt)?;
    let parsed = parse_verdict(&raw);
    let mut note = String::new();
    if let Ok(v) = serde_json::from_str::<Value>(&raw) {
        if let Some(n) = v.get("note").and_then(|x| x.as_str()) {
            note.push_str(n);
        }
    }
    if llm.is_mock() {
        if !note.is_empty() { note.push_str(" · "); }
        note.push_str("mock 推理（未接真实模型）");
    }
    Ok(ReviewVerdict {
        passed: parsed.unwrap_or(false),
        checked_by: "llm",
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::domain::llm::MockProvider;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn outputs() -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert(
            "n1".into(),
            json!({"status": 200, "anomalies": 3, "email": "a@b.com"}),
        );
        m
    }

    #[test]
    fn rule_criterion_evaluated_by_rule() {
        let c = json!("steps.n1.status == 200");
        // eval_condition 去掉 steps. 前缀后，按 step id 作顶层 key 取输出。
        let mut out = HashMap::new();
        out.insert("n1".into(), json!({"status": 200}));
        let v = review(&c, &out, &MockProvider).unwrap();
        assert_eq!(v.checked_by, "rule");
        assert!(v.passed);
    }

    #[test]
    fn rule_criterion_failure() {
        let c = json!("steps.n1.status == 500");
        let mut out = HashMap::new();
        out.insert("n1".into(), json!({"status": 200}));
        let v = review(&c, &out, &MockProvider).unwrap();
        assert_eq!(v.checked_by, "rule");
        assert!(!v.passed);
    }

    struct FixedProvider {
        answer: String,
        calls: Arc<AtomicUsize>,
        mock: bool,
    }
    impl LlmProvider for FixedProvider {
        fn name(&self) -> &str { "fixed" }
        fn is_mock(&self) -> bool { self.mock }
        fn generate(&self, _prompt: &str) -> AppResult<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.answer.clone())
        }
    }

    #[test]
    fn vague_criterion_uses_llm_and_parses_bool() {
        // 模糊标准（规则无法求值）→ LLM，解析 {"passed":true}
        let c = json!("金额与后台一致");
        let calls = Arc::new(AtomicUsize::new(0));
        let llm = FixedProvider {
            answer: r#"{"passed":true,"note":"逐项核对一致"}"#.into(),
            calls: calls.clone(),
            mock: false,
        };
        let v = review(&c, &outputs(), &llm).unwrap();
        assert_eq!(v.checked_by, "llm");
        assert!(v.passed);
        assert!(v.note.contains("逐项核对一致"));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "应调用 LLM 一次");
    }

    #[test]
    fn forced_llm_mode_overrides_parseable_rule() {
        // 即便表达式本身可求值，mode=llm 也强制走 LLM。
        let c = json!({"criterion": "steps.n1.status == 200", "mode": "llm"});
        let calls = Arc::new(AtomicUsize::new(0));
        let llm = FixedProvider {
            answer: "false".into(),
            calls: calls.clone(),
            mock: false,
        };
        let v = review(&c, &outputs(), &llm).unwrap();
        assert_eq!(v.checked_by, "llm");
        assert!(!v.passed);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn llm_failure_defaults_to_fail() {
        // LLM 回复无法解析 → 判 false（不轻信），保守。
        let c = json!("相关性判断");
        let llm = FixedProvider {
            answer: "无法判断".into(),
            calls: Arc::new(AtomicUsize::new(0)),
            mock: false,
        };
        let v = review(&c, &outputs(), &llm).unwrap();
        assert_eq!(v.checked_by, "llm");
        assert!(!v.passed, "证据不足判 false");
    }

    #[test]
    fn mock_provider_marked_in_note() {
        let c = json!("模糊标准");
        let v = review(&c, &outputs(), &MockProvider).unwrap();
        assert_eq!(v.checked_by, "llm");
        assert!(v.note.contains("mock"), "mock 结论须标注");
    }
}
