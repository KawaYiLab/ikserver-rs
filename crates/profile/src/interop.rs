//! 给其他域用的接口。

use ik_kernel::Problem;
use sqlx::PgConnection;

use crate::repo;

/// 注销账号时清空个人资料：签名、主页精选，并隐藏主页（DEL-006）。在调用方的注销事务里执行。
pub async fn clear_for_deletion(conn: &mut PgConnection, user_id: i64) -> Result<(), Problem> {
    repo::clear(conn, user_id).await?;
    Ok(())
}
