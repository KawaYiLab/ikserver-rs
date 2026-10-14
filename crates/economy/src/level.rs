//! 等级与经验数值（EXP-001/009）。纯函数。

/// Lv1…Lv7 的经验下限。
pub const LEVEL_THRESHOLDS: [i64; 7] = [0, 500, 2000, 6000, 15000, 35000, 80000];
pub const MAX_LEVEL: i16 = 7;
/// 主动行为每日上限（签到、发帖、评论、点赞、被采纳）。
pub const ACTIVE_DAILY_CAP: i32 = 50;
/// 被动互动每日上限（被投币等）。
pub const PASSIVE_DAILY_CAP: i32 = 1000;

/// 按经验算等级（1–7）。负数按 Lv1。
pub fn level_for_exp(exp: i64) -> i16 {
    let reached = LEVEL_THRESHOLDS.iter().filter(|&&t| exp >= t).count();
    i16::try_from(reached.max(1)).unwrap_or(MAX_LEVEL)
}

/// 实际应存的等级：没有考试豁免的普通用户固定为 0（EXP-002），否则按经验算。
pub fn effective_level(exp: i64, exam_exempt: bool) -> i16 {
    if exam_exempt { level_for_exp(exp) } else { 0 }
}

/// 下一级的经验门槛；已满级时为 None。
pub fn next_level_exp(level: i16) -> Option<i64> {
    let index = usize::try_from(level).ok()?;
    LEVEL_THRESHOLDS.get(index).copied()
}

/// 签到经验：6 + min(max(连签-1, 0), 4)，即 6–10（CHECKIN-004）。
pub fn check_in_exp(streak: i32) -> i32 {
    6 + (streak - 1).clamp(0, 4)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn thresholds_are_inclusive() {
        assert_eq!(level_for_exp(-5), 1);
        assert_eq!(level_for_exp(0), 1);
        assert_eq!(level_for_exp(499), 1);
        assert_eq!(level_for_exp(500), 2);
        assert_eq!(level_for_exp(1999), 2);
        assert_eq!(level_for_exp(2000), 3);
        assert_eq!(level_for_exp(79_999), 6);
        assert_eq!(level_for_exp(80_000), 7);
        assert_eq!(level_for_exp(i64::MAX), 7);
    }

    #[test]
    fn non_exempt_users_stay_at_zero() {
        assert_eq!(effective_level(100_000, false), 0);
        assert_eq!(effective_level(0, true), 1);
    }

    #[test]
    fn next_level_exp_matches_thresholds() {
        assert_eq!(next_level_exp(0), Some(0));
        assert_eq!(next_level_exp(1), Some(500));
        assert_eq!(next_level_exp(6), Some(80_000));
        assert_eq!(next_level_exp(7), None);
    }

    #[test]
    fn check_in_exp_grows_to_ten() {
        assert_eq!(check_in_exp(1), 6);
        assert_eq!(check_in_exp(2), 7);
        assert_eq!(check_in_exp(5), 10);
        assert_eq!(check_in_exp(365), 10);
        assert_eq!(check_in_exp(0), 6);
    }

    proptest! {
        #[test]
        fn level_is_monotonic(a in 0i64..200_000, b in 0i64..200_000) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(level_for_exp(lo) <= level_for_exp(hi));
            prop_assert!((1..=MAX_LEVEL).contains(&level_for_exp(a)));
        }
    }
}
