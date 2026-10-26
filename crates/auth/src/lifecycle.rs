//! 账号注销的事务内扩展点，由 server 注册上层业务域，auth 不依赖它们。
use chrono::{DateTime, Utc};
use ik_kernel::Problem;
use sqlx::PgConnection;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock},
};

pub type DeleteFuture<'a> = Pin<Box<dyn Future<Output = Result<(), Problem>> + Send + 'a>>;

pub trait AccountLifecycle: Send + Sync {
    fn deleting<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> DeleteFuture<'a>;
}

#[derive(Clone, Default)]
pub struct LifecycleHooks(Arc<OnceLock<Arc<dyn AccountLifecycle>>>);

impl LifecycleHooks {
    pub fn install(&self, hook: Arc<dyn AccountLifecycle>) -> bool {
        self.0.set(hook).is_ok()
    }
    pub async fn deleting(
        &self,
        conn: &mut PgConnection,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> Result<(), Problem> {
        if let Some(hook) = self.0.get() {
            hook.deleting(conn, user_id, now).await?;
        }
        Ok(())
    }
}
