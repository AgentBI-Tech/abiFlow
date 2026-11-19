use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models::{AppError, AppResult};

/// SOP AST 节点类型（实现设计 §4，7 种）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    Start,
    AgentStep,
    McpTool,
    HumanGate,
    Condition,
    Acceptance,
    End,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SopNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: NodeType,
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SopEdge {
    pub from: String,
    pub to: String,
    /// condition 节点的分支条件；省略表示默认分支。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SopDefinition {
    pub version: i32,
    pub nodes: Vec<SopNode>,
    pub edges: Vec<SopEdge>,
}

impl SopDefinition {
    pub fn node(&self, id: &str) -> Option<&SopNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn outgoing(&self, id: &str) -> Vec<&SopEdge> {
        self.edges.iter().filter(|e| e.from == id).collect()
    }

    /// 静态校验（§4）：结构合法、分支完整、工具存在、无非法环。
    /// `known_tools` 由调用方从 tools 表传入；domain 层不碰数据库。
    pub fn validate(&self, known_tools: &HashSet<String>) -> AppResult<()> {
        if self.nodes.is_empty() {
            return Err(AppError::InvalidInput("AST 无节点".into()));
        }
        let mut ids = HashSet::new();
        for n in &self.nodes {
            if !ids.insert(n.id.clone()) {
                return Err(AppError::InvalidInput(format!("重复节点 id：{}", n.id)));
            }
        }
        let starts = self.nodes.iter().filter(|n| n.node_type == NodeType::Start).count();
        let ends = self.nodes.iter().filter(|n| n.node_type == NodeType::End).count();
        if starts != 1 || ends != 1 {
            return Err(AppError::InvalidInput("AST 必须有且仅有一个 start 和一个 end".into()));
        }

        for e in &self.edges {
            if !ids.contains(&e.from) || !ids.contains(&e.to) {
                return Err(AppError::InvalidInput(format!(
                    "边引用了不存在的节点：{} -> {}", e.from, e.to
                )));
            }
        }

        for n in &self.nodes {
            match n.node_type {
                NodeType::Condition => {
                    if !self.outgoing(&n.id).iter().any(|e| e.when.is_some()) {
                        return Err(AppError::InvalidInput(format!(
                            "condition 节点 {} 缺少 when 分支", n.id
                        )));
                    }
                }
                NodeType::Acceptance => {
                    let has = n
                        .config
                        .get("criteria")
                        .and_then(|v| v.as_array())
                        .map(|a| !a.is_empty())
                        .unwrap_or(false);
                    if !has {
                        return Err(AppError::InvalidInput(format!(
                            "acceptance 节点 {} 至少需要一条判定标准", n.id
                        )));
                    }
                }
                NodeType::McpTool => {
                    let tool = n.config.get("tool").and_then(|v| v.as_str()).unwrap_or("");
                    if tool.is_empty() {
                        return Err(AppError::InvalidInput(format!("mcp_tool {} 缺 tool", n.id)));
                    }
                    if !known_tools.contains(tool) {
                        return Err(AppError::InvalidInput(format!("工具未注册：{}", tool)));
                    }
                }
                _ => {}
            }
        }

        self.check_cycles()?;
        Ok(())
    }

    /// 环检测：只允许经过 human_gate 的受控回环，其余环一律拒绝。
    fn check_cycles(&self) -> AppResult<()> {
        #[derive(Clone, Copy, PartialEq)]
        enum Color { White, Gray, Black }

        fn dfs(
            def: &SopDefinition,
            id: &str,
            color: &mut HashMap<String, Color>,
            stack: &mut Vec<String>,
        ) -> AppResult<()> {
            color.insert(id.to_string(), Color::Gray);
            stack.push(id.to_string());
            for e in def.outgoing(id) {
                match color.get(&e.to).copied().unwrap_or(Color::White) {
                    Color::White => dfs(def, &e.to, color, stack)?,
                    Color::Gray => {
                        // 回环必须包含 human_gate，否则判非法环。
                        let start = stack.iter().position(|x| x == &e.to).unwrap_or(0);
                        let has_gate = stack[start..]
                            .iter()
                            .filter_map(|n| def.node(n))
                            .any(|n| n.node_type == NodeType::HumanGate);
                        if !has_gate {
                            return Err(AppError::InvalidInput(format!(
                                "非法环：{} -> {}（未经过 human_gate）", e.from, e.to
                            )));
                        }
                    }
                    Color::Black => {}
                }
            }
            stack.pop();
            color.insert(id.to_string(), Color::Black);
            Ok(())
        }

        let mut color: HashMap<String, Color> = HashMap::new();
        let mut stack = Vec::new();
        for n in &self.nodes {
            if color.get(&n.id).copied().unwrap_or(Color::White) == Color::White {
                dfs(self, &n.id, &mut color, &mut stack)?;
            }
        }
        Ok(())
    }
}

/// 极简条件求值（仅支持 `字段 op 字面量` / true / false）。
/// 字段形如 `steps.n1.anomalies`；字面量可为数字、布尔或带引号字符串。
/// 不做任意代码执行，供 condition 与 acceptance 共用。
pub fn eval_condition(expr: &str, outputs: &HashMap<String, Value>) -> AppResult<bool> {
    let e = expr.trim();
    if e == "true" {
        return Ok(true);
    }
    if e == "false" {
        return Ok(false);
    }
    // 先匹配双字符运算符，再单字符，避免 >= 被 > 截断。
    for op in [">=", "<=", "==", "!=", ">", "<"] {
        if let Some((left, right)) = e.split_once(op) {
            let lv = resolve_field(left.trim(), outputs)?;
            let rv = parse_literal(right.trim());
            return compare(&lv, &rv, op);
        }
    }
    Err(AppError::InvalidInput(format!("无法解析条件表达式：{}", expr)))
}

fn resolve_field(path: &str, outputs: &HashMap<String, Value>) -> AppResult<Value> {
    let p = path.strip_prefix("steps.").unwrap_or(path);
    let mut parts = p.split('.');
    let step = parts.next().unwrap_or("").to_string();
    let rest: Vec<&str> = parts.collect();
    let v = outputs
        .get(&step)
        .ok_or_else(|| AppError::InvalidInput(format!("步骤 {} 无输出", step)))?;
    if rest.is_empty() {
        return Ok(v.clone());
    }
    let pointer = format!("/{}", rest.join("/"));
    v.pointer(&pointer)
        .cloned()
        .ok_or_else(|| AppError::InvalidInput(format!("字段 {} 不存在", path)))
}

fn parse_literal(s: &str) -> Value {
    if let Ok(n) = s.parse::<i64>() {
        return Value::Number(n.into());
    }
    if let Ok(f) = s.parse::<f64>() {
        return serde_json::json!(f);
    }
    if s == "true" {
        return Value::Bool(true);
    }
    if s == "false" {
        return Value::Bool(false);
    }
    let unquoted = s.trim_matches('"').trim_matches('\'');
    Value::String(unquoted.to_string())
}

fn compare(l: &Value, r: &Value, op: &str) -> AppResult<bool> {
    match op {
        "==" => Ok(l == r),
        "!=" => Ok(l != r),
        ">" | "<" | ">=" | "<=" => {
            if let (Some(a), Some(b)) = (l.as_f64(), r.as_f64()) {
                return Ok(match op {
                    ">" => a > b,
                    "<" => a < b,
                    ">=" => a >= b,
                    _ => a <= b,
                });
            }
            let a = l.as_str().unwrap_or("");
            let b = r.as_str().unwrap_or("");
            Ok(match op {
                ">" => a > b,
                "<" => a < b,
                ">=" => a >= b,
                _ => a <= b,
            })
        }
        _ => Err(AppError::InvalidInput(format!("未知运算符 {}", op))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> HashSet<String> {
        ["demo:check_anomalies".to_string()].into_iter().collect()
    }

    fn linear() -> SopDefinition {
        serde_json::from_value(json!({
            "version": 1,
            "nodes": [
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"mcp_tool","config":{"tool":"demo:check_anomalies","args":{}}},
                {"id":"n2","type":"human_gate","config":{"reason":"review","prompt":"转人工？"}},
                {"id":"n3","type":"acceptance","config":{"criteria":["steps.n1.anomalies == 0"]}},
                {"id":"n4","type":"end","config":{}}
            ],
            "edges": [
                {"from":"n0","to":"n1"},
                {"from":"n1","to":"n2"},
                {"from":"n2","to":"n3"},
                {"from":"n3","to":"n4"}
            ]
        })).unwrap()
    }

    #[test]
    fn validates_linear_sop() {
        linear().validate(&tools()).unwrap();
    }

    #[test]
    fn rejects_unknown_tool() {
        let mut d = linear();
        d.nodes[1].config = json!({"tool":"no:such","args":{}});
        assert!(d.validate(&tools()).is_err());
    }

    #[test]
    fn rejects_acceptance_without_criteria() {
        let mut d = linear();
        d.nodes[3].config = json!({});
        assert!(d.validate(&tools()).is_err());
    }

    #[test]
    fn rejects_condition_without_when_branch() {
        let mut d = linear();
        d.nodes[2].node_type = NodeType::Condition;
        d.nodes[2].config = json!({"expr":"true"});
        assert!(d.validate(&tools()).is_err());
    }

    #[test]
    fn allows_gate_cycle_but_rejects_plain_cycle() {
        let mut d = linear();
        d.edges.push(SopEdge { from: "n3".into(), to: "n2".into(), when: None });
        // n2 是 human_gate，回环允许
        d.validate(&tools()).unwrap();

        let mut d2 = linear();
        d2.edges.push(SopEdge { from: "n3".into(), to: "n1".into(), when: None });
        // 回环 n1->n2->n3->n1 含 gate n2，仍允许；改测纯工具回环
        let mut d3 = linear();
        d3.edges.retain(|e| e.from != "n1");
        d3.edges.push(SopEdge { from: "n0".into(), to: "n1".into(), when: None });
        d3.edges.push(SopEdge { from: "n1".into(), to: "n1".into(), when: None });
        assert!(d3.validate(&tools()).is_err(), "自环不含 gate 应拒绝");
        let _ = d2;
    }

    #[test]
    fn evaluates_conditions() {
        let mut out = HashMap::new();
        out.insert("n1".to_string(), json!({"anomalies": 3, "ok": true}));
        assert!(eval_condition("steps.n1.anomalies > 0", &out).unwrap());
        assert!(!eval_condition("steps.n1.anomalies == 0", &out).unwrap());
        assert!(eval_condition("steps.n1.ok == true", &out).unwrap());
        assert!(eval_condition("true", &out).unwrap());
        assert!(!eval_condition("false", &out).unwrap());
    }
}
