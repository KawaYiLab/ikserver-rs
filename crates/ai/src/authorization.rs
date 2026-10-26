//! 渠道提供的可撤销授权；AI 不认识渠道表，只在敏感操作与写事务内复核。
use ik_kernel::Problem;
use sqlx::PgConnection;
use std::{fmt::Debug, future::Future, pin::Pin, sync::Arc};

pub type CheckFuture<'a> = Pin<Box<dyn Future<Output = Result<bool, Problem>> + Send + 'a>>;

pub trait Authorization: Debug + Send + Sync {
    /// 写入调用方必须传自己的事务连接，让授权与副作用使用相同的锁生命周期。
    fn check<'a>(&'a self, conn: &'a mut PgConnection) -> CheckFuture<'a>;
}

pub type Guard = Option<Arc<dyn Authorization>>;

pub async fn check(conn: &mut PgConnection, guard: &Guard) -> Result<bool, Problem> {
    match guard {
        Some(guard) => guard.check(conn).await,
        None => Ok(true),
    }
}

pub(crate) async fn require(conn: &mut PgConnection, guard: &Guard) -> Result<(), Problem> {
    if check(conn, guard).await? {
        Ok(())
    } else {
        Err(Problem::new(
            http::StatusCode::FORBIDDEN,
            "AUTHORIZATION_REVOKED",
            "对话授权已变更",
        ))
    }
}
