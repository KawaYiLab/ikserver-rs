//! 每日加权 token 额度（QUOTA 第 5 节）：预占 → 结算，计数放在 PostgreSQL。
//!
//! 每个配额日（北京时间 04:00 切换，QUOTA-004）每人一个计数器，`user_id = 0` 是全站。计数含在飞的
//! 预占，所以并发请求在计数器行锁上串行（QUOTA-005）。加锁顺序固定为「先全站行、再个人行」，
//! 预占、结算、回收都遵守，避免死锁。
//!
//! 函数都接收调用方的连接/事务：结算与回复定稿同一事务提交（QUOTA-017 的回执在新设计里就是这个事务）。

use chrono::{DateTime, Duration, NaiveDate, Utc};
use ik_kernel::{Problem, Snowflake};
use sqlx::PgConnection;

use crate::repo;

/// 全站计数器的 user_id。
pub const SITE: i64 = 0;
/// 结算去重标记保留时长（QUOTA-015）。
pub const SETTLEMENT_TTL: Duration = Duration::hours(48);
/// 每份预占估算里，一次工具调用回灌的 token（QUOTA-007）。
pub const TOOL_FEEDBACK_TOKENS: i64 = 500;
/// 每份预占估算里，回复正文的 token（QUOTA-007）。
pub const REPLY_TOKENS: i64 = 1500;
/// 计数器保留天数（额度条只读当天；留一段供排障）。
pub const COUNTER_RETENTION_DAYS: i64 = 90;

/// 额度上限（加权 token；≤0 表示不限制，QUOTA-003）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub global_enabled: bool,
    pub per_user: i64,
    pub site: i64,
}

impl Limits {
    fn unlimited(self) -> bool {
        self.per_user <= 0 && self.site <= 0
    }
}

/// 预占被拒的原因（QUOTA-009、TRIG-001）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    GlobalDisabled,
    UserBudget,
    SiteBudget,
}

impl Denial {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GlobalDisabled => "global_disabled",
            Self::UserBudget => "daily_token_budget_exceeded",
            Self::SiteBudget => "site_token_budget_exceeded",
        }
    }
}

/// 一份预占。`id` 为 None：两道闸门都没配上限，没有登记（QUOTA-006），结算时照实累加。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hold {
    pub id: Option<i64>,
    pub user_id: i64,
    pub day: NaiveDate,
    pub amount: i64,
}

/// 合法倍率：非有限值或 ≤0 一律按 1（QUOTA-001，后台漏填不能免费）。
pub fn sane_weight(weight: f64) -> f64 {
    if weight.is_finite() && weight > 0.0 {
        weight
    } else {
        1.0
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn scale(tokens: i64, weight: f64) -> i64 {
    let scaled = (tokens as f64 * sane_weight(weight)).round();
    if scaled >= i64::MAX as f64 {
        i64::MAX
    } else {
        scaled as i64
    }
}

/// 一次回复计入的量：总量为 0 记 0，有用量至少记 1（QUOTA-001）。
pub fn weighted(tokens: i64, weight: f64) -> i64 {
    if tokens <= 0 {
        0
    } else {
        scale(tokens, weight).max(1)
    }
}

/// 预占量（QUOTA-007）：上下文基数 + 每轮工具回灌 + 回复正文 + 提示字符数，乘倍率，至少 1。
pub fn estimate(context_tokens: i64, tool_rounds: u32, prompt_chars: i64, weight: f64) -> i64 {
    let tokens = context_tokens
        .max(0)
        .saturating_add(i64::from(tool_rounds) * TOOL_FEEDBACK_TOKENS)
        .saturating_add(REPLY_TOKENS)
        .saturating_add(prompt_chars.max(0));
    scale(tokens, weight).max(1)
}

/// 文本的 token 估算（GEN-082）：CJK、假名、谚文 1 字 1 token，其余每 4 字符 1 token（向上取整）。
pub fn estimate_text_tokens(text: &str) -> i64 {
    let mut wide = 0i64;
    let mut other = 0i64;
    for c in text.chars() {
        let cp = u32::from(c);
        let is_wide = (0x3040..=0x30FF).contains(&cp)
            || (0x3400..=0x4DBF).contains(&cp)
            || (0x4E00..=0x9FFF).contains(&cp)
            || (0xAC00..=0xD7AF).contains(&cp)
            || (0xF900..=0xFAFF).contains(&cp)
            || (0xFF66..=0xFF9F).contains(&cp);
        if is_wide {
            wide += 1;
        } else {
            other += 1;
        }
    }
    wide + (other + 3) / 4
}

/// 下一次日切的时刻（`resetAt`，QUOTA-004）。
pub fn reset_at(now: DateTime<Utc>) -> DateTime<Utc> {
    let today = ik_kernel::time::biz_day(now);
    ik_kernel::time::biz_day_start(today + Duration::days(1))
}

/// 预占（QUOTA-005～010）。被拒时两个计数器都已就地退回。必须在事务里调用。
pub async fn reserve(
    conn: &mut PgConnection,
    ids: &Snowflake,
    limits: Limits,
    user_id: i64,
    amount: i64,
    now: DateTime<Utc>,
    hold_ttl: Duration,
) -> Result<Result<Hold, Denial>, Problem> {
    if !limits.global_enabled {
        return Ok(Err(Denial::GlobalDisabled));
    }
    let day = ik_kernel::time::biz_day(now);
    if limits.unlimited() {
        return Ok(Ok(Hold {
            id: None,
            user_id,
            day,
            amount: 0,
        }));
    }
    let amount = amount.max(1);
    let site_before = repo::add_used(conn, day, SITE, amount).await? - amount;
    let user_before = repo::add_used(conn, day, user_id, amount).await? - amount;
    // 恰好等于上限即拒；进来时没到上限就放行（当天最后一条允许超出）。
    let denial = if limits.per_user > 0 && user_before >= limits.per_user {
        Some(Denial::UserBudget)
    } else if limits.site > 0 && site_before >= limits.site {
        Some(Denial::SiteBudget)
    } else {
        None
    };
    if let Some(denial) = denial {
        repo::add_used(conn, day, SITE, -amount).await?;
        repo::add_used(conn, day, user_id, -amount).await?;
        return Ok(Err(denial));
    }
    let id = ids.next_raw();
    repo::insert_hold(conn, id, user_id, day, amount, now + hold_ttl, now).await?;
    Ok(Ok(Hold {
        id: Some(id),
        user_id,
        day,
        amount,
    }))
}

/// 结算的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    /// 按 `delta` 补扣（正）或退回（负）。
    Applied { delta: i64 },
    /// 同一个键已经结算过（QUOTA-015）。
    Duplicate,
}

/// 结算（QUOTA-012～015）：按 `实际 − 预占` 多退少补，回到预占当时那个配额日。
///
/// `execution_key` 是没有登记的预占（无上限）时的去重键（`<任务 id>:<第几次尝试>`）。
/// 预占已被回收（过期）时按实际用量完整扣一次（QUOTA-016）。必须在事务里调用。
pub async fn settle(
    conn: &mut PgConnection,
    hold: &Hold,
    execution_key: &str,
    actual: i64,
    now: DateTime<Utc>,
) -> Result<Settled, Problem> {
    let actual = actual.max(0);
    let key = hold.id.map_or_else(
        || format!("exec:{execution_key}"),
        |id| format!("hold:{id}"),
    );
    if !repo::mark_settled(conn, &key, now).await? {
        return Ok(Settled::Duplicate);
    }
    let delta = match hold.id {
        None => actual,
        Some(id) => match repo::take_hold(conn, id).await? {
            Some(row) if row.user_id != hold.user_id => {
                // 不动账（QUOTA-014）；调用方的事务回滚会连同去重标记一起撤销。
                return Err(Problem::internal_from(&QuotaOwnerMismatch));
            }
            Some(row) => actual - row.amount,
            None => actual,
        },
    };
    if delta != 0 {
        repo::add_used(conn, hold.day, SITE, delta).await?;
        repo::add_used(conn, hold.day, hold.user_id, delta).await?;
    }
    Ok(Settled::Applied { delta })
}

#[derive(Debug, thiserror::Error)]
#[error("quota reservation owner mismatch")]
struct QuotaOwnerMismatch;

/// 回收一批过期未结算的预占，整份退回（QUOTA-016）。返回回收的份数。
pub async fn reclaim_expired(
    conn: &mut PgConnection,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<usize, Problem> {
    let mut holds = repo::take_expired_holds(conn, now, limit).await?;
    // 加锁顺序：先全站行（按日）、再个人行（按日、按人）。
    let mut site: std::collections::BTreeMap<NaiveDate, i64> = std::collections::BTreeMap::new();
    for hold in &holds {
        *site.entry(hold.day).or_default() += hold.amount;
    }
    for (day, amount) in &site {
        repo::add_used(conn, *day, SITE, -amount).await?;
    }
    holds.sort_by_key(|h| (h.day, h.user_id));
    // 回收不写去重标记：迟到的真实结算认领不到预占，按实际用量完整扣一次（见 `settle`）。
    for hold in &holds {
        repo::add_used(conn, hold.day, hold.user_id, -hold.amount).await?;
    }
    Ok(holds.len())
}

/// 清理过期的去重标记与很久以前的计数器。
pub async fn prune(conn: &mut PgConnection, now: DateTime<Utc>) -> Result<(), Problem> {
    repo::prune_settlements(conn, now - SETTLEMENT_TTL).await?;
    let before = ik_kernel::time::biz_day(now) - Duration::days(COUNTER_RETENTION_DAYS);
    repo::prune_counters(conn, before).await?;
    Ok(())
}

/// 额度条读数（QUOTA-031：含在飞预占，不参与放行判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub used: i64,
    pub day: NaiveDate,
}

pub async fn read(
    conn: &mut PgConnection,
    user_id: i64,
    now: DateTime<Utc>,
) -> Result<Reading, Problem> {
    let day = ik_kernel::time::biz_day(now);
    let used = repo::used(conn, day, user_id).await?.unwrap_or(0).max(0);
    Ok(Reading { used, day })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use chrono::TimeZone;

    use super::*;

    #[test]
    fn weighting_follows_quota_001() {
        assert_eq!(weighted(0, 2.0), 0);
        assert_eq!(weighted(1000, 1.5), 1500);
        assert_eq!(weighted(1, 0.01), 1);
        assert_eq!(weighted(1000, f64::NAN), 1000);
        assert_eq!(weighted(1000, -3.0), 1000);
        assert_eq!(weighted(1000, 0.0), 1000);
    }

    #[test]
    fn estimate_follows_quota_007() {
        assert_eq!(estimate(4000, 2, 0, 1.0), 6500);
        assert_eq!(estimate(4000, 2, 100, 2.0), 13200);
        assert_eq!(estimate(0, 0, 0, 0.0001), 1);
    }

    #[test]
    fn text_estimate_counts_cjk_per_char() {
        assert_eq!(estimate_text_tokens(""), 0);
        assert_eq!(estimate_text_tokens("绳网"), 2);
        assert_eq!(estimate_text_tokens("abcde"), 2);
        assert_eq!(estimate_text_tokens("绳网abcd"), 3);
        assert_eq!(estimate_text_tokens("カタ한"), 3);
    }

    #[test]
    fn reset_is_next_0400_beijing() {
        // 北京时间 2026-10-15 03:59:59 → 当天 04:00 重置。
        let before = Utc.with_ymd_and_hms(2026, 10, 14, 19, 59, 59).unwrap();
        assert_eq!(
            reset_at(before),
            Utc.with_ymd_and_hms(2026, 10, 14, 20, 0, 0).unwrap()
        );
        // 恰好 04:00 → 次日 04:00。
        let at = Utc.with_ymd_and_hms(2026, 10, 14, 20, 0, 0).unwrap();
        assert_eq!(
            reset_at(at),
            Utc.with_ymd_and_hms(2026, 10, 15, 20, 0, 0).unwrap()
        );
    }
}
