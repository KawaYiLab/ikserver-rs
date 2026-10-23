//! 给其他业务域（评论、点赞、投币……）用的帖子接口。
//!
//! 函数都接收调用方的连接/事务，这样对方的写入与帖子计数、顶帖、采纳投影在同一个事务里提交
//! （AGENTS.md 规则 6），同时不需要直接读写 `posts` 表（规则 5）。

use chrono::{DateTime, Utc};
use ik_kernel::Problem;
use ik_kernel::bus::Effects;
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
    /// 第一张封面（媒体 id）。
    pub cover_id: Option<i64>,
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
                    cover_id: row.cover_id,
                },
            )
        })
        .collect())
}

// ---------- 审核（moderation） ----------

/// 隐藏原因（SANCTION-010）。
pub const HIDDEN_BY_REPORTS: &str = "auto_report";
pub const HIDDEN_BY_MODERATOR: &str = "moderator";

/// 举报目标：帖子（RPT-009、RPT-013）。
#[derive(Debug, Clone)]
pub struct ReportablePost {
    pub author_id: i64,
    pub title: String,
    pub excerpt: String,
    /// 封面（媒体 id，有序）。
    pub cover_ids: Vec<i64>,
    pub is_anonymous: bool,
    /// 已发布且未隐藏。只有公开的帖子能被举报。
    pub is_public: bool,
}

pub async fn reportable_post(
    conn: &mut PgConnection,
    post_id: i64,
) -> Result<Option<ReportablePost>, Problem> {
    Ok(repo::reportable(conn, post_id)
        .await?
        .map(|row| ReportablePost {
            author_id: row.author_id,
            excerpt: crate::service::excerpt(&row.body),
            title: row.title,
            cover_ids: row.cover_ids,
            is_anonymous: row.is_anonymous,
            is_public: row.is_public,
        }))
}

/// 隐藏帖子（同时取消置顶，RPT-053）。已经隐藏的改为新的原因与操作人。帖子不存在时返回 false。
/// 原本公开的帖子会发出 `post.removed`（信息流、搜索、在线客户端据此移除）。
pub async fn hide_post(
    conn: &mut PgConnection,
    post_id: i64,
    by: Option<i64>,
    reason: &str,
    now: DateTime<Utc>,
    effects: &mut Effects,
) -> Result<bool, Problem> {
    let Some(row) = repo::hide(conn, post_id, by, reason, now).await? else {
        return Ok(false);
    };
    if row.was_public {
        effects.publish(crate::service::removed_event(post_id)?);
    }
    Ok(true)
}

/// 取消隐藏，只在当前隐藏原因是 `reason` 时生效（驳回只恢复自动隐藏，RPTADM-060）。返回是否恢复了。
pub async fn unhide_post(
    conn: &mut PgConnection,
    post_id: i64,
    reason: &str,
    effects: &mut Effects,
) -> Result<bool, Problem> {
    let Some((category_id, published)) = repo::unhide(conn, post_id, reason).await? else {
        return Ok(false);
    };
    if published {
        effects.publish(crate::service::published_event(
            post_id,
            None,
            category_id,
            false,
        )?);
    }
    Ok(true)
}

/// 审核删除帖子（评论随外键一起删除）。返回是否删除了。
pub async fn delete_post(
    conn: &mut PgConnection,
    post_id: i64,
    effects: &mut Effects,
) -> Result<bool, Problem> {
    if repo::get_post(conn, post_id).await?.is_none() {
        return Ok(false);
    }
    repo::delete_post(conn, post_id).await?;
    effects.publish(crate::service::removed_event(post_id)?);
    Ok(true)
}

/// 后台直接处置时锁住的帖子（AGENTS.md 规则 6：先锁再判断状态）。
#[derive(Debug, Clone)]
pub struct ModerationTarget {
    pub author_id: i64,
    pub is_anonymous: bool,
    pub title: String,
    pub published: bool,
    /// 隐藏原因；未隐藏为 None。
    pub hidden_reason: Option<String>,
}

/// 锁住帖子行并返回处置需要的信息；不存在时为 None。
pub async fn lock_moderation_target(
    conn: &mut PgConnection,
    post_id: i64,
) -> Result<Option<ModerationTarget>, Problem> {
    Ok(repo::lock_moderation_target(conn, post_id)
        .await?
        .map(|row| ModerationTarget {
            author_id: row.author_id,
            is_anonymous: row.is_anonymous,
            title: row.title,
            published: row.published,
            hidden_reason: if row.hidden {
                Some(row.hidden_reason.unwrap_or_default())
            } else {
                None
            },
        }))
}

/// 帖子标题（不论是否公开、是否草稿；已删除的不出现）。后台评论列表用。
pub async fn post_titles(
    conn: &mut PgConnection,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, String>, Problem> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    Ok(repo::titles(conn, ids).await?.into_iter().collect())
}

/// 帖子是否隐藏（按 id；已删除的不出现）。审核后台展示目标状态用。
pub async fn hidden_states(
    conn: &mut PgConnection,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, bool>, Problem> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    Ok(repo::moderation_states(conn, ids)
        .await?
        .into_iter()
        .map(|r| (r.id, r.hidden))
        .collect())
}

/// 已发布的帖子总数与 `since` 之后首次发布的数量（管理后台仪表盘）。
pub async fn post_counts(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
) -> Result<(i64, i64), Problem> {
    Ok(repo::post_counts(conn, since).await?)
}

/// 每个业务日首次发布的帖子数，按 `bounds` 分桶（仪表盘趋势）。
pub async fn daily_posts(
    conn: &mut PgConnection,
    bounds: &[DateTime<Utc>],
) -> Result<Vec<i64>, Problem> {
    let rows = repo::daily_first_published(conn, bounds).await?;
    Ok(ik_kernel::time::fill_buckets(
        rows,
        bounds.len().saturating_sub(1),
    ))
}
