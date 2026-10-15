use std::num::NonZeroU32;
use std::sync::Arc;

use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use ik_kernel::Snowflake;
use ik_kernel::bus::EventBus;
use ik_media::MediaState;
use sqlx::PgPool;

/// 发消息：每个用户每秒 5 条、每分钟 60 条（MSG-020）。
const SENDS_PER_SECOND: NonZeroU32 = match NonZeroU32::new(5) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};
const SENDS_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(60) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};

#[derive(Clone)]
pub struct MessagingState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub bus: Arc<dyn EventBus>,
    /// 对方头像。
    pub media: MediaState,
    pub(crate) send_limits: Arc<SendLimits>,
}

pub(crate) struct SendLimits {
    pub per_second: DefaultKeyedRateLimiter<i64>,
    pub per_minute: DefaultKeyedRateLimiter<i64>,
}

impl MessagingState {
    pub fn new(db: PgPool, ids: Arc<Snowflake>, bus: Arc<dyn EventBus>, media: MediaState) -> Self {
        Self {
            db,
            ids,
            bus,
            media,
            send_limits: Arc::new(SendLimits {
                per_second: RateLimiter::keyed(Quota::per_second(SENDS_PER_SECOND)),
                per_minute: RateLimiter::keyed(Quota::per_minute(SENDS_PER_MINUTE)),
            }),
        }
    }
}
