use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::infra::store;
use crate::models::{AppResult, ToolDefPayload};
use crate::domain::gateway::Mode;

/// 「第一个使用者」的示例工具组：指向 agentbi.tech 的只读 API。
/// 这些只是**配置数据**（scope_json），用户可在 UI 里改成任何自己的 HTTP 服务；
/// 核心引擎不认得 agentbi.tech，只按配置发请求。
pub fn agentbi_example_tools() -> Vec<ToolDefPayload> {
    vec![
        http_tool(
            "agentbi:health",
            "站点健康（示例）",
            "https://agentbi.tech/summary.json",
            json!({}),
        ),
        http_tool(
            "agentbi:funnel",
            "当日漏斗（示例）",
            "https://agentbi.tech/api/funnel",
            json!({}),
        ),
        http_tool(
            "agentbi:seo-llms",
            "llms.txt 检查（示例）",
            "https://agentbi.tech/llms.txt",
            json!({}),
        ),
        http_tool(
            "agentbi:seo-sitemap",
            "sitemap.xml 检查（示例）",
            "https://agentbi.tech/sitemap.xml",
            json!({}),
        ),
        http_tool(
            "agentbi:check",
            "单站检测（示例，url 参数）",
            "https://agentbi.tech/api/check",
            json!({"url": "{{arg:url}}"}),
        ),
    ]
}

fn http_tool(id: &str, title: &str, url: &str, query: Value) -> ToolDefPayload {
    ToolDefPayload {
        id: id.into(),
        title: title.into(),
        risk: "low".into(),
        server: "http".into(),
        command: "GET".into(),
        scope: json!({
            "kind": "http",
            "method": "GET",
            "url": url,
            "query": query,
            "timeout_s": 20,
            "plane": "public",
            "read_only": true
        }),
        enabled: true,
    }
}

/// 把示例工具组写入 tools 表（不存在则建、存在则更新，幂等可重放）。
pub fn install_agentbi_examples(conn: &rusqlite::Connection) -> AppResult<Vec<String>> {
    let mut ids = Vec::new();
    for t in agentbi_example_tools() {
        let scope = serde_json::to_string(&t.scope)
            .map_err(|e| crate::models::AppError::InvalidInput(e.to_string()))?;
        store::upsert_tool(
            conn, &t.id, &t.server, &t.command, &t.title, &t.risk, &scope, t.enabled,
        )?;
        // 三种模式都授权 public 面（否则 gateway 判 Deny）。
        for m in [Mode::Plan, Mode::Build, Mode::Review] {
            store::upsert_tool_grant(conn, &t.id, m, true, &["public".into()])?;
        }
        ids.push(t.id);
    }
    Ok(ids)
}

/// 「每日维护 agentbi.tech」SOP 示例：健康巡检 + SEO 产物 + 漏斗记录 + 异常转人工。
/// 这是一份可导出的模板；动作节点用自定义 HTTP 工具，用户可增删改。
pub fn maintenance_sop_definition() -> Value {
    json!({
        "version": 1,
        "nodes": [
            {"id":"n0","type":"start","config":{}},
            {"id":"n1","type":"mcp_tool","config":{"tool":"agentbi:health","args":{},"timeout_s":20}},
            {"id":"n2","type":"mcp_tool","config":{"tool":"agentbi:funnel","args":{},"timeout_s":20}},
            {"id":"n3","type":"mcp_tool","config":{"tool":"agentbi:seo-llms","args":{},"timeout_s":20}},
            {"id":"n4","type":"mcp_tool","config":{"tool":"agentbi:seo-sitemap","args":{},"timeout_s":20}},
            {"id":"n5","type":"condition","config":{"expr":"steps.n1.status == 200"}},
            {"id":"n6","type":"human_gate","config":{"reason":"health_anomaly","prompt":"站点健康端点非 200，是否转人工处理？"}},
            {"id":"n7","type":"acceptance","config":{"criteria":["steps.n1.status == 200"]}},
            {"id":"n8","type":"end","config":{}}
        ],
        "edges": [
            {"from":"n0","to":"n1"},
            {"from":"n1","to":"n2"},
            {"from":"n2","to":"n3"},
            {"from":"n3","to":"n4"},
            {"from":"n4","to":"n5"},
            {"from":"n5","to":"n6","when":"false"},
            {"from":"n5","to":"n7","when":"true"},
            {"from":"n6","to":"n7"},
            {"from":"n7","to":"n8"}
        ]
    })
}

/// 安装维护 SOP（不存在则建）。与示例工具组搭配，供「每日维护」直接跑。
pub fn install_maintenance_sop(conn: &rusqlite::Connection) -> AppResult<String> {
    // SOP 依赖示例工具；先幂等装好工具与授权，避免装出「引用未注册工具」的无效 SOP。
    install_agentbi_examples(conn)?;
    let sop_id = "sop-agentbi-daily-maintenance";
    let existing: Option<i32> = conn
        .query_row("SELECT version FROM sops WHERE id=?1", [sop_id], |r| r.get(0))
        .optional()
        .map_err(|e| crate::models::AppError::Db(e.to_string()))?;
    if existing.is_none() {
        let now = crate::models::now_secs();
        conn.execute(
            "INSERT INTO sops(id,name,version,trigger_json,risk,owner_policy,enabled,created_at,updated_at,definition) \
             VALUES(?1,'每日维护 agentbi.tech（示例）',1,'{\"type\":\"interval\",\"secs\":86400}','low','hybrid',1,?2,?2,?3)",
            params![sop_id, now, maintenance_sop_definition().to_string()],
        )
        .map_err(|e| crate::models::AppError::Db(e.to_string()))?;
    }
    // 每天调度一次。
    store::upsert_schedule(conn, sop_id, 86400)?;
    Ok(sop_id.into())
}

/// 放权演示 SOP：一个 human-owned 的 agent_step，用来端到端演示「修正累积 → 候选 → 人确认放权」。
/// 不依赖任何外部工具/店铺；仅本地 Mock 推理，可导出为模板。安装幂等（已存在则不覆盖用户改动）。
pub fn delegation_demo_definition() -> Value {
    json!({
        "version": 1,
        "nodes": [
            {"id":"n0","type":"start","config":{}},
            {"id":"n1","type":"agent_step","config":{"goal":"汇总上游并给出建议","owner":"human"}},
            {"id":"n2","type":"acceptance","config":{"criteria":["steps.n1.mock == true"]}},
            {"id":"n3","type":"end","config":{}}
        ],
        "edges": [
            {"from":"n0","to":"n1"},
            {"from":"n1","to":"n2"},
            {"from":"n2","to":"n3"}
        ]
    })
}

/// 安装放权演示 SOP（不存在则建）。
pub fn install_delegation_demo(conn: &rusqlite::Connection) -> AppResult<String> {
    let sop_id = "sop-delegation-demo";
    let existing: Option<i32> = conn
        .query_row("SELECT version FROM sops WHERE id=?1", [sop_id], |r| r.get(0))
        .optional()
        .map_err(|e| crate::models::AppError::Db(e.to_string()))?;
    if existing.is_none() {
        let now = crate::models::now_secs();
        conn.execute(
            "INSERT INTO sops(id,name,version,trigger_json,risk,owner_policy,enabled,created_at,updated_at,definition) \
             VALUES(?1,'放权演示（本地 Mock）',1,'{}','low','hybrid',1,?2,?2,?3)",
            params![sop_id, now, delegation_demo_definition().to_string()],
        )
        .map_err(|e| crate::models::AppError::Db(e.to_string()))?;
    }
    Ok(sop_id.into())
}

// ---- Shopify 只读工具组（2026-11-25）----
// 政策/FAQ 走 Storefront MCP（免 token）；商品/订单/客户/门店走社区 Admin MCP（需 token）。
// 只接只读工具：create/update/cancel/set_inventory/graphql_request 等写类工具一律不装。
fn shopify_mcp_tool(id: &str, title: &str, server: &str, tool: &str, _args: Value) -> ToolDefPayload {
    ToolDefPayload {
        id: id.into(),
        title: title.into(),
        risk: "low".into(),
        server: server.into(),
        command: "mcp".into(),
        scope: json!({
            "kind": "mcp",
            "server": server,
            "tool": tool,
            "read_only": true,
            "plane": "public"
        }),
        enabled: true,
    }
}

/// 6 个只读工具；args 默认值在 SOP 节点里覆盖（{{arg:*}} 可由节点 args 传入）。
pub fn shopify_readonly_tools() -> Vec<ToolDefPayload> {
    vec![
        shopify_mcp_tool(
            "shopify:policies", "Shopify 政策/FAQ（只读）", "shopify-storefront",
            "search_shop_policies_and_faqs", json!({"query": "{{arg:query}}"}),
        ),
        shopify_mcp_tool(
            "shopify:shop-info", "Shopify 门店信息（只读）", "shopify-admin", "get_shop", json!({}),
        ),
        shopify_mcp_tool(
            "shopify:products", "Shopify 商品列表（只读）", "shopify-admin", "list_products",
            json!({"first": 10}),
        ),
        shopify_mcp_tool(
            "shopify:orders", "Shopify 订单列表（只读）", "shopify-admin", "list_orders",
            json!({"first": 10}),
        ),
        shopify_mcp_tool(
            "shopify:customers", "Shopify 客户列表（只读）", "shopify-admin", "list_customers",
            json!({"first": 10}),
        ),
        shopify_mcp_tool(
            "shopify:locations", "Shopify 门店位置（只读）", "shopify-admin", "list_locations",
            json!({"first": 10}),
        ),
    ]
}

pub fn shopify_storefront_sop_definition() -> Value {
    json!({
        "version": 1,
        "nodes": [
            {"id":"n0","type":"start","config":{}},
            {"id":"n1","type":"mcp_tool","config":{"tool":"shopify:policies","args":{"query":"What is your return policy?"},"timeout_s":30}},
            {"id":"n2","type":"human_gate","config":{"reason":"review","prompt":"Shopify 政策/FAQ 已读取，请核对结果后放行或驳回。"}},
            {"id":"n3","type":"end","config":{}}
        ],
        "edges": [
            {"from":"n0","to":"n1"},
            {"from":"n1","to":"n2"},
            {"from":"n2","to":"n3"}
        ]
    })
}

pub fn shopify_admin_sop_definition() -> Value {
    json!({
        "version": 1,
        "nodes": [
            {"id":"n0","type":"start","config":{}},
            {"id":"n1","type":"mcp_tool","config":{"tool":"shopify:shop-info","args":{},"timeout_s":30}},
            {"id":"n2","type":"mcp_tool","config":{"tool":"shopify:products","args":{"first":10},"timeout_s":30}},
            {"id":"n3","type":"mcp_tool","config":{"tool":"shopify:orders","args":{"first":10},"timeout_s":30}},
            {"id":"n4","type":"human_gate","config":{"reason":"review","prompt":"Shopify 门店/商品/订单已只读拉取，请核对结果后放行或驳回。"}},
            {"id":"n5","type":"end","config":{}}
        ],
        "edges": [
            {"from":"n0","to":"n1"},
            {"from":"n1","to":"n2"},
            {"from":"n2","to":"n3"},
            {"from":"n3","to":"n4"},
            {"from":"n4","to":"n5"}
        ]
    })
}

/// 幂等安装 Shopify 只读工具 + 授权 + 两条 SOP（无调度：不自动对真店跑）。
pub fn install_shopify_readonly(conn: &rusqlite::Connection) -> AppResult<Vec<String>> {
    let mut ids = Vec::new();
    for t in shopify_readonly_tools() {
        let scope = serde_json::to_string(&t.scope)
            .map_err(|e| crate::models::AppError::InvalidInput(e.to_string()))?;
        store::upsert_tool(
            conn, &t.id, &t.server, &t.command, &t.title, &t.risk, &scope, t.enabled,
        )?;
        for m in [Mode::Plan, Mode::Build, Mode::Review] {
            store::upsert_tool_grant(conn, &t.id, m, true, &["public".into()])?;
        }
        ids.push(t.id);
    }
    for (sop_id, name, def) in [
        ("sop-shopify-storefront", "Shopify 政策/FAQ 只读（Storefront）", shopify_storefront_sop_definition()),
        ("sop-shopify-admin-read", "Shopify 门店/商品/订单 只读（Admin）", shopify_admin_sop_definition()),
    ] {
        let existing: Option<i32> = conn
            .query_row("SELECT version FROM sops WHERE id=?1", [sop_id], |r| r.get(0))
            .optional()
            .map_err(|e| crate::models::AppError::Db(e.to_string()))?;
        if existing.is_none() {
            let now = crate::models::now_secs();
            conn.execute(
                "INSERT INTO sops(id,name,version,trigger_json,risk,owner_policy,enabled,created_at,updated_at,definition) \
                 VALUES(?1,?2,1,'{}','low','hybrid',1,?3,?3,?4)",
                params![sop_id, name, now, def.to_string()],
            )
            .map_err(|e| crate::models::AppError::Db(e.to_string()))?;
        }
        ids.push(sop_id.into());
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::db;

    #[test]
    fn example_tools_are_readonly_http_config() {
        let tools = agentbi_example_tools();
        assert_eq!(tools.len(), 5);
        for t in tools {
            assert_eq!(t.scope["kind"], "http");
            assert_eq!(t.scope["method"], "GET");
            assert_eq!(t.scope["read_only"], true);
        }
    }

    #[test]
    fn install_is_idempotent() {
        let c = db::open_in_memory().unwrap();
        let a = install_agentbi_examples(&c).unwrap();
        let b = install_agentbi_examples(&c).unwrap();
        assert_eq!(a, b);
        let all = store::list_tool_specs(&c).unwrap();
        assert!(all.iter().any(|s| s.id == "agentbi:health"));
    }
    #[test]
    fn delegation_demo_installs_with_human_owned_step() {
        use crate::domain::sop::SopDefinition;
        let c = db::open_in_memory().unwrap();
        let id = install_delegation_demo(&c).unwrap();
        assert_eq!(id, "sop-delegation-demo");
        let def: SopDefinition = serde_json::from_value(delegation_demo_definition()).unwrap();
        assert_eq!(crate::domain::sop::effective_owner(def.node("n1").unwrap()), "human");
        // 幂等：重复安装不报错、不新增 SOP。
        install_delegation_demo(&c).unwrap();
        assert_eq!(store::list_sops(&c, false).unwrap().len(), 1);
    }

    #[test]
    fn maintenance_sop_validates_and_installs() {
        use crate::domain::sop::SopDefinition;
        let def: SopDefinition = serde_json::from_value(maintenance_sop_definition()).unwrap();
        let c = db::open_in_memory().unwrap();
        // 只装 SOP，不预先装工具——安装函数应自动把依赖的工具/授权补齐。
        let sop_id = install_maintenance_sop(&c).unwrap();
        assert_eq!(sop_id, "sop-agentbi-daily-maintenance");
        let known = store::known_tools(&c).unwrap();
        for t in agentbi_example_tools() {
            assert!(known.contains(&t.id), "SOP 依赖的工具 {} 应随 SOP 一起装好", t.id);
        }
        def.validate(&known).unwrap();
        assert!(store::get_sop(&c, &sop_id).is_ok());
        assert!(store::dashboard(&c).unwrap().schedules.iter().any(|s| s.sop_id == sop_id));
    }

    #[test]
    fn shopify_readonly_installs_and_validates() {
        use crate::domain::sop::SopDefinition;
        let c = db::open_in_memory().unwrap();
        let ids = install_shopify_readonly(&c).unwrap();
        assert_eq!(ids.len(), 8, "6 工具 + 2 SOP");
        let known = store::known_tools(&c).unwrap();
        for t in shopify_readonly_tools() {
            assert!(known.contains(&t.id));
        }
        for def in [shopify_storefront_sop_definition(), shopify_admin_sop_definition()] {
            let sop: SopDefinition = serde_json::from_value(def).unwrap();
            sop.validate(&known).unwrap();
        }
        // 幂等：重复安装不报错、不新增 SOP、不自动建调度。
        install_shopify_readonly(&c).unwrap();
        assert_eq!(store::dashboard(&c).unwrap().sops.len(), 2);
        assert!(store::dashboard(&c).unwrap().schedules.is_empty());
    }

}
