use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use sqlx::PgPool;

/// 拉黑 / 取消拉黑：每个用户每分钟 10 次、每天 100 次（BLOCK-006）。
const BLOCKS_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(10) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};
const BLOCKS_PER_DAY: NonZeroU32 = match NonZeroU32::new(100) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};
/// 日额度按令牌桶实现：容量 100，每 864 秒（一天 / 100）回 1 个。
const DAY_REFILL: Duration = Duration::from_secs(86_400 / 100);

#[derive(Clone)]
pub struct SocialState {
    pub db: PgPool,
    pub(crate) block_limits: Arc<BlockLimits>,
}

pub(crate) struct BlockLimits {
    pub per_minute: DefaultKeyedRateLimiter<i64>,
    pub per_day: DefaultKeyedRateLimiter<i64>,
}

impl SocialState {
    pub fn new(db: PgPool) -> Self {
        let per_day = Quota::with_period(DAY_REFILL)
            .map(|q| q.allow_burst(BLOCKS_PER_DAY))
            .unwrap_or_else(|| Quota::per_hour(BLOCKS_PER_MINUTE));
        Self {
            db,
            block_limits: Arc::new(BlockLimits {
                per_minute: RateLimiter::keyed(Quota::per_minute(BLOCKS_PER_MINUTE)),
                per_day: RateLimiter::keyed(per_day),
            }),
        }
    }
}
