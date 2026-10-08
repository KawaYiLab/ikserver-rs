//! 时间 helper。存储与计算一律 UTC；只有"业务上的一天"才换算北京时间。
//!
//! 北京时间全年 UTC+8、无夏令时，因此这里直接用固定偏移换算，避免时区库的歧义分支。

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};

pub const BEIJING_OFFSET_HOURS: i64 = 8;
/// 业务日从北京时间 04:00 开始（经验日、签到日、每日限额统一用这个口径）。
pub const BIZ_DAY_START_HOUR: u32 = 4;

/// 北京时间的日历日期（0 点切换）。
pub fn beijing_date(at: DateTime<Utc>) -> NaiveDate {
    (at + Duration::hours(BEIJING_OFFSET_HOURS)).date_naive()
}

/// 业务日：北京时间 04:00 之前仍算前一天。
pub fn biz_day(at: DateTime<Utc>) -> NaiveDate {
    beijing_date(at - Duration::hours(i64::from(BIZ_DAY_START_HOUR)))
}

/// 某个业务日开始的 UTC 时刻（该日北京时间 04:00）。
pub fn biz_day_start(day: NaiveDate) -> DateTime<Utc> {
    let local = day
        .and_hms_opt(BIZ_DAY_START_HOUR, 0, 0)
        .unwrap_or_else(|| day.and_time(NaiveTime::MIN));
    (local - Duration::hours(BEIJING_OFFSET_HOURS)).and_utc()
}

/// `at` 所在业务日的 `[开始, 结束)` UTC 区间，用于 SQL 按日统计。
pub fn biz_day_range(at: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = biz_day_start(biz_day(at));
    (start, start + Duration::days(1))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn beijing_date_switches_at_16_utc() {
        assert_eq!(beijing_date(utc(2026, 10, 7, 15, 59)), date(2026, 10, 7));
        assert_eq!(beijing_date(utc(2026, 10, 7, 16, 0)), date(2026, 10, 8));
    }

    #[test]
    fn biz_day_switches_at_4am_beijing() {
        // 北京时间 10-08 03:59 → 仍是 10-07；04:00 → 10-08。
        assert_eq!(biz_day(utc(2026, 10, 7, 19, 59)), date(2026, 10, 7));
        assert_eq!(biz_day(utc(2026, 10, 7, 20, 0)), date(2026, 10, 8));
    }

    #[test]
    fn biz_day_range_is_24h_and_contains_input() {
        let at = utc(2026, 10, 7, 12, 34);
        let (start, end) = biz_day_range(at);
        assert_eq!(start, utc(2026, 10, 6, 20, 0));
        assert_eq!(end - start, Duration::days(1));
        assert!(start <= at && at < end);
    }
}
