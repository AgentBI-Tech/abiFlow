use serde_json::Value;

/// 写入 step_logs 前对入参/产出脱敏（实现设计 §3.2）。
/// 规则保守：只替换明显敏感形态，不试图理解业务语义。
pub fn redact_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut buf = String::new();
    for ch in s.chars() {
        if ch.is_alphanumeric() || ch == '@' || ch == '.' || ch == '_' || ch == '-' || ch == '+' {
            buf.push(ch);
        } else {
            out.push_str(&redact_token(&buf));
            buf.clear();
            out.push(ch);
        }
    }
    out.push_str(&redact_token(&buf));
    out
}

fn redact_token(t: &str) -> String {
    if t.is_empty() {
        return String::new();
    }
    // 邮箱：含 @ 且 @ 后有点
    if let Some((_, domain)) = t.split_once('@') {
        if domain.contains('.') {
            return "[redacted]".into();
        }
    }
    // 长 token / 密钥：连续 32+ 位且含数字或字母混合
    if t.len() >= 32 && t.chars().any(|c| c.is_ascii_digit()) && t.chars().any(|c| c.is_alphabetic()) {
        return "[redacted]".into();
    }
    // 手机/长数字串：11 位以上纯数字（含 +、-、空格由分隔处理）
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 11 && digits.len() == t.chars().filter(|c| c.is_ascii_digit() || *c == '+' || *c == '-').count() {
        return "[redacted]".into();
    }
    t.to_string()
}

/// 递归脱敏：对敏感键名（token/password/...）整值替换，对字符串文本按形态替换。
pub fn redact_value(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map {
                let key = k.to_ascii_lowercase();
                if ["token", "password", "passwd", "secret", "api_key", "apikey", "authorization", "cookie", "credential"]
                    .iter()
                    .any(|s| key.contains(s))
                {
                    out.insert(k.clone(), Value::String("[redacted]".into()));
                } else {
                    out.insert(k.clone(), redact_value(val));
                }
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(redact_value).collect()),
        Value::String(s) => Value::String(redact_text(s)),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_email_and_token() {
        let s = "contact me at jc@example.com or use abc123DEF456ghi789JKL012mno345PQR678";
        let r = redact_text(s);
        assert!(r.contains("[redacted]"));
        assert!(!r.contains("jc@example.com"));
        assert!(!r.contains("abc123DEF456"));
    }

    #[test]
    fn redacts_sensitive_keys() {
        let v = json!({"token": "abc", "nested": {"password": "x", "safe": "ok"}});
        let r = redact_value(&v);
        assert_eq!(r["token"], "[redacted]");
        assert_eq!(r["nested"]["password"], "[redacted]");
        assert_eq!(r["nested"]["safe"], "ok");
    }

    #[test]
    fn keeps_normal_text() {
        let s = "异常订单 3 条，余额 100 元";
        assert_eq!(redact_text(s), s);
        let v = json!({"anomalies": 3, "note": "正常"});
        assert_eq!(redact_value(&v), v);
    }
}
