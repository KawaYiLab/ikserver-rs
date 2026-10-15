//! 给其他业务域用的拉黑判断。
//!
//! 函数都接收调用方的连接/事务，这样判断与对方的写入在同一个事务里，
//! 同时不需要直接读 `user_blocks` 表（AGENTS.md 规则 5）。
//!
//! 隐私红线（规则 10）：匿名内容一律不按拉黑过滤、不因拉黑拒绝操作，
//! 否则「内容消失 / 操作被拒」会反推出匿名作者。是否匿名由调用方判断，匿名时不要调用这里。

use http::StatusCode;
use ik_kernel::Problem;
use sqlx::PgConnection;

use crate::repo;

/// 双方存在拉黑关系时的通用错误。不说明是谁拉黑了谁。
pub fn user_blocked() -> Problem {
    Problem::new(
        StatusCode::FORBIDDEN,
        "USER_BLOCKED",
        "你与对方存在拉黑关系，无法进行此操作",
    )
}

/// 两人之间任一方向存在拉黑（BLOCK-002）。同一个人返回 false。
pub async fn any_block(conn: &mut PgConnection, a: i64, b: i64) -> Result<bool, Problem> {
    if a == b {
        return Ok(false);
    }
    Ok(repo::any_block(conn, a, b).await?)
}

/// 存在拉黑时返回 403 `USER_BLOCKED`（点赞、收藏、投币、评论等，BLOCK-015/016）。
pub async fn ensure_no_block(conn: &mut PgConnection, a: i64, b: i64) -> Result<(), Problem> {
    if any_block(conn, a, b).await? {
        Err(user_blocked())
    } else {
        Ok(())
    }
}

/// 新帖扇出的候选收件人：`user_id` 的粉丝，去掉与其有任一方向拉黑的人（FOLLOW-017）。
/// 拉黑本来就会删除关注，这里是兜底。
pub async fn followers_for_fanout(
    conn: &mut PgConnection,
    user_id: i64,
) -> Result<Vec<i64>, Problem> {
    Ok(repo::followers_without_blocks(conn, user_id).await?)
}

/// 列表过滤用的排除集合：「我拉黑的人 ∪ 拉黑我的人」（COMMON-008）。未登录时为空。
/// 调用方在 SQL 里写成 `(is_anonymous OR NOT (author_id = ANY($excluded)))`。
pub async fn excluded_authors(
    conn: &mut PgConnection,
    viewer_id: Option<i64>,
) -> Result<Vec<i64>, Problem> {
    match viewer_id {
        Some(id) => Ok(repo::related_by_blocks(conn, id).await?),
        None => Ok(Vec::new()),
    }
}
