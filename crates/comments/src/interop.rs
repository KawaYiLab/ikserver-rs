//! 给其他业务域用的评论数据接口（不直接读 `comments` 表，AGENTS.md 规则 5）。

use chrono::{DateTime, Utc};
use ik_content::interop::Activity;
use ik_kernel::Problem;
use ik_kernel::bus::{Audience, Effects, Event, internal};
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

// ---------- 搜索（search 域用）----------

/// 建索引用的评论：自身与父评论未隐藏、所属帖子公开。匿名评论的 `author_id` 为 None（PRIV-005）。
#[derive(Debug, Clone)]
pub struct CommentSearchDoc {
    pub id: i64,
    pub post_id: i64,
    pub parent_id: Option<i64>,
    pub post_title: String,
    pub author_id: Option<i64>,
    /// 纯文本（提及与表情 token 已转成可读文本）。
    pub text: String,
    pub like_count: i32,
    pub created_at: DateTime<Utc>,
}

async fn to_docs(
    conn: &mut PgConnection,
    rows: Vec<repo::CommentRow>,
) -> Result<Vec<CommentSearchDoc>, Problem> {
    let mut post_ids: Vec<i64> = rows.iter().map(|r| r.post_id).collect();
    post_ids.sort_unstable();
    post_ids.dedup();
    let titles = ik_content::interop::public_post_titles(conn, &post_ids).await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            titles.get(&row.post_id).map(|title| CommentSearchDoc {
                id: row.id,
                post_id: row.post_id,
                parent_id: row.parent_id,
                post_title: title.clone(),
                author_id: (!row.is_anonymous).then_some(row.author_id),
                text: crate::text::plain_text(&row.body),
                like_count: row.like_count,
                created_at: row.created_at,
            })
        })
        .collect())
}

/// `ids` 中应当出现在索引里的评论（其余的调用方据此删除）。
pub async fn search_docs(
    conn: &mut PgConnection,
    ids: &[i64],
) -> Result<Vec<CommentSearchDoc>, Problem> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = repo::listed_by_ids(conn, ids).await?;
    to_docs(conn, rows).await
}

/// 全部应当出现在索引里的评论，按 id 升序分批。返回 (这一批的文档, 这一批扫到的最大 id)；
/// 扫到的行可能因帖子不公开被滤掉，所以续扫要用第二个值。
pub async fn search_docs_after(
    conn: &mut PgConnection,
    after_id: i64,
    limit: i64,
) -> Result<(Vec<CommentSearchDoc>, Option<i64>), Problem> {
    let rows = repo::listed_after(conn, after_id, limit).await?;
    let last = rows.last().map(|r| r.id);
    Ok((to_docs(conn, rows).await?, last))
}

/// 某个帖子下的评论 id（最多 `limit` 个）。
pub async fn comment_ids_of_post(
    conn: &mut PgConnection,
    post_id: i64,
    limit: i64,
) -> Result<Vec<i64>, Problem> {
    Ok(repo::ids_of_post(conn, post_id, limit).await?)
}

/// `since` 之后发表的评论 id，最多 `limit` 个，按时间升序。
pub async fn created_since(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<i64>, Problem> {
    Ok(repo::created_since(conn, since, limit).await?)
}

// ---------- 审核（moderation） ----------

/// 举报目标：评论（RPT-009、RPT-013）。
#[derive(Debug, Clone)]
pub struct ReportableComment {
    pub author_id: i64,
    pub post_id: i64,
    pub post_title: String,
    /// 纯文本（提及与表情 token 已转成可读文本）。
    pub text: String,
    pub image_ids: Vec<i64>,
    pub is_anonymous: bool,
    /// 评论与父评论（若有）未隐藏、所属帖子公开。只有公开的评论能被举报。
    pub is_public: bool,
}

pub async fn reportable_comment(
    conn: &mut PgConnection,
    comment_id: i64,
) -> Result<Option<ReportableComment>, Problem> {
    let Some(comment) = repo::get(conn, comment_id).await? else {
        return Ok(None);
    };
    let Some(post) = ik_content::interop::post(conn, comment.post_id).await? else {
        return Ok(None);
    };
    let parent_public = match comment.parent_id {
        Some(parent_id) => repo::get(conn, parent_id)
            .await?
            .is_some_and(|p| p.hidden_at.is_none() && p.parent_id.is_none()),
        None => true,
    };
    Ok(Some(ReportableComment {
        author_id: comment.author_id,
        post_id: comment.post_id,
        post_title: post.title,
        text: crate::text::plain_text(&comment.body),
        image_ids: comment.image_ids,
        is_anonymous: comment.is_anonymous,
        is_public: comment.hidden_at.is_none() && post.is_public && parent_public,
    }))
}

/// 隐藏评论（同时取消置顶，采纳保留）。已经隐藏的改为新的原因与操作人。评论不存在时返回 false。
/// 原本可见的评论会发出内部事件，搜索索引删除它。
pub async fn hide_comment(
    conn: &mut PgConnection,
    comment_id: i64,
    by: Option<i64>,
    reason: &str,
    now: DateTime<Utc>,
    effects: &mut Effects,
) -> Result<bool, Problem> {
    let Some(post_id) = repo::get(conn, comment_id).await?.map(|c| c.post_id) else {
        return Ok(false);
    };
    let Some(was_visible) = repo::hide(conn, comment_id, by, reason, now).await? else {
        return Ok(false);
    };
    if was_visible {
        effects.publish(crate::service::removed_event(comment_id, post_id)?);
    }
    Ok(true)
}

/// 取消隐藏，只在当前隐藏原因是 `reason` 时生效（驳回只恢复自动隐藏）。返回是否恢复了。
pub async fn unhide_comment(
    conn: &mut PgConnection,
    comment_id: i64,
    reason: &str,
    effects: &mut Effects,
) -> Result<bool, Problem> {
    let Some(post_id) = repo::unhide(conn, comment_id, reason).await? else {
        return Ok(false);
    };
    effects.publish(Event::new(
        internal::COMMENT_RESTORED,
        Audience::Internal,
        &serde_json::json!({ "commentId": comment_id, "postId": post_id }),
    )?);
    Ok(true)
}

/// 审核删除评论（连同回复），维护帖子评论数与采纳。返回是否删除了。
pub async fn delete_comment(
    conn: &mut PgConnection,
    comment_id: i64,
    effects: &mut Effects,
) -> Result<bool, Problem> {
    let Some(post_id) = repo::get(conn, comment_id).await?.map(|c| c.post_id) else {
        return Ok(false);
    };
    ik_content::interop::lock_post(conn, post_id).await?;
    let Some(comment) = repo::lock(conn, comment_id).await? else {
        return Ok(false);
    };
    crate::service::remove_locked(conn, post_id, &comment).await?;
    effects.publish(crate::service::removed_event(comment_id, post_id)?);
    Ok(true)
}

/// 评论是否隐藏（按 id；已删除的不出现）。审核后台展示目标状态用。
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
