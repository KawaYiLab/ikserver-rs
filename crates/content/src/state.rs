use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use ik_kernel::Snowflake;
use ik_kernel::bus::EventBus;
use ik_media::MediaState;
use moka::future::Cache;
use sqlx::PgPool;

/// 同一观众对同一帖子的浏览量冷却（POST-080）。
const VIEW_COOLDOWN: Duration = Duration::from_secs(5 * 60);
const VIEW_COOLDOWN_CAPACITY: u64 = 200_000;
/// 每个观众每分钟最多计 30 次浏览（POST-080）。
const VIEWS_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(30) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};

#[derive(Clone)]
pub struct ContentState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub bus: Arc<dyn EventBus>,
    /// 封面、头像的展示数据与归属校验。
    pub media: MediaState,
    pub(crate) views: Arc<ViewGuard>,
}

impl ContentState {
    pub fn new(db: PgPool, ids: Arc<Snowflake>, bus: Arc<dyn EventBus>, media: MediaState) -> Self {
        Self {
            db,
            ids,
            bus,
            media,
            views: Arc::new(ViewGuard {
                cooldown: Cache::builder()
                    .max_capacity(VIEW_COOLDOWN_CAPACITY)
                    .time_to_live(VIEW_COOLDOWN)
                    .build(),
                limiter: RateLimiter::keyed(Quota::per_minute(VIEWS_PER_MINUTE)),
            }),
        }
    }
}

pub(crate) struct ViewGuard {
    /// 键 `<观众>|<帖子>`。单进程内存实现；多实例部署时同一观众可能在每个实例各计一次。
    pub cooldown: Cache<String, ()>,
    pub limiter: DefaultKeyedRateLimiter<String>,
}
