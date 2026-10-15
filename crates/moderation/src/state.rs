use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use ik_kernel::Snowflake;
use ik_kernel::bus::EventBus;
use ik_media::MediaState;
use sqlx::PgPool;

/// 每人每分钟最多 3 次、每天最多 20 次举报（RPT-040）。
const REPORTS_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(3) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};
const REPORTS_PER_DAY: NonZeroU32 = match NonZeroU32::new(20) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};
/// 日额度按令牌桶实现：容量 20，每 72 分钟（一天 / 20）回 1 个。
const DAY_REFILL: Duration = Duration::from_secs(86_400 / 20);

#[derive(Clone)]
pub struct ModerationState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub bus: Arc<dyn EventBus>,
    /// 快照里图片的展示数据。
    pub media: MediaState,
    pub auto_hide_threshold: i64,
    pub(crate) limits: Arc<Limits>,
}

pub(crate) struct Limits {
    pub per_minute: DefaultKeyedRateLimiter<i64>,
    pub per_day: DefaultKeyedRateLimiter<i64>,
}

impl ModerationState {
    pub fn new(
        db: PgPool,
        ids: Arc<Snowflake>,
        bus: Arc<dyn EventBus>,
        media: MediaState,
        auto_hide_threshold: i64,
    ) -> Self {
        let per_day = Quota::with_period(DAY_REFILL)
            .map(|q| q.allow_burst(REPORTS_PER_DAY))
            .unwrap_or_else(|| Quota::per_hour(REPORTS_PER_DAY));
        Self {
            db,
            ids,
            bus,
            media,
            auto_hide_threshold,
            limits: Arc::new(Limits {
                per_minute: RateLimiter::keyed(Quota::per_minute(REPORTS_PER_MINUTE)),
                per_day: RateLimiter::keyed(per_day),
            }),
        }
    }
}
