use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike};
use serde_json::json;

use crate::models::{AppError, AppResult};

/// 标准 5 段 cron 表达式：分 时 日 月 周（minute hour dom month dow）。
/// 支持 `*`、`*/n`、`a-b`、`a-b/n`、`a,b,c` 及其组合；周 0 和 7 都表示周日。
/// 按**本地时区**解释：每天 9 点就是本机 09:00，不是 UTC 09:00。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExpr {
    pub raw: String,
    minutes: Vec<u32>,
    hours: Vec<u32>,
    doms: Vec<u32>,
    months: Vec<u32>,
    dows: Vec<u32>, // 0=周日 ... 6=周六
    dom_any: bool,
    dow_any: bool,
}

fn parse_field(s: &str, min: u32, max: u32) -> AppResult<(Vec<u32>, bool)> {
    let s = s.trim();
    if s.is_empty() {
        return Err(AppError::InvalidInput("cron 字段不得为空".into()));
    }
    let any = s == "*";
    let mut out = BTreeSet::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(AppError::InvalidInput("cron 列表含空段".into()));
        }
        // 拆 `基础/步长`
        let (base, step) = match part.split_once('/') {
            Some((b, st)) => {
                let st: u32 = st.parse().map_err(|_| {
                    AppError::InvalidInput(format!("cron 非法步长：{}", part))
                })?;
                if st == 0 {
                    return Err(AppError::InvalidInput("cron 步长须 > 0".into()));
                }
                (b, st)
            }
            None => (part, 1),
        };
        let range: (u32, u32) = if base == "*" {
            (min, max)
        } else if let Some((a, b)) = base.split_once('-') {
            let lo: u32 = a.parse().map_err(|_| {
                AppError::InvalidInput(format!("cron 非法区间：{}", part))
            })?;
            let hi: u32 = b.parse().map_err(|_| {
                AppError::InvalidInput(format!("cron 非法区间：{}", part))
            })?;
            if lo > hi {
                return Err(AppError::InvalidInput(format!("cron 区间倒置：{}", part)));
            }
            (lo, hi)
        } else {
            let v: u32 = base.parse().map_err(|_| {
                AppError::InvalidInput(format!("cron 非法取值：{}", part))
            })?;
            (v, v)
        };
        if range.0 < min || range.1 > max {
            return Err(AppError::InvalidInput(format!(
                "cron 取值越界：{}（应在 {}-{}）", part, min, max
            )));
        }
        let mut v = range.0;
        while v <= range.1 {
            out.insert(v);
            v += step;
        }
    }
    if out.is_empty() {
        return Err(AppError::InvalidInput("cron 字段无有效取值".into()));
    }
    Ok((out.into_iter().collect(), any))
}

impl CronExpr {
    pub fn parse(expr: &str) -> AppResult<Self> {
        let fields: Vec<&str> = expr.trim().split_whitespace().collect();
        if fields.len() != 5 {
            return Err(AppError::InvalidInput(
                "cron 须为 5 段：分 时 日 月 周（如 0 9 * * *）".into(),
            ));
        }
        let (minutes, _) = parse_field(fields[0], 0, 59)?;
        let (hours, _) = parse_field(fields[1], 0, 23)?;
        let (doms, dom_any) = parse_field(fields[2], 1, 31)?;
        let (months, _) = parse_field(fields[3], 1, 12)?;
        let (dows_raw, dow_any) = parse_field(fields[4], 0, 7)?;
        // 周字段 7 == 0（周日）
        let mut dows: Vec<u32> = dows_raw.into_iter().map(|d| if d == 7 { 0 } else { d }).collect();
        dows.sort();
        dows.dedup();
        Ok(CronExpr {
            raw: expr.trim().to_string(),
            minutes, hours, doms, months, dows, dom_any, dow_any,
        })
    }

    fn matches(&self, dt: &DateTime<Local>) -> bool {
        if !self.minutes.contains(&dt.minute()) { return false; }
        if !self.hours.contains(&dt.hour()) { return false; }
        if !self.months.contains(&(dt.month() as u32)) { return false; }
        // cron 语义：dom 与 dow 都受限时取 OR（任一满足即触发）；否则各自满足。
        let dom_ok = self.doms.contains(&(dt.day() as u32));
        let dow_ok = self.dows.contains(&(dt.weekday().num_days_from_sunday()));
        let day_ok = match (self.dom_any, self.dow_any) {
            (false, false) => dom_ok || dow_ok,
            (false, true) => dom_ok,
            (true, false) => dow_ok,
            (true, true) => true,
        };
        day_ok
    }

    /// 求 `from_secs`（Unix 秒）之后下一次匹配时刻的 Unix 秒。
    /// 逐分钟推进，最多扫描 4 年，防止不可能表达式死循环。
    pub fn next_after(&self, from_secs: i64) -> AppResult<i64> {
        // 对齐到下一分钟的整点（from_secs 所在分钟内不再触发）。
        let mut t = (from_secs / 60 + 1) * 60;
        let limit = from_secs + 4 * 366 * 24 * 3600;
        while t <= limit {
            let dt: DateTime<Local> = Local
                .timestamp_opt(t, 0)
                .single()
                .ok_or_else(|| AppError::InvalidInput("时间戳无法转本地时间".into()))?;
            if self.matches(&dt) {
                return Ok(t);
            }
            t += 60;
        }
        Err(AppError::InvalidInput("cron 表达式在 4 年内无匹配时刻".into()))
    }

    /// 给前端展示的简短说明（中英共用结构，纯事实陈述不编数）。
    pub fn describe(&self) -> String {
        json!({
            "expr": self.raw,
            "fields": {
                "minute": self.minutes,
                "hour": self.hours,
                "day": self.doms,
                "month": self.months,
                "weekday": self.dows
            }
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap().timestamp()
    }
    fn ymdhm(secs: i64) -> (i32, u32, u32, u32, u32) {
        let dt: DateTime<Local> = Local.timestamp_opt(secs, 0).unwrap();
        (dt.year(), dt.month(), dt.day(), dt.hour(), dt.minute())
    }

    #[test]
    fn parse_common_expressions() {
        assert!(CronExpr::parse("0 9 * * *").is_ok());       // 每天 9 点
        assert!(CronExpr::parse("*/5 * * * *").is_ok());     // 每 5 分钟
        assert!(CronExpr::parse("0 9 * * 1").is_ok());       // 每周一 9 点
        assert!(CronExpr::parse("30 2 1 * *").is_ok());      // 每月 1 号 2:30
        assert!(CronExpr::parse("0 0 1 1 *").is_ok());       // 每年元旦
        assert!(CronExpr::parse("0,15,30,45 * * * *").is_ok());
        assert!(CronExpr::parse("0 9 * * 7").is_ok());       // 周日=7
    }

    #[test]
    fn parse_rejects_bad_expressions() {
        assert!(CronExpr::parse("* * * *").is_err());        // 少一段
        assert!(CronExpr::parse("60 * * * *").is_err());     // 分钟越界
        assert!(CronExpr::parse("0 24 * * *").is_err());     // 小时越界
        assert!(CronExpr::parse("0 9 * * 8").is_err());      // 周越界
        assert!(CronExpr::parse("*/0 * * * *").is_err());    // 步长 0
        assert!(CronExpr::parse("5-1 * * * *").is_err());    // 区间倒置
        assert!(CronExpr::parse("abc * * * *").is_err());
    }

    #[test]
    fn daily_9am_next_after() {
        let c = CronExpr::parse("0 9 * * *").unwrap();
        // 当天 8:59 -> 当天 9:00
        let from = ts(2026, 11, 20, 8, 59);
        assert_eq!(ymdhm(c.next_after(from).unwrap()), (2026, 11, 20, 9, 0));
        // 当天 9:00 之后（含）-> 次日 9:00
        let from2 = ts(2026, 11, 20, 9, 0);
        assert_eq!(ymdhm(c.next_after(from2).unwrap()), (2026, 11, 21, 9, 0));
        // 当天 9:01 -> 次日 9:00
        let from3 = ts(2026, 11, 20, 9, 1);
        assert_eq!(ymdhm(c.next_after(from3).unwrap()), (2026, 11, 21, 9, 0));
    }

    #[test]
    fn every_five_minutes() {
        let c = CronExpr::parse("*/5 * * * *").unwrap();
        let from = ts(2026, 11, 20, 10, 3);
        assert_eq!(ymdhm(c.next_after(from).unwrap()), (2026, 11, 20, 10, 5));
        let from2 = ts(2026, 11, 20, 10, 5);
        assert_eq!(ymdhm(c.next_after(from2).unwrap()), (2026, 11, 20, 10, 10));
    }

    #[test]
    fn weekly_and_monthly() {
        // 2026-11-23 是周一
        let w = CronExpr::parse("0 9 * * 1").unwrap();
        let from = ts(2026, 11, 20, 12, 0); // 周五
        assert_eq!(ymdhm(w.next_after(from).unwrap()), (2026, 11, 23, 9, 0));

        let m = CronExpr::parse("30 2 1 * *").unwrap();
        let from2 = ts(2026, 11, 15, 0, 0);
        assert_eq!(ymdhm(m.next_after(from2).unwrap()), (2026, 12, 1, 2, 30));
    }

    #[test]
    fn dom_dow_or_semantics() {
        // 1 号 或 每周一 的 9 点（dom 与 dow 都受限 → OR）
        let c = CronExpr::parse("0 9 1 * 1").unwrap();
        // 2026-11-23 周一
        let from = ts(2026, 11, 20, 12, 0);
        assert_eq!(ymdhm(c.next_after(from).unwrap()), (2026, 11, 23, 9, 0));
        // 2026-11-30 是周一（先于 12-01 命中）
        let from2 = ts(2026, 11, 24, 12, 0);
        assert_eq!(ymdhm(c.next_after(from2).unwrap()), (2026, 11, 30, 9, 0));
        // 跳过周一后再验 dom：从 11-30 9:01 起 → 12-01（1 号，周二）
        let from3 = ts(2026, 11, 30, 9, 1);
        assert_eq!(ymdhm(c.next_after(from3).unwrap()), (2026, 12, 1, 9, 0));
    }

    #[test]
    fn step_and_range() {
        let c = CronExpr::parse("0 9-17 * * *").unwrap();
        let from = ts(2026, 11, 20, 8, 0);
        assert_eq!(ymdhm(c.next_after(from).unwrap()), (2026, 11, 20, 9, 0));
        let from2 = ts(2026, 11, 20, 9, 30);
        assert_eq!(ymdhm(c.next_after(from2).unwrap()), (2026, 11, 20, 10, 0));
    }
}
