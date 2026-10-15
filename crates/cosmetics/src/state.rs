use ik_media::MediaState;
use sqlx::PgPool;

#[derive(Clone)]
pub struct CosmeticsState {
    pub db: PgPool,
    pub ids: std::sync::Arc<ik_kernel::Snowflake>,
    /// 目录图片的展示数据。
    pub media: MediaState,
}

impl CosmeticsState {
    pub fn new(db: PgPool, ids: std::sync::Arc<ik_kernel::Snowflake>, media: MediaState) -> Self {
        Self { db, ids, media }
    }
}
