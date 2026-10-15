//! 给其他业务域用的评论数据接口（不直接读 `comments` 表，AGENTS.md 规则 5）。

use chrono::{DateTime, Utc};
use ik_content::interop::Activity;
use ik_kernel::Problem;
use sqlx::PgConnection;

use crate::repo;

/// 窗口内的有效评论行为（热榜计算用）：每个帖子、每个评论者取最早一次。
/// 匿名评论照样计入（行为人是真实作者，只参与计数，不会对外展示）；
/// 行为人是否有效、是不是帖子作者本人由调用方过滤。
pub async fn comment_activity(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Vec<Activity>, Problem> {
    Ok(repo::comment_activity(conn, since, until)
        .await?
        .into_iter()
        .map(|row| Activity {
            post_id: row.post_id,
            user_id: row.user_id,
            first_at: row.first_at,
        })
        .collect())
}
