//! 数据库连接池与常用错误判别。

use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use crate::config::Database;

pub async fn connect(config: &Database) -> Result<PgPool, sqlx::Error> {
    let options: PgConnectOptions = config.url.expose().parse()?;
    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(Duration::from_secs(config.acquire_timeout_secs))
        .connect_with(options.application_name("ik-server"))
        .await
}

/// 是否为指定唯一约束的冲突（23505）。用于把约束冲突映射成友好的业务错误。
pub fn is_unique_violation(error: &sqlx::Error, constraint: &str) -> bool {
    match error {
        sqlx::Error::Database(db) => {
            db.code().as_deref() == Some("23505") && db.constraint() == Some(constraint)
        }
        _ => false,
    }
}
