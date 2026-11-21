//! 熔断判定（纯逻辑，不碰 IO）：置信度过低 / 原地打转 → 转人工门（设计 §5.2 转人工判定③）。
//!
//! 纪律：只读 LLM 产物本身与上游步骤输出，不编造置信度、不下合规判定；
//! 缺字段或字段类型不对时**不触发**（保守：不把"未知"当成"低置信度"）。

use std::collections::HashMap;

use serde_json::Value;

/// 默认置信度阈值。可由 SOP 节点 `config.confidence_threshold` 覆盖。
pub const DEFAULT_CONFIDENCE_THRESHOLD: f64 = 0.5;

#[derive(Debug, Clone, PartialEq)]
pub enum BreakReason {
    /// LLM 产物自带的 confidence 数字低于阈值。
    LowConfidence { score: f64, threshold: f64 },
    /// 当前产物与先前某一步的产物规范化后完全相同（无进展）。
    Loop { same_as: String },
}

/// 判定一条 agent_step 产物是否应熔断转人工。
/// `prior` 为已产出的步骤输出（键为 step id / "input"）；`input` 键不参与打转比对。
pub fn check_agent_output(
    current: &Value,
    prior: &HashMap<String, Value>,
    threshold: f64,
) -> Option<BreakReason> {
    // 1) 低置信度：只在产物自带数字 confidence 时判定。
    if let Some(obj) = current.as_object() {
        if let Some(score) = obj.get("confidence").and_then(|v| v.as_f64()) {
            if score < threshold {
                return Some(BreakReason::LowConfidence { score, threshold });
            }
        }
    }

    // 2) 原地打转：与先前任一步产物（规范化 JSON）逐字相同。
    // serde_json::Value 的 Map 默认按 key 排序，to_string 可作规范化形式。
    let canon = current.to_string();
    for (step_id, old) in prior {
        if step_id == "input" {
            continue;
        }
        if old.to_string() == canon {
            return Some(BreakReason::Loop {
                same_as: step_id.clone(),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn low_confidence_below_threshold_breaks() {
        let cur = json!({"confidence": 0.2, "text": "可能"});
        assert_eq!(
            check_agent_output(&cur, &HashMap::new(), DEFAULT_CONFIDENCE_THRESHOLD),
            Some(BreakReason::LowConfidence {
                score: 0.2,
                threshold: 0.5,
            })
        );
    }

    #[test]
    fn confidence_at_threshold_does_not_break() {
        let cur = json!({"confidence": 0.5});
        assert_eq!(
            check_agent_output(&cur, &HashMap::new(), DEFAULT_CONFIDENCE_THRESHOLD),
            None
        );
    }

    #[test]
    fn missing_or_non_numeric_confidence_does_not_break() {
        // 缺字段不触发（未知 ≠ 低置信度）
        assert_eq!(
            check_agent_output(&json!({"text": "ok"}), &HashMap::new(), 0.5),
            None
        );
        // 非数字不触发
        assert_eq!(
            check_agent_output(&json!({"confidence": "低"}), &HashMap::new(), 0.5),
            None
        );
    }

    #[test]
    fn loop_detected_against_prior_step() {
        let cur = json!({"text": "相同结论"});
        let mut prior = HashMap::new();
        prior.insert("n1".to_string(), json!({"text": "相同结论"}));
        assert_eq!(
            check_agent_output(&cur, &prior, 0.5),
            Some(BreakReason::Loop {
                same_as: "n1".into()
            })
        );
    }

    #[test]
    fn loop_ignores_input_key() {
        let cur = json!({"text": " Echo "});
        let mut prior = HashMap::new();
        prior.insert("input".to_string(), json!({"text": " Echo "}));
        assert_eq!(check_agent_output(&cur, &prior, 0.5), None);
    }

    #[test]
    fn different_output_does_not_break() {
        let cur = json!({"text": "新结论", "confidence": 0.9});
        let mut prior = HashMap::new();
        prior.insert("n1".to_string(), json!({"text": "旧结论"}));
        assert_eq!(check_agent_output(&cur, &prior, 0.5), None);
    }

    #[test]
    fn custom_threshold_respected() {
        let cur = json!({"confidence": 0.6});
        assert_eq!(
            check_agent_output(&cur, &HashMap::new(), 0.8),
            Some(BreakReason::LowConfidence {
                score: 0.6,
                threshold: 0.8,
            })
        );
    }
}
