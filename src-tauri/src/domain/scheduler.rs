use rusqlite::Connection;

use crate::domain::executor::{self, ToolRunner};
use crate::domain::llm::LlmProvider;
use crate::infra::store;
use crate::models::{now_secs, AppResult, SchedulerTickResult};

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

    for (sop_id, interval_secs) in due {
        // 占位标记：本次 tick 期间该项不再被选中。
        store::mark_schedule(conn, &sop_id, now + interval_secs, now, "running")?;
        match executor::start_run(
            conn, &sop_id, "build", "scheduler", &serde_json::json!({}), tools, llm,
        ) {
            Ok(run_id) => {
                started.push(run_id);
                store::mark_schedule(conn, &sop_id, now + interval_secs, now, "idle")?;
            }
            Err(e) => {
                errors.push(format!("{}: {}", sop_id, e));
                store::mark_schedule(conn, &sop_id, now + interval_secs, now, "error")?;
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
}
