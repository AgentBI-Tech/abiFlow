use rusqlite::Connection;

use crate::domain::cron::CronExpr;
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

/// 扫描到期调度并触发 run。每个 due 项：先标记 running 防重入，触发后按结果回写下一次时间。
pub fn tick(
    conn: &Connection,
    tools: &dyn ToolRunner,
    llm: &dyn LlmProvider,
) -> AppResult<SchedulerTickResult> {
    let now = now_secs();
    let due = store::list_due_schedules(conn, now)?;
    let mut started = Vec::new();
    let mut errors = Vec::new();

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
    use crate::domain::executor::DemoTools;
    use crate::domain::llm::MockProvider;
    use crate::infra::db;

    #[test]
    fn tick_starts_due_schedule_once() {
        let c = db::open_in_memory().unwrap();
        executor::demo_install(&c).unwrap();
        let llm = MockProvider;
        // 把 next_run_at 提前到过去。
        c.execute("UPDATE schedules SET next_run_at=1", []).unwrap();
        let r = tick(&c, &DemoTools, &llm).unwrap();
        assert_eq!(r.started.len(), 1, "应触发一条 run");
        assert!(r.errors.is_empty());
        // 同一时刻再 tick 不应重复触发（next 已推到未来）。
        let r2 = tick(&c, &DemoTools, &llm).unwrap();
        assert!(r2.started.is_empty());
    }

    #[test]
    fn cron_schedule_aligns_to_calendar_boundary() {
        use crate::infra::store;
        let c = db::open_in_memory().unwrap();
        executor::demo_install(&c).unwrap();
        let llm = MockProvider;
        let sop_id = "sop-demo-daily-anomaly";
        // 设为 cron 模式：每天 9 点；把 next_run_at 提前到过去使其到期。
        store::upsert_cron_schedule(&c, sop_id, "0 9 * * *", 1).unwrap();
        let r = tick(&c, &DemoTools, &llm).unwrap();
        assert_eq!(r.started.len(), 1, "cron 到期应触发 run");
        assert!(r.errors.is_empty(), "cron 合法不应报错：{:?}", r.errors);
        // 下次触发必须是本地 09:00（分钟=0 小时=9），而不是 now+间隔。
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
        let sop_id = "sop-demo-daily-anomaly";
        store::upsert_cron_schedule(&c, sop_id, "not a cron", 1).unwrap();
        let r = tick(&c, &DemoTools, &llm).unwrap();
        assert!(r.started.is_empty(), "非法 cron 不应触发 run");
        assert_eq!(r.errors.len(), 1, "应报 1 条错误");
        assert!(r.errors[0].contains("cron 表达式非法"), "错误信息应点明 cron");
    }
}
