use std::sync::Arc;

use ik_kernel::bus::EventBus;
use ik_media::MediaState;
use sqlx::PgPool;

#[derive(Clone)]
pub struct NotifyState {
    pub db: PgPool,
    pub bus: Arc<dyn EventBus>,
    /// 发送者、私信对方的头像。
    pub media: MediaState,
}

impl NotifyState {
    pub fn new(db: PgPool, bus: Arc<dyn EventBus>, media: MediaState) -> Self {
        Self { db, bus, media }
    }
}
