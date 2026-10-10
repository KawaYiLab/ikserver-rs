//! 给其他业务域（评论、点赞、投币……）用的帖子接口。
//!
//! 函数都接收调用方的连接/事务，这样对方的写入与帖子计数、顶帖、采纳投影在同一个事务里提交
//! （AGENTS.md 规则 6），同时不需要直接读写 `posts` 表（规则 5）。

use chrono::{DateTime, Utc};
use ik_kernel::Problem;
use sqlx::PgConnection;

use crate::repo;

/// 帖子在其他域眼里的样子。
#[derive(Debug, Clone)]
pub struct PostRef {
    pub id: i64,
    pub author_id: i64,
    pub is_anonymous: bool,
    /// 已发布且未隐藏。
    pub is_public: bool,
    /// 所在分区允许采纳评论（没有分区视为不允许，CMT-039）。
    pub allow_comment_accept: bool,
}

impl From<repo::PostRefRow> for PostRef {
    fn from(row: repo::PostRefRow) -> Self {
        Self {
            id: row.id,
            author_id: row.author_id,
            is_anonymous: row.is_anonymous,
            is_public: row.is_public,
            allow_comment_accept: row.allow_comment_accept,
        }
    }
}

impl PostRef {
    /// 读者能否看到这篇帖子：公开，或者是作者本人（作者能看到自己被隐藏的帖子）。
    pub fn readable_by(&self, viewer_id: Option<i64>) -> bool {
        self.is_public || viewer_id == Some(self.author_id)
    }
}

pub async fn post(conn: &mut PgConnection, post_id: i64) -> Result<Option<PostRef>, Problem> {
    Ok(repo::post_ref(conn, post_id).await?.map(PostRef::from))
}

/// 锁住帖子行。同一帖子下的评论写入、置顶、采纳因此串行化。
pub async fn lock_post(conn: &mut PgConnection, post_id: i64) -> Result<Option<PostRef>, Problem> {
    Ok(repo::lock_post_ref(conn, post_id).await?.map(PostRef::from))
}

/// 新评论：评论数 +1，并把帖子顶到信息流前面。
pub async fn comment_added(
    conn: &mut PgConnection,
    post_id: i64,
    commenter_id: i64,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    Ok(repo::comment_added(conn, post_id, commenter_id, now).await?)
}

/// 删除了 `n` 条评论（含级联删除的回复）。
pub async fn comments_removed(
    conn: &mut PgConnection,
    post_id: i64,
    n: i32,
) -> Result<(), Problem> {
    Ok(repo::comments_removed(conn, post_id, n).await?)
}

pub async fn mark_solved(
    conn: &mut PgConnection,
    post_id: i64,
    comment_id: i64,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    Ok(repo::mark_solved(conn, post_id, comment_id, now).await?)
}

/// 帖子当前采纳的正是 `comment_id` 时清除已解决状态。
pub async fn clear_solved(
    conn: &mut PgConnection,
    post_id: i64,
    comment_id: i64,
) -> Result<(), Problem> {
    Ok(repo::clear_solved(conn, post_id, comment_id).await?)
}
