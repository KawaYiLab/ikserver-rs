use std::sync::Arc;

use ik_kernel::Snowflake;
use ik_kernel::config::Exam;
use sqlx::PgPool;

#[derive(Clone)]
pub struct ExamState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub settings: Arc<Exam>,
}

impl ExamState {
    pub fn new(db: PgPool, ids: Arc<Snowflake>, settings: Exam) -> Self {
        Self {
            db,
            ids,
            settings: Arc::new(settings),
        }
    }
}
