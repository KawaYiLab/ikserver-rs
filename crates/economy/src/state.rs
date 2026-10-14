use std::sync::Arc;

use ik_kernel::Snowflake;
use sqlx::PgPool;

#[derive(Clone)]
pub struct EconomyState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
}

impl EconomyState {
    pub fn new(db: PgPool, ids: Arc<Snowflake>) -> Self {
        Self { db, ids }
    }
}
