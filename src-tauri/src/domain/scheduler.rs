use rusqlite::Connection;

use crate::domain::cron::CronExpr;
use crate::domain::event_bus::EventSource;
use crate::domain::executor::{self, ToolRunner};
use crate::domain::llm::LlmProvider;
use crate::infra::store;
use crate::models::{now_secs, AppResult, SchedulerTickResult};

/// 计算某到期项的下次触发时间：cron 模式用表达式求下一次匹配，否则 now + interval_secs。
fn next_run_at(interval_secs: i64, cron_expr: Option<&str>, now: i64) -> AppResult<i64> {
    match cron_expr {
        Some(expr) => CronExpr::parse(expr)?.next_after(now),
        None => Ok(now + interval_secs.max(1)),
    }
}

/// 判断 SOP 是否订阅了某事件。trigger_json 形如 {"type":"event","event":"new_order"}。
/// 解析失败 / 缺字段 / 非 event 类型一律视为不订阅（保守）。
fn subscribes_to(trigger_json: &str, event_type: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(trigger_json) else {
        return false;
    };
    v.get("type").and_then(|s| s.as_str()) == Some("event")
        && v.get("event").and_then(|s| s.as_str()) == Some(event_type)
}

/// 事件触发：消费事件队列，对每个事件找到订阅它的启用 SOP，启动 run。
/// 事件→run 与 cron 走同一个 start_run（build 模式、risk/人工门不变），只是 trigger_by="event"。
fn trigger_from_events(
    conn: &Connection,
    events: &dyn EventSource,
    tools: &dyn ToolRunner,
    llm: &dyn LlmProvider,
    started: &mut Vec<String>,
    errors: &mut Vec<String>,
) -> AppResult<()> {
    let evts = events.drain();
    if evts.is_empty() {
        return Ok(());
    }
    let sops = store::list_sops(conn, true)?; // 只启用
    for event in evts {
        for sop in &sops {
            if !subscribes_to(&sop.trigger_json, &event.event_type) {
                continue;
            }
            let input = serde_json::json!({
                "event_type": event.event_type,
                "payload": event.payload,
                "occurred_at": event.occurred_at,
            });
            match executor::start_run(conn, &sop.id, "build", "event", &input, tools, llm) {
                Ok(run_id) => started.push(run_id),
                Err(e) => errors.push(format!("事件 {} → SOP {}: {}", event.event_type, sop.id, e)),
            }
        }
    }
    Ok(())
}

/// 扫描到期调度 + 消费事件队列，触发 run。
/// 每个 due 项：先标记 running 防重入，触发后按结果回写下一次时间。
pub fn tick(
    conn: &Connection,
    events: &dyn EventSource,
    tools: &dyn ToolRunner,
    llm: &dyn LlmProvider,
) -> AppResult<SchedulerTickResult> {
    let now = now_secs();
    let mut started = Vec::new();
    let mut errors = Vec::new();

    // 第二类触发：先消费事件（事件无 next_run_at，不走 schedules 表）。
    trigger_from_events(conn, events, tools, llm, &mut started, &mut errors)?;

    // 第一类触发：cron / interval due。
    let due = store::list_due_schedules(conn, now)?;
    for due_item in due {
        let sop_id = &due_item.sop_id;
        // 先算下次触发时间（cron 模式按日历对齐，不是 now+间隔）；即使触发失败也照常推进。
        let next = match next_run_at(due_item.interval_secs, due_item.cron_expr.as_deref(), now) {
            Ok(n) => n,
            Err(e) => {
                errors.push(format!("{}: cron 表达式非法：{}", sop_id, e));
                continue;
            }
        };
        // 占位标记：本次 tick 期间该项不再被选中。
        store::mark_schedule(conn, sop_id, next, now, "running")?;
        match executor::start_run(
            conn, sop_id, "build", "scheduler", &serde_json::json!({}), tools, llm,
        ) {
            Ok(run_id) => {
                started.push(run_id);
                store::mark_schedule(conn, sop_id, next, now, "idle")?;
            }
            Err(e) => {
                errors.push(format!("{}: {}", sop_id, e));
                store::mark_schedule(conn, sop_id, next, now, "error")?;
            }
        }
    }
    Ok(SchedulerTickResult { started, errors })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::event_bus::InMemoryEventSource;
    use crate::domain::executor::DemoTools;
    use crate::domain::llm::MockProvider;
    use crate::infra::db;
    use crate::models::SopUpsertPayload;

    #[test]
    fn tick_starts_due_schedule_once() {
        let c = db::open_in_memory().unwrap();
        executor::demo_install(&c).unwrap();
        let llm = MockProvider;
        let src = InMemoryEventSource::new();
        // 把 next_run_at 提前到过去。
        c.execute("UPDATE schedules SET next_run_at=1", []).unwrap();
        let r = tick(&c, &src, &DemoTools, &llm).unwrap();
        assert_eq!(r.started.len(), 1, "应触发一条 run");
        assert!(r.errors.is_empty());
        // 同一时刻再 tick 不应重复触发（next 已推到未来）。
        let r2 = tick(&c, &src, &DemoTools, &llm).unwrap();
        assert!(r2.started.is_empty());
    }

    #[test]
    fn cron_schedule_aligns_to_calendar_boundary() {
        use crate::infra::store;
        let c = db::open_in_memory().unwrap();
        executor::demo_install(&c).unwrap();
        let llm = MockProvider;
        let src = InMemoryEventSource::new();
        let sop_id = "sop-demo-daily-anomaly";
        store::upsert_cron_schedule(&c, sop_id, "0 9 * * *", 1).unwrap();
        let r = tick(&c, &src, &DemoTools, &llm).unwrap();
        assert_eq!(r.started.len(), 1, "cron 到期应触发 run");
        assert!(r.errors.is_empty(), "cron 合法不应报错：{:?}", r.errors);
        let next: i64 = c
            .query_row(
                "SELECT next_run_at FROM schedules WHERE sop_id=?1",
                rusqlite::params![sop_id],
                |r| r.get(0),
            )
            .unwrap();
        let expected = CronExpr::parse("0 9 * * *").unwrap().next_after(now_secs()).unwrap();
        assert_eq!(next, expected, "下次触发应对齐到下一个本地 9 点");
    }

    #[test]
    fn bad_cron_expr_reports_error_without_starting_run() {
        use crate::infra::store;
        let c = db::open_in_memory().unwrap();
        executor::demo_install(&c).unwrap();
        let llm = MockProvider;
        let src = InMemoryEventSource::new();
        let sop_id = "sop-demo-daily-anomaly";
        store::upsert_cron_schedule(&c, sop_id, "not a cron", 1).unwrap();
        let r = tick(&c, &src, &DemoTools, &llm).unwrap();
        assert!(r.started.is_empty(), "非法 cron 不应触发 run");
        assert_eq!(r.errors.len(), 1, "应报 1 条错误");
        assert!(r.errors[0].contains("cron 表达式非法"), "错误信息应点明 cron");
    }

    // ---- 事件触发 ----

    #[test]
    fn subscribes_to_parses_trigger_json() {
        assert!(subscribes_to(r#"{"type":"event","event":"new_order"}"#, "new_order"));
        assert!(!subscribes_to(r#"{"type":"event","event":"new_order"}"#, "stock_change"));
        assert!(!subscribes_to(r#"{"type":"cron","event":"new_order"}"#, "new_order"));
        assert!(!subscribes_to("not json", "new_order"));
        assert!(!subscribes_to(r#"{"event":"new_order"}"#, "new_order")); // 缺 type
    }

    #[test]
    fn event_triggers_matching_sop() {
        use crate::infra::store;
        let c = db::open_in_memory().unwrap();
        executor::demo_install(&c).unwrap();
        let llm = MockProvider;
        let src = InMemoryEventSource::new();
        let sop_id = "sop-event-new-order";
        // 新建一个启用 SOP，trigger_json 声明订阅 new_order 事件。
        store::upsert_sop(&c, &SopUpsertPayload {
            id: Some(sop_id.into()),
            name: "新订单处理".into(),
            trigger_json: r#"{"type":"event","event":"new_order"}"#.into(),
            risk: "low".into(),
            owner_policy: "agent".into(),
            // 用 demo 已注册的工具定义，保证 start_run 不被未注册工具挡住。
            definition: serde_json::json!({
                "version": 1,
                "nodes": [
                    {"id":"n0","type":"start","config":{}},
                    {"id":"n1","type":"mcp_tool","config":{"tool":"demo:check_anomalies","args":{"anomalies":3},"timeout_s":60}},
                    {"id":"n2","type":"human_gate","config":{"reason":"anomaly_review","prompt":"转人工？"}},
                    {"id":"n3","type":"acceptance","config":{"criteria":["steps.n1.anomalies == 3"]}},
                    {"id":"n4","type":"end","config":{}}
                ],
                "edges": [
                    {"from":"n0","to":"n1"},{"from":"n1","to":"n2"},
                    {"from":"n2","to":"n3"},{"from":"n3","to":"n4"}
                ]
            }),
            expect_version: None,
        }).unwrap();
        store::set_sop_enabled(&c, sop_id, true).unwrap();

        src.emit("new_order", serde_json::json!({"id":"o1"}));
        let r = tick(&c, &src, &DemoTools, &llm).unwrap();
        assert_eq!(r.started.len(), 1, "事件应触发一条 run");
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        // 同一事件（队列已空）再 tick 不应再触发。
        let r2 = tick(&c, &src, &DemoTools, &llm).unwrap();
        assert!(r2.started.is_empty());
    }

    #[test]
    fn event_with_no_subscriber_starts_nothing() {
        let c = db::open_in_memory().unwrap();
        executor::demo_install(&c).unwrap();
        let llm = MockProvider;
        let src = InMemoryEventSource::new();
        src.emit("unknown_event", serde_json::json!({}));
        let r = tick(&c, &src, &DemoTools, &llm).unwrap();
        assert!(r.started.is_empty(), "无订阅者不应触发 run");
        assert!(r.errors.is_empty());
    }
}
