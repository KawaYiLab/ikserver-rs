use std::num::NonZeroU32;
use std::sync::Arc;

use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use ik_kernel::Snowflake;
use ik_kernel::bus::EventBus;
use sqlx::PgPool;

/// 每个用户每分钟最多发 10 条评论（D-037 方案 A；具体数值待产品确认）。
const COMMENTS_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(10) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};

#[derive(Clone)]
pub struct CommentsState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub bus: Arc<dyn EventBus>,
    pub(crate) limiter: Arc<DefaultKeyedRateLimiter<i64>>,
}

impl CommentsState {
    pub fn new(db: PgPool, ids: Arc<Snowflake>, bus: Arc<dyn EventBus>) -> Self {
        Self {
            db,
            ids,
            bus,
            limiter: Arc::new(RateLimiter::keyed(Quota::per_minute(COMMENTS_PER_MINUTE))),
        }
    }
}
