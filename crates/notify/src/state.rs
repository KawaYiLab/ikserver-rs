use std::sync::Arc;

use ik_kernel::bus::EventBus;
use sqlx::PgPool;

#[derive(Clone)]
pub struct NotifyState {
    pub db: PgPool,
    pub bus: Arc<dyn EventBus>,
}

impl NotifyState {
    pub fn new(db: PgPool, bus: Arc<dyn EventBus>) -> Self {
        Self { db, bus }
    }
}
