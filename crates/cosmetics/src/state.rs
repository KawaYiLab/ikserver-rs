use std::sync::Arc;
use std::time::Duration;

use ik_media::MediaState;
use moka::future::Cache;
use sqlx::PgPool;

use crate::dto::EmoteManifest;

/// 表情清单的进程内缓存。本实例的后台修改会立即让它失效；其他实例最多晚这么久。
const EMOTE_MANIFEST_TTL: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct CosmeticsState {
    pub db: PgPool,
    pub ids: Arc<ik_kernel::Snowflake>,
    /// 目录图片的展示数据。
    pub media: MediaState,
    pub(crate) emotes: Cache<(), Arc<EmoteManifest>>,
}

impl CosmeticsState {
    pub fn new(db: PgPool, ids: Arc<ik_kernel::Snowflake>, media: MediaState) -> Self {
        Self {
            db,
            ids,
            media,
            emotes: Cache::builder()
                .max_capacity(1)
                .time_to_live(EMOTE_MANIFEST_TTL)
                .build(),
        }
    }
}
