use serde::{Deserialize, Serialize};

use crate::models::{AppError, AppResult};

/// 执行模式（实现设计 §3.2）：plan 只规划只读工具、build 可执行、review 只读复核。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Plan,
    Build,
    Review,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Plan => "plan",
            Mode::Build => "build",
            Mode::Review => "review",
        }
    }

    pub fn parse(s: &str) -> AppResult<Self> {
        match s {
            "plan" => Ok(Mode::Plan),
            "build" => Ok(Mode::Build),
            "review" => Ok(Mode::Review),
            _ => Err(AppError::InvalidInput(format!("未知模式：{}", s))),
        }
    }
}

/// 数据可见面：工具访问的数据维度。成本面（cost）即「进价」，内容角色不可见。
pub const PLANE_PUBLIC: &str = "public";
pub const PLANE_CONTENT: &str = "content";
pub const PLANE_COST: &str = "cost";
pub const PLANE_OPS: &str = "ops";
pub const PLANE_FINANCE: &str = "finance";

/// 角色可见面策略（框架常量，后续可挪到配置）。内容角色只能看 public/content。
const ROLE_PLANES: &[(&str, &[&str])] = &[
    ("content", &[PLANE_PUBLIC, PLANE_CONTENT]),
    ("ops", &[PLANE_PUBLIC, PLANE_CONTENT, PLANE_COST, PLANE_OPS]),
    ("admin", &[PLANE_PUBLIC, PLANE_CONTENT, PLANE_COST, PLANE_OPS, PLANE_FINANCE]),
    ("agent", &[PLANE_PUBLIC, PLANE_CONTENT, PLANE_COST, PLANE_OPS]),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub id: String,
    /// content / ops / admin / agent —— 决定可见数据面。
    pub role: String,
}

impl Actor {
    pub fn agent() -> Self {
        Actor { id: "agent".into(), role: "agent".into() }
    }
    pub fn human(id: &str, role: &str) -> Self {
        Actor { id: id.to_string(), role: role.to_string() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub id: String,
    pub risk: String,          // low | medium | high
    pub plane: String,         // 该工具访问的数据面，默认 public
    pub requires_approval: bool,
    pub irreversible: bool,    // 资金/不可逆：永远人工门，不可被经验升级
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolGrant {
    pub tool_id: String,
    pub mode: Mode,
    pub allowed: bool,
    pub planes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CallCtx {
    pub run_id: String,
    pub actor: Actor,
    pub mode: Mode,
    pub grant: Option<ToolGrant>,
}

/// 授权判定（实现设计 §3.2 的 Allow|Deny|NeedGate）。纯逻辑，不碰数据库。
///
/// 顺序（不可调换）：
/// 1. 无授权或 allowed=0 → Deny
/// 2. 工具数据面不在授权面内 → Deny（「内容角色看不到进价」由第 3 步与第 2 步共同保证）
/// 3. 工具数据面不在角色可见面内 → Deny
/// 4. irreversible（资金/不可逆）→ 永远 NeedGate，不受经验升级影响
/// 5. risk=high 或 requires_approval → NeedGate
/// 6. 其余 → Allow
pub fn authorize(tool: &ToolSpec, ctx: &CallCtx) -> Decision {
    let grant = match &ctx.grant {
        Some(g) if g.allowed => g,
        _ => return Decision::Deny,
    };
    if !grant.planes.iter().any(|p| p == &tool.plane) {
        return Decision::Deny;
    }
    let role_planes = ROLE_PLANES
        .iter()
        .find(|(r, _)| *r == ctx.actor.role)
        .map(|(_, p)| *p)
        .unwrap_or(&[]);
    if !role_planes.iter().any(|p| *p == tool.plane) {
        return Decision::Deny;
    }
    if tool.irreversible || tool.risk == "high" || tool.requires_approval {
        return Decision::NeedGate;
    }
    Decision::Allow
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    NeedGate,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Deny => "deny",
            Decision::NeedGate => "need_gate",
        }
    }
}

/// 拒绝原因归类，供前端提示与 gate reason 用。
pub fn deny_reason(tool: &ToolSpec, ctx: &CallCtx) -> &'static str {
    match &ctx.grant {
        None => "no_grant",
        Some(g) if !g.allowed => "grant_disabled",
        Some(g) if !g.planes.iter().any(|p| p == &tool.plane) => "plane_not_granted",
        _ => "role_plane_forbidden",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(allowed: bool, planes: &[&str]) -> ToolGrant {
        ToolGrant {
            tool_id: "t".into(),
            mode: Mode::Build,
            allowed,
            planes: planes.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn ctx(role: &str, grant: Option<ToolGrant>) -> CallCtx {
        CallCtx {
            run_id: "r1".into(),
            actor: Actor { id: "u1".into(), role: role.into() },
            mode: Mode::Build,
            grant,
        }
    }
    fn tool(risk: &str, plane: &str, requires: bool, irreversible: bool) -> ToolSpec {
        ToolSpec {
            id: "t".into(),
            risk: risk.into(),
            plane: plane.into(),
            requires_approval: requires,
            irreversible,
        }
    }

    #[test]
    fn missing_or_disabled_grant_denies() {
        let t = tool("low", PLANE_PUBLIC, false, false);
        assert_eq!(authorize(&t, &ctx("ops", None)), Decision::Deny);
        assert_eq!(authorize(&t, &ctx("ops", Some(grant(false, &[PLANE_PUBLIC])))), Decision::Deny);
    }

    #[test]
    fn plane_not_in_grant_denies() {
        let t = tool("low", PLANE_COST, false, false);
        assert_eq!(authorize(&t, &ctx("admin", Some(grant(true, &[PLANE_CONTENT])))), Decision::Deny);
    }

    #[test]
    fn content_role_cannot_see_cost() {
        // 「内容角色看不到进价」：content 角色即使得了 cost 面授权，也被角色面策略拦下。
        let t = tool("low", PLANE_COST, false, false);
        let g = grant(true, &[PLANE_PUBLIC, PLANE_CONTENT, PLANE_COST]);
        assert_eq!(authorize(&t, &ctx("content", Some(g))), Decision::Deny);
        // ops 角色可看 cost。
        let g2 = grant(true, &[PLANE_COST]);
        assert_eq!(authorize(&t, &ctx("ops", Some(g2))), Decision::Allow);
    }

    #[test]
    fn high_risk_and_requires_approval_need_gate() {
        let g = grant(true, &[PLANE_PUBLIC]);
        assert_eq!(authorize(&tool("high", PLANE_PUBLIC, false, false), &ctx("ops", Some(g.clone()))), Decision::NeedGate);
        assert_eq!(authorize(&tool("low", PLANE_PUBLIC, true, false), &ctx("ops", Some(g))), Decision::NeedGate);
    }

    #[test]
    fn irreversible_always_needs_gate() {
        // 即使经验把工具标为低风险且批准，irreversible 仍永不自动放行。
        let g = grant(true, &[PLANE_PUBLIC, PLANE_FINANCE, PLANE_COST]);
        let t = tool("low", PLANE_FINANCE, false, true);
        assert_eq!(authorize(&t, &ctx("admin", Some(g))), Decision::NeedGate);
    }

    #[test]
    fn allow_when_all_checks_pass() {
        let g = grant(true, &[PLANE_PUBLIC]);
        assert_eq!(authorize(&tool("low", PLANE_PUBLIC, false, false), &ctx("ops", Some(g))), Decision::Allow);
    }

    #[test]
    fn mode_parse_roundtrip() {
        for s in ["plan", "build", "review"] {
            assert_eq!(Mode::parse(s).unwrap().as_str(), s);
        }
        assert!(Mode::parse("bad").is_err());
    }
}
