use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::models::{AppError, AppResult};

/// 模板换参复用（§5.5 / §8 v0.4）：把一个店铺验证过的 SOP 定义里的可换参数
/// `{{param:name}}` 替换成新值，生成新 SOP —— 换店铺只换参数，不重编流程。
/// `{{param:*}}` 与工具执行时的 `{{arg:*}}` / `{{cred:*}}` 明确区分：前者是模板声明的可换点，
/// 后者是运行时占位符，互不干扰。

/// 从 SOP 定义里收集所有 `{{param:name}}` 占位符名（去重、排序，便于前端生成输入框）。
pub fn collect_params(def: &Value) -> Vec<String> {
    let mut names = BTreeSet::new();
    collect_in(def, &mut names);
    names.into_iter().collect()
}

fn collect_in(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::String(s) => scan_placeholders(s, |name| {
            out.insert(name.to_string());
        }),
        Value::Object(m) => {
            for val in m.values() {
                collect_in(val, out);
            }
        }
        Value::Array(a) => {
            for val in a {
                collect_in(val, out);
            }
        }
        _ => {}
    }
}

/// 扫描字符串里的 `{{param:...}}`，把名字交给回调；未闭合的跳过（apply 阶段再报错）。
fn scan_placeholders<F: FnMut(&str)>(s: &str, mut f: F) {
    let mut rest = s;
    while let Some(start) = rest.find("{{param:") {
        let after = &rest[start + 8..];
        if let Some(end) = after.find("}}") {
            let name = after[..end].trim();
            if !name.is_empty() {
                f(name);
            }
            rest = &after[end + 2..];
        } else {
            break;
        }
    }
}

/// 把定义里的 `{{param:name}}` 替换为 params 中同名值（字符串/数字/布尔按原类型注入）。
/// 缺参数或占位符未闭合 → 报错；params 中多余的键忽略（不污染定义）。
pub fn apply_params(def: &Value, params: &Map<String, Value>) -> AppResult<Value> {
    replace_in(def, params)
}

fn replace_in(v: &Value, params: &Map<String, Value>) -> AppResult<Value> {
    match v {
        Value::String(s) => Ok(Value::String(replace_in_string(s, params)?)),
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, val) in m {
                out.insert(k.clone(), replace_in(val, params)?);
            }
            Ok(Value::Object(out))
        }
        Value::Array(a) => {
            let mut out = Vec::with_capacity(a.len());
            for val in a {
                out.push(replace_in(val, params)?);
            }
            Ok(Value::Array(out))
        }
        other => Ok(other.clone()),
    }
}

fn replace_in_string(s: &str, params: &Map<String, Value>) -> AppResult<String> {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("{{param:") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 8..];
        let end = after
            .find("}}")
            .ok_or_else(|| AppError::InvalidInput("模板占位符未闭合：{{param:".into()))?;
        let name = after[..end].trim();
        let value = params.get(name).ok_or_else(|| {
            AppError::InvalidInput(format!("缺少复用参数：{}", name))
        })?;
        match value {
            Value::String(x) => out.push_str(x),
            Value::Number(x) => out.push_str(&x.to_string()),
            Value::Bool(x) => out.push_str(if *x { "true" } else { "false" }),
            Value::Null => out.push_str("null"),
            other => out.push_str(&other.to_string()),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collect_params_dedup_and_sorted() {
        let def = json!({
            "nodes": [
                {"config": {"url": "https://{{param:shop}}.example.com", "args": {"id": "{{param:sku}}"}}},
                {"config": {"goal": "{{param:shop}} 再核实"}}
            ]
        });
        assert_eq!(collect_params(&def), vec!["shop", "sku"]);
    }

    #[test]
    fn apply_params_replaces_typed_values() {
        let def = json!({
            "nodes": [
                {"config": {"url": "https://{{param:shop}}/p/{{param:sku}}"}},
                {"config": {"args": {"n": "{{param:count}}", "flag": "{{param:on}}"}}}
            ]
        });
        let params = json!({"shop": "new-shop", "sku": "ABC", "count": 5, "on": true}).as_object().unwrap().clone();
        let out = apply_params(&def, &params).unwrap();
        let nodes = out["nodes"].as_array().unwrap();
        assert_eq!(nodes[0]["config"]["url"], "https://new-shop/p/ABC");
        // 数字/布尔按字符串注入到原本的字符串位置（占位符在字符串里）。
        assert_eq!(nodes[1]["config"]["args"]["n"], "5");
        assert_eq!(nodes[1]["config"]["args"]["flag"], "true");
        // 原定义未被修改（apply 返回新值）。
        assert_eq!(def["nodes"][0]["config"]["url"], "https://{{param:shop}}/p/{{param:sku}}");
    }

    #[test]
    fn missing_param_errors() {
        let def = json!({"config": {"url": "{{param:shop}}"}});
        let params = json!({"other": "x"}).as_object().unwrap().clone();
        assert!(apply_params(&def, &params).is_err());
    }

    #[test]
    fn unclosed_placeholder_errors() {
        let def = json!({"config": {"url": "{{param:shop"}});
        let params = json!({"shop": "x"}).as_object().unwrap().clone();
        assert!(apply_params(&def, &params).is_err());
    }

    #[test]
    fn no_placeholder_returns_same() {
        let def = json!({"config": {"url": "https://example.com"}});
        let out = apply_params(&def, &Map::new()).unwrap();
        assert_eq!(out, def);
        assert!(collect_params(&def).is_empty());
    }

    #[test]
    fn arg_and_cred_untouched() {
        // 运行时的 {{arg:*}} / {{cred:*}} 不应被换参收集或替换。
        let def = json!({"config": {"url": "{{arg:url}}", "key": "{{cred:k}}", "shop": "{{param:shop}}"}});
        assert_eq!(collect_params(&def), vec!["shop"]);
        let params = json!({"shop": "s"}).as_object().unwrap().clone();
        let out = apply_params(&def, &params).unwrap();
        assert_eq!(out["config"]["url"], "{{arg:url}}");
        assert_eq!(out["config"]["key"], "{{cred:k}}");
    }

    #[test]
    fn reuse_validated_template_into_new_sop() {
        // 端到端（domain+store）：validated 模板的 snapshot 换参后生成新 SOP。
        use crate::domain::experience::MANIFEST_SCHEMA;
        use crate::infra::db;
        use crate::infra::store;
        use crate::models::TemplateBundle;
        use crate::models::SopUpsertPayload;

        let c = db::open_in_memory().unwrap();
        let snapshot = json!({
            "version": 1,
            "nodes": [
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"mcp_tool","config":{"tool":"demo:check","args":{"url":"https://{{param:shop}}.example.com","sku":"{{param:sku}}"}}},
                {"id":"n2","type":"end","config":{}}
            ],
            "edges": [{"from":"n0","to":"n1"},{"from":"n1","to":"n2"}]
        }).to_string();
        let bundle = TemplateBundle {
            manifest_schema: MANIFEST_SCHEMA,
            name: "巡检模板".into(),
            validated: true,
            sop_snapshot: snapshot,
            trajectory_json: "[]".into(),
            corrections_json: "[]".into(),
            source_shop: Some("shop-a".into()),
        };
        let tid = store::save_template(&c, &bundle).unwrap();
        let t = store::list_templates(&c).unwrap().into_iter().find(|x| x.id == tid).unwrap();
        assert!(t.validated, "模板须实测通过才可复用");
        let def: Value = serde_json::from_str(&t.sop_snapshot).unwrap();
        assert_eq!(collect_params(&def), vec!["shop", "sku"]);
        let params = json!({"shop": "shop-b", "sku": "XYZ"}).as_object().unwrap().clone();
        let new_def = apply_params(&def, &params).unwrap();
        let upsert = SopUpsertPayload {
            id: None,
            name: "巡检 B 店".into(),
            trigger_json: "{}".into(),
            risk: "low".into(),
            owner_policy: "hybrid".into(),
            definition: new_def,
            expect_version: None,
        };
        let (new_id, _) = store::upsert_sop(&c, &upsert).unwrap();
        let new_sop = store::get_sop(&c, &new_id).unwrap();
        let used: Value = serde_json::from_str(&new_sop.definition).unwrap();
        assert_eq!(used["nodes"][1]["config"]["args"]["url"], "https://shop-b.example.com");
        assert_eq!(used["nodes"][1]["config"]["args"]["sku"], "XYZ");
        // 原模板快照未被修改。
        let t2 = store::list_templates(&c).unwrap().into_iter().find(|x| x.id == tid).unwrap();
        assert!(t2.sop_snapshot.contains("{{param:shop}}"), "原模板须保持不变");
    }
}
