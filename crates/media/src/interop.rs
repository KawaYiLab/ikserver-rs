//! 给其他业务域用的媒体接口（不直接读 `media_*` 表，AGENTS.md 规则 5）。

use ik_kernel::Problem;
use sqlx::PgConnection;

use crate::repo;

/// `ids` 中由 `user_id` 上传过的文件 id（引用媒体时的归属校验，D-012 方案 A）。
/// 顺序不保证；调用方据此判断哪些引用不合法。
pub async fn owned_media(
    conn: &mut PgConnection,
    user_id: i64,
    ids: &[i64],
) -> Result<Vec<i64>, Problem> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(repo::owned_among(conn, user_id, ids)
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect())
}
