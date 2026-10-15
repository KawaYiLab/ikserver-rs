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
    /// 当前标题（通知快照用）。
    pub title: String,
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
            title: row.title,
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

/// `ids` 中公开（已发布且未隐藏）的帖子 → 标题。作者主页的评论列表用它过滤并展示所属帖子。
pub async fn public_post_titles(
    conn: &mut PgConnection,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, String>, Problem> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    Ok(repo::public_posts(conn, ids)
        .await?
        .into_iter()
        .map(|row| (row.id, row.title))
        .collect())
}

// ---------- 热榜数据（feed 域计算热度用）----------

/// 热度计算需要的帖子字段。
#[derive(Debug, Clone)]
pub struct HotPost {
    pub id: i64,
    pub author_id: i64,
    pub is_anonymous: bool,
    /// 首次发布时间（编辑重发不重置衰减，EDGE-016）。
    pub created_at: DateTime<Utc>,
    pub bumped_at: DateTime<Utc>,
    pub like_count: i32,
}

impl From<repo::HotPostRow> for HotPost {
    fn from(row: repo::HotPostRow) -> Self {
        Self {
            id: row.id,
            author_id: row.author_id,
            is_anonymous: row.is_anonymous,
            created_at: row.created_at,
            bumped_at: row.bumped_at,
            like_count: row.like_count,
        }
    }
}

/// 窗口内的一次行为：某人第一次在某帖上点赞 / 评论的时间。
#[derive(Debug, Clone, Copy)]
pub struct Activity {
    pub post_id: i64,
    pub user_id: i64,
    pub first_at: DateTime<Utc>,
}

/// 最近活跃的公开帖子，最多 `limit` 篇（HOT-004）。
pub async fn hot_recent_posts(
    conn: &mut PgConnection,
    limit: i64,
) -> Result<Vec<HotPost>, Problem> {
    Ok(repo::hot_recent(conn, limit)
        .await?
        .into_iter()
        .map(HotPost::from)
        .collect())
}

/// 指定 id 中公开的帖子。
pub async fn hot_posts(conn: &mut PgConnection, ids: &[i64]) -> Result<Vec<HotPost>, Problem> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(repo::hot_by_ids(conn, ids)
        .await?
        .into_iter()
        .map(HotPost::from)
        .collect())
}

/// 窗口内对帖子的点赞（每个帖子、每人取最早一次）。行为人是否有效、是不是作者本人由调用方过滤。
pub async fn like_activity(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Vec<Activity>, Problem> {
    Ok(repo::like_activity(conn, since, until)
        .await?
        .into_iter()
        .map(|row| Activity {
            post_id: row.post_id,
            user_id: row.user_id,
            first_at: row.first_at,
        })
        .collect())
}

// ---------- 搜索（search 域用）----------

/// 建索引用的公开帖子。匿名帖的 `author_id` 为 None：真实作者不进入搜索索引（PRIV-005）。
#[derive(Debug, Clone)]
pub struct PostSearchDoc {
    pub id: i64,
    pub author_id: Option<i64>,
    pub title: String,
    /// Markdown 原文，调用方负责转纯文本。
    pub body: String,
    pub category_slug: Option<String>,
    pub category_name: Option<String>,
    pub view_count: i64,
    pub like_count: i32,
    pub comment_count: i32,
    pub bumped_at: DateTime<Utc>,
}

impl From<repo::SearchDocRow> for PostSearchDoc {
    fn from(row: repo::SearchDocRow) -> Self {
        Self {
            id: row.id,
            author_id: (!row.is_anonymous).then_some(row.author_id),
            title: row.title,
            body: row.body,
            category_slug: row.category_slug,
            category_name: row.category_name,
            view_count: row.view_count,
            like_count: row.like_count,
            comment_count: row.comment_count,
            bumped_at: row.bumped_at,
        }
    }
}

/// `ids` 中公开的帖子（不公开的不出现，调用方据此删除索引文档）。
pub async fn search_docs(
    conn: &mut PgConnection,
    ids: &[i64],
) -> Result<Vec<PostSearchDoc>, Problem> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(repo::search_docs_by_ids(conn, ids)
        .await?
        .into_iter()
        .map(PostSearchDoc::from)
        .collect())
}

/// 全部公开帖子，按 id 升序分批。
pub async fn search_docs_after(
    conn: &mut PgConnection,
    after_id: i64,
    limit: i64,
) -> Result<Vec<PostSearchDoc>, Problem> {
    Ok(repo::search_docs_after(conn, after_id, limit)
        .await?
        .into_iter()
        .map(PostSearchDoc::from)
        .collect())
}

/// `since` 之后改动过的帖子 id（任意状态），最多 `limit` 个，按改动时间升序。
pub async fn changed_post_ids(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<i64>, Problem> {
    Ok(repo::changed_post_ids(conn, since, limit).await?)
}

/// 公开帖子的作者与标题（评论搜索的可见性与拉黑判断用）。
#[derive(Debug, Clone)]
pub struct PostMeta {
    pub author_id: i64,
    pub is_anonymous: bool,
    pub title: String,
}

pub async fn public_post_meta(
    conn: &mut PgConnection,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, PostMeta>, Problem> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    Ok(repo::public_post_meta(conn, ids)
        .await?
        .into_iter()
        .map(|row| {
            (
                row.id,
                PostMeta {
                    author_id: row.author_id,
                    is_anonymous: row.is_anonymous,
                    title: row.title,
                },
            )
        })
        .collect())
}
