use chrono::{Datelike, Duration, Local, TimeZone};

use crate::models::{BoardPeriodStats, BoardSummary, GroupStat, RunRow, SopRow};

/// run status → 看板分桶（对齐设计 §5.4 的完成/进行中/等人工/打回/失败）。
/// "rejected" 对应设计里的"打回"；cancelled 单列。
pub fn status_bucket(status: &str) -> &'static str {
    match status {
        "done" => "done",
        "running" | "pending" => "running",
        "wait_human" => "wait_human",
        "rejected" => "rejected",
        "failed" => "failed",
        "cancelled" => "cancelled",
        _ => "other",
    }
}

fn bucket_stats(label: &str, runs: &[&RunRow]) -> BoardPeriodStats {
    let mut s = BoardPeriodStats {
        label: label.to_string(),
        total: runs.len(),
        done: 0, running: 0, wait_human: 0, rejected: 0, failed: 0, cancelled: 0,
    };
    for r in runs {
        match status_bucket(&r.status) {
            "done" => s.done += 1,
            "running" => s.running += 1,
            "wait_human" => s.wait_human += 1,
            "rejected" => s.rejected += 1,
            "failed" => s.failed += 1,
            "cancelled" => s.cancelled += 1,
            _ => {}
        }
    }
    s
}

fn run_time(r: &RunRow) -> i64 {
    r.started_at.or(r.finished_at).unwrap_or(0)
}

/// 本地时区的今日 [00:00, +1d) 与本周 [周一 00:00, +7d) 起止（Unix 秒）。
fn period_bounds(now: i64) -> AppBounds {
    let now_lt: chrono::DateTime<Local> = Local.timestamp_opt(now, 0).single().unwrap_or_else(|| Local.timestamp_opt(0, 0).single().unwrap());
    let today = now_lt.date_naive();
    let day_start = Local.from_local_datetime(&today.and_hms_opt(0, 0, 0).unwrap()).unwrap().timestamp();
    // 本周一：weekday() 0=Mon，num_days_from_monday 同。
    let days_from_mon = now_lt.weekday().num_days_from_monday() as i64;
    let monday = today - Duration::days(days_from_mon);
    let week_start = Local.from_local_datetime(&monday.and_hms_opt(0, 0, 0).unwrap()).unwrap().timestamp();
    AppBounds { day_start, week_start }
}

struct AppBounds { day_start: i64, week_start: i64 }

/// 按 SOP / 风险分类聚合（全部 run，不限时间窗——看板分类是累计事实）。
fn group_stats<F, G>(runs: &[&RunRow], key_of: F, label_of: G) -> Vec<GroupStat>
where
    F: Fn(&RunRow) -> String,
    G: Fn(&RunRow) -> String,
{
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, GroupStat> = BTreeMap::new();
    for r in runs {
        let key = key_of(r);
        let e = map.entry(key.clone()).or_insert_with(|| GroupStat {
            key: key.clone(), label: label_of(r), total: 0, done: 0, wait_human: 0, failed: 0,
        });
        e.total += 1;
        match status_bucket(&r.status) {
            "done" => e.done += 1,
            "wait_human" => e.wait_human += 1,
            "failed" => e.failed += 1,
            _ => {}
        }
    }
    map.into_values().collect()
}

/// 聚合看板数据：全部从本地 runs/sops 计算，不造数、不跨店比绝对值。
pub fn aggregate(runs: &[RunRow], sops: &[SopRow], now: i64) -> BoardSummary {
    let bounds = period_bounds(now);
    let refs: Vec<&RunRow> = runs.iter().collect();
    let day: Vec<&RunRow> = refs.iter().copied().filter(|r| run_time(r) >= bounds.day_start).collect();
    let week: Vec<&RunRow> = refs.iter().copied().filter(|r| run_time(r) >= bounds.week_start).collect();

    let name_of = |r: &RunRow| -> String {
        sops.iter().find(|s| s.id == r.sop_id).map(|s| s.name.clone()).unwrap_or_else(|| r.sop_id.clone())
    };
    let risk_of = |r: &RunRow| -> String {
        sops.iter().find(|s| s.id == r.sop_id).map(|s| s.risk.clone()).unwrap_or_else(|| "unknown".into())
    };
    let by_sop = group_stats(&refs, |r| r.sop_id.clone(), name_of);
    let by_risk = group_stats(&refs, |r| risk_of(r), |r| risk_of(r));

    BoardSummary {
        day: bucket_stats("今日", &day),
        week: bucket_stats("本周", &week),
        by_sop,
        by_risk,
        scope_note: "本地事实数据：按运行记录统计，不跨店比绝对值、不因数字没动就建议加投入或换方向。".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::now_secs;

    fn run(id: &str, sop: &str, status: &str, started: i64) -> RunRow {
        RunRow {
            id: id.into(), sop_id: sop.into(), sop_version: 1, status: status.into(),
            mode: "build".into(), trigger_by: "manual".into(), started_at: Some(started),
            finished_at: None, verdict: None, note: None, definition_snapshot: None,
            current_node: None, outputs_json: "{}".into(), branch_skipped: "[]".into(),
            reject_count: 0,
        }
    }
    fn sop(id: &str, name: &str, risk: &str) -> SopRow {
        SopRow {
            id: id.into(), name: name.into(), version: 1, trigger_json: "{}".into(),
            risk: risk.into(), owner_policy: "hybrid".into(), enabled: true,
            created_at: 0, updated_at: 0, definition: "{}".into(),
        }
    }

    #[test]
    fn status_buckets() {
        assert_eq!(status_bucket("done"), "done");
        assert_eq!(status_bucket("running"), "running");
        assert_eq!(status_bucket("pending"), "running");
        assert_eq!(status_bucket("wait_human"), "wait_human");
        assert_eq!(status_bucket("rejected"), "rejected");
        assert_eq!(status_bucket("failed"), "failed");
        assert_eq!(status_bucket("cancelled"), "cancelled");
    }

    #[test]
    fn aggregate_day_week_and_groups() {
        // 固定 2026-11-20（周五）15:00，避免测试依赖运行当天是周几。
        let now = Local.with_ymd_and_hms(2026, 11, 20, 15, 0, 0).unwrap().timestamp();
        let b = period_bounds(now);
        let runs = vec![
            run("r1", "sop-a", "done", b.day_start + 60),       // 今日、本周
            run("r2", "sop-b", "wait_human", b.day_start + 120), // 今日、本周
            run("r3", "sop-a", "failed", b.week_start + 60),     // 本周非今日（周一 00:01）
            run("r4", "sop-b", "done", b.week_start - 1),        // 上周日，不计本周
            run("r5", "sop-a", "done", b.week_start - 86400),    // 更早，不计本周
        ];
        let sops = vec![sop("sop-a", "巡检 A", "low"), sop("sop-b", "巡检 B", "high")];
        let s = aggregate(&runs, &sops, now);
        assert_eq!(s.day.total, 2, "今日 = r1 r2");
        assert_eq!(s.day.done, 1);
        assert_eq!(s.day.wait_human, 1);
        assert_eq!(s.week.total, 3, "本周 = r1 r2 r3（r4/r5 在上周）");
        let a = s.by_sop.iter().find(|g| g.key == "sop-a").unwrap();
        assert_eq!(a.label, "巡检 A");
        assert_eq!(a.total, 3); // r1 r3 r5（累计，不限周）
        let high = s.by_risk.iter().find(|g| g.key == "high").unwrap();
        assert_eq!(high.total, 2); // r2 r4
    }

    #[test]
    fn scope_note_has_no_growth_hint() {
        let s = aggregate(&[], &[], now_secs());
        assert!(s.scope_note.contains("不跨店比绝对值"));
        assert!(s.scope_note.contains("不因数字没动就建议加投入或换方向"), "口径须明确否定催方向");
        assert!(!s.scope_note.contains("建议加大投入"), "不得出现正向加投入建议");
    }
}
