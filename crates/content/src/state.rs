use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use ik_kernel::Snowflake;
use ik_kernel::bus::EventBus;
use ik_kernel::jobs::Jobs;
use ik_media::MediaState;
use moka::future::Cache;
use sqlx::PgPool;

use crate::bilibili::{Disabled, VideoApi, Videos};

/// 同一观众对同一帖子的浏览量冷却（POST-080）。
const VIEW_COOLDOWN: Duration = Duration::from_secs(5 * 60);
const VIEW_COOLDOWN_CAPACITY: u64 = 200_000;
/// 每个观众每分钟最多计 30 次浏览（POST-080）。
const VIEWS_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(30) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};
/// 视频预览每人每分钟最多 30 次（会访问 B 站）。
const VIDEO_LOOKUPS_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(30) {
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
    pub jobs: Jobs,
    pub(crate) views: Arc<ViewGuard>,
    /// B 站视频信息（带缓存）。默认不访问网络，server 用 [`ContentState::with_video_api`] 接上真实上游。
    pub(crate) videos: Videos,
    pub(crate) video_lookups: Arc<DefaultKeyedRateLimiter<i64>>,
}

impl ContentState {
    pub fn new(db: PgPool, ids: Arc<Snowflake>, bus: Arc<dyn EventBus>, media: MediaState) -> Self {
        Self {
            jobs: Jobs::new(db.clone()),
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
            videos: Videos::new(Arc::new(Disabled)),
            video_lookups: Arc::new(RateLimiter::keyed(Quota::per_minute(
                VIDEO_LOOKUPS_PER_MINUTE,
            ))),
        }
    }

    /// 换上查询 B 站视频信息的上游（生产用 [`crate::bilibili::HttpBilibili`]，测试用假实现）。
    #[must_use]
    pub fn with_video_api(mut self, api: Arc<dyn VideoApi>) -> Self {
        self.videos = Videos::new(api);
        self
    }
}

pub(crate) struct ViewGuard {
    /// 键 `<观众>|<帖子>`。单进程内存实现；多实例部署时同一观众可能在每个实例各计一次。
    pub cooldown: Cache<String, ()>,
    pub limiter: DefaultKeyedRateLimiter<String>,
}

// 执行器的扩展要求 Debug；内部都是连接池与缓存，不值得打印。
impl std::fmt::Debug for ContentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContentState").finish_non_exhaustive()
    }
}
