//! 给其他域用的接口。

use ik_kernel::Problem;
use sqlx::PgConnection;

use crate::repo;

/// 注销账号时清空个人资料：签名、主页精选，并隐藏主页（DEL-006）。在调用方的注销事务里执行。
pub async fn clear_for_deletion(conn: &mut PgConnection, user_id: i64) -> Result<(), Problem> {
    repo::clear(conn, user_id).await?;
    Ok(())
}

/// 签名（没有资料行时为空串）。管理后台账号详情用。
pub async fn bio_of(conn: &mut PgConnection, user_id: i64) -> Result<String, Problem> {
    Ok(repo::get(conn, user_id)
        .await?
        .map(|row| row.bio)
        .unwrap_or_default())
}

/// 管理后台清空违规签名。返回原本是否有签名。
pub async fn clear_bio(conn: &mut PgConnection, user_id: i64) -> Result<bool, Problem> {
    Ok(repo::clear_bio(conn, user_id).await?)
}
