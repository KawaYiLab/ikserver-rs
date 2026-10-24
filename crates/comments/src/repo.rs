//! SQL。只被本 crate 的 service 调用。

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

#[derive(Clone)]
pub(crate) struct CommentRow {
    pub id: i64,
    pub post_id: i64,
    pub reply_to_id: Option<i64>,
    pub reply_to_floor: Option<i32>,
    pub author_id: i64,
    pub floor: i32,
    pub body: String,
    pub is_anonymous: bool,
    pub anonymous_seed: Option<String>,
    pub like_count: i32,
    pub pinned_at: Option<DateTime<Utc>>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub hidden_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub image_ids: Vec<i64>,
}

pub(crate) async fn get(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, reply_to_id, reply_to_floor, author_id, floor, body, is_anonymous,
                anonymous_seed, like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments WHERE id = $1",
        id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn lock(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, reply_to_id, reply_to_floor, author_id, floor, body, is_anonymous,
                anonymous_seed, like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments WHERE id = $1 FOR UPDATE",
        id,
    )
    .fetch_optional(conn)
    .await
}

/// 按 id 取（不论是否可见），被回复评论的引用用。
pub(crate) async fn by_ids(conn: &mut PgConnection, ids: &[i64]) -> sqlx::Result<Vec<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, reply_to_id, reply_to_floor, author_id, floor, body, is_anonymous,
                anonymous_seed, like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments WHERE id = ANY($1)",
        ids,
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct NewComment<'a> {
    pub id: i64,
    pub post_id: i64,
    /// 被回复的评论与它的楼层号。
    pub reply_to: Option<(i64, i32)>,
    pub author_id: i64,
    pub floor: i32,
    pub body: &'a str,
    pub is_anonymous: bool,
    pub anonymous_seed: Option<&'a str>,
    pub image_ids: &'a [i64],
    pub now: DateTime<Utc>,
}

pub(crate) async fn insert(conn: &mut PgConnection, c: &NewComment<'_>) -> sqlx::Result<()> {
    let (reply_to_id, reply_to_floor) = c.reply_to.unzip();
    sqlx::query!(
        "INSERT INTO comments (id, post_id, reply_to_id, reply_to_floor, author_id, floor, body,
                               is_anonymous, anonymous_seed, created_at, image_ids)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        c.id,
        c.post_id,
        reply_to_id,
        reply_to_floor,
        c.author_id,
        c.floor,
        c.body,
        c.is_anonymous,
        c.anonymous_seed,
        c.now,
        c.image_ids,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 硬删除。回复它的评论保留（`reply_to_id` 由外键置空，`reply_to_floor` 留着显示「原评论已删除」）。
pub(crate) async fn delete(conn: &mut PgConnection, id: i64) -> sqlx::Result<()> {
    sqlx::query!("DELETE FROM comments WHERE id = $1", id)
        .execute(conn)
        .await?;
    Ok(())
}

// ---------- 置顶与采纳 ----------

pub(crate) async fn clear_pins(conn: &mut PgConnection, post_id: i64) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE comments SET pinned_at = NULL, pinned_by = NULL
         WHERE post_id = $1 AND pinned_at IS NOT NULL",
        post_id,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn set_pin(
    conn: &mut PgConnection,
    id: i64,
    by: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE comments SET pinned_at = $3, pinned_by = $2 WHERE id = $1",
        id,
        by,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn clear_accepts(conn: &mut PgConnection, post_id: i64) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE comments SET accepted_at = NULL, accepted_by = NULL
         WHERE post_id = $1 AND accepted_at IS NOT NULL",
        post_id,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn set_accept(
    conn: &mut PgConnection,
    id: i64,
    by: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE comments SET accepted_at = $3, accepted_by = $2 WHERE id = $1",
        id,
        by,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn unpin(conn: &mut PgConnection, id: i64) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE comments SET pinned_at = NULL, pinned_by = NULL WHERE id = $1",
        id,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn unaccept(conn: &mut PgConnection, id: i64) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE comments SET accepted_at = NULL, accepted_by = NULL WHERE id = $1",
        id,
    )
    .execute(conn)
    .await?;
    Ok(())
}

// ---------- 读取 ----------
//
// 本节所有读取都带 `excluded_authors`：与读者有拉黑关系的人的非匿名评论不出现（BLOCK-014）。

/// 帖子的可见评论，楼层号严格大于 `after`，旧的在前。
pub(crate) async fn page_after(
    conn: &mut PgConnection,
    post_id: i64,
    excluded_authors: &[i64],
    after: i32,
    limit: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, reply_to_id, reply_to_floor, author_id, floor, body, is_anonymous,
                anonymous_seed, like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE post_id = $1 AND floor > $2 AND hidden_at IS NULL
           AND (is_anonymous OR NOT (author_id = ANY($4)))
         ORDER BY floor
         LIMIT $3",
        post_id,
        after,
        limit,
        excluded_authors,
    )
    .fetch_all(conn)
    .await
}

/// 帖子的可见评论，楼层号严格小于 `before`，新的在前（往回翻；调用方再倒过来）。
pub(crate) async fn page_before(
    conn: &mut PgConnection,
    post_id: i64,
    excluded_authors: &[i64],
    before: i32,
    limit: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, reply_to_id, reply_to_floor, author_id, floor, body, is_anonymous,
                anonymous_seed, like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE post_id = $1 AND floor < $2 AND hidden_at IS NULL
           AND (is_anonymous OR NOT (author_id = ANY($4)))
         ORDER BY floor DESC
         LIMIT $3",
        post_id,
        before,
        limit,
        excluded_authors,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn accepted(
    conn: &mut PgConnection,
    post_id: i64,
    excluded_authors: &[i64],
) -> sqlx::Result<Option<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, reply_to_id, reply_to_floor, author_id, floor, body, is_anonymous,
                anonymous_seed, like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE post_id = $1 AND hidden_at IS NULL AND accepted_at IS NOT NULL
           AND (is_anonymous OR NOT (author_id = ANY($2)))",
        post_id,
        excluded_authors,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn pinned(
    conn: &mut PgConnection,
    post_id: i64,
    excluded_authors: &[i64],
) -> sqlx::Result<Option<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, reply_to_id, reply_to_floor, author_id, floor, body, is_anonymous,
                anonymous_seed, like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE post_id = $1 AND hidden_at IS NULL
           AND pinned_at IS NOT NULL AND accepted_at IS NULL
           AND (is_anonymous OR NOT (author_id = ANY($2)))",
        post_id,
        excluded_authors,
    )
    .fetch_optional(conn)
    .await
}

/// 按热度的评论（AI 工具 `get_post_comments`）：可见，赞数多的在前，同赞旧的在前。
pub(crate) async fn hot(
    conn: &mut PgConnection,
    post_id: i64,
    excluded_authors: &[i64],
    limit: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, reply_to_id, reply_to_floor, author_id, floor, body, is_anonymous,
                anonymous_seed, like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE post_id = $1 AND hidden_at IS NULL
           AND (is_anonymous OR NOT (author_id = ANY($2)))
         ORDER BY like_count DESC, floor
         LIMIT $3",
        post_id,
        excluded_authors,
        limit,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn visible_count(
    conn: &mut PgConnection,
    post_id: i64,
    excluded_authors: &[i64],
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM comments
           WHERE post_id = $1 AND hidden_at IS NULL
             AND (is_anonymous OR NOT (author_id = ANY($2)))"#,
        post_id,
        excluded_authors,
    )
    .fetch_one(conn)
    .await
}

pub(crate) struct ReplyCount {
    pub reply_to_id: i64,
    pub n: i64,
}

/// 每条评论收到的可见回复数。
pub(crate) async fn visible_reply_counts(
    conn: &mut PgConnection,
    ids: &[i64],
    excluded_authors: &[i64],
) -> sqlx::Result<Vec<ReplyCount>> {
    sqlx::query_as!(
        ReplyCount,
        r#"SELECT reply_to_id AS "reply_to_id!", count(*) AS "n!"
           FROM comments
           WHERE reply_to_id = ANY($1) AND hidden_at IS NULL
             AND (is_anonymous OR NOT (author_id = ANY($2)))
           GROUP BY reply_to_id"#,
        ids,
        excluded_authors,
    )
    .fetch_all(conn)
    .await
}

/// 回复某条评论的可见评论，旧的在前，楼层号严格大于 `after`。
pub(crate) async fn replies_page(
    conn: &mut PgConnection,
    reply_to_id: i64,
    excluded_authors: &[i64],
    after: i32,
    limit: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, reply_to_id, reply_to_floor, author_id, floor, body, is_anonymous,
                anonymous_seed, like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE reply_to_id = $1 AND floor > $2 AND hidden_at IS NULL
           AND (is_anonymous OR NOT (author_id = ANY($4)))
         ORDER BY floor
         LIMIT $3",
        reply_to_id,
        after,
        limit,
        excluded_authors,
    )
    .fetch_all(conn)
    .await
}

// ---------- 点赞 ----------

pub(crate) async fn insert_like(
    conn: &mut PgConnection,
    comment_id: i64,
    user_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "INSERT INTO comment_likes (comment_id, user_id, created_at) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
        comment_id,
        user_id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn delete_like(
    conn: &mut PgConnection,
    comment_id: i64,
    user_id: i64,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "DELETE FROM comment_likes WHERE comment_id = $1 AND user_id = $2",
        comment_id,
        user_id,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 点赞数加减（单条原子 UPDATE，下限 0），返回新值；`delta` 为 0 时就是读取。评论不存在时返回 None。
pub(crate) async fn add_like_count(
    conn: &mut PgConnection,
    comment_id: i64,
    delta: i32,
) -> sqlx::Result<Option<i32>> {
    if delta == 0 {
        return sqlx::query_scalar!("SELECT like_count FROM comments WHERE id = $1", comment_id)
            .fetch_optional(conn)
            .await;
    }
    sqlx::query_scalar!(
        "UPDATE comments SET like_count = GREATEST(like_count + $2, 0) WHERE id = $1
         RETURNING like_count",
        comment_id,
        delta,
    )
    .fetch_optional(conn)
    .await
}

/// `comment_ids` 中我点过赞的。
pub(crate) async fn liked_among(
    conn: &mut PgConnection,
    user_id: i64,
    comment_ids: &[i64],
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT comment_id FROM comment_likes WHERE user_id = $1 AND comment_id = ANY($2)",
        user_id,
        comment_ids,
    )
    .fetch_all(conn)
    .await
}

// ---------- 作者主页 ----------

/// 作者主页展示的评论：非匿名、未隐藏。所属帖子是否公开由调用方再过滤。
pub(crate) async fn author_comments(
    conn: &mut PgConnection,
    author_id: i64,
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, reply_to_id, reply_to_floor, author_id, floor, body, is_anonymous,
                anonymous_seed, like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE author_id = $1 AND NOT is_anonymous AND hidden_at IS NULL
           AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3::bigint))
         ORDER BY created_at DESC, id DESC
         LIMIT $4",
        author_id,
        after_at,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 作者的非匿名、未隐藏评论数（作者主页统计）。
pub(crate) async fn author_comment_count(
    conn: &mut PgConnection,
    author_id: i64,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM comments
           WHERE author_id = $1 AND NOT is_anonymous AND hidden_at IS NULL"#,
        author_id,
    )
    .fetch_one(conn)
    .await
}

// ---------- 热榜数据 ----------

pub(crate) struct ActivityRow {
    pub post_id: i64,
    pub user_id: i64,
    pub first_at: DateTime<Utc>,
}

/// 窗口内的有效评论：评论未隐藏（HOT-005）。
/// 每个 (帖子, 评论者) 一行，取最早时间。
pub(crate) async fn comment_activity(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> sqlx::Result<Vec<ActivityRow>> {
    sqlx::query_as!(
        ActivityRow,
        r#"SELECT c.post_id, c.author_id AS user_id, min(c.created_at) AS "first_at!"
           FROM comments c
           WHERE c.created_at >= $1 AND c.created_at <= $2 AND c.hidden_at IS NULL
           GROUP BY c.post_id, c.author_id"#,
        since,
        until,
    )
    .fetch_all(conn)
    .await
}

// ---------- 搜索（search 域建索引与数据库兜底用）----------

/// 未隐藏的评论（所属帖子是否公开由调用方经 content 判断）。
pub(crate) async fn listed_by_ids(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, reply_to_id, reply_to_floor, author_id, floor, body, is_anonymous,
                anonymous_seed, like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE id = ANY($1) AND hidden_at IS NULL",
        ids,
    )
    .fetch_all(conn)
    .await
}

/// 同上，按 id 升序分批（全量对账）。
pub(crate) async fn listed_after(
    conn: &mut PgConnection,
    after_id: i64,
    limit: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, reply_to_id, reply_to_floor, author_id, floor, body, is_anonymous,
                anonymous_seed, like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE id > $1 AND hidden_at IS NULL
         ORDER BY id
         LIMIT $2",
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 某个帖子下的全部评论 id（帖子重新公开时补建索引）。
pub(crate) async fn ids_of_post(
    conn: &mut PgConnection,
    post_id: i64,
    limit: i64,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT id FROM comments WHERE post_id = $1 ORDER BY id LIMIT $2",
        post_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// `since` 之后发表的评论。
pub(crate) async fn created_since(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
    limit: i64,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT id FROM comments WHERE created_at >= $1 ORDER BY created_at, id LIMIT $2",
        since,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 数据库兜底搜索的候选（SRCH-033）：正文包含 `q`（忽略大小写），未隐藏，
/// 可选限定帖子，新的在前，最多 `cap` 条。所属帖子是否公开与拉黑由调用方再过滤。
pub(crate) async fn search_candidates(
    conn: &mut PgConnection,
    q: &str,
    post_id: Option<i64>,
    cap: i64,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT id FROM comments
         WHERE hidden_at IS NULL
           AND ($2::bigint IS NULL OR post_id = $2)
           AND strpos(lower(body), lower($1)) > 0
         ORDER BY created_at DESC, id DESC
         LIMIT $3",
        q,
        post_id,
        cap,
    )
    .fetch_all(conn)
    .await
}

// ---------- 审核（moderation） ----------

/// 隐藏评论并取消置顶（RPT-053）；采纳保留（D-028 方案 A，驳回后自然恢复）。已隐藏的只改原因与操作人。
/// 返回隐藏前是否可见。
pub(crate) async fn hide(
    conn: &mut PgConnection,
    id: i64,
    by: Option<i64>,
    reason: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<Option<bool>> {
    sqlx::query_scalar!(
        r#"WITH before AS (SELECT id, hidden_at IS NULL AS was_visible FROM comments WHERE id = $1 FOR UPDATE)
           UPDATE comments c
           SET hidden_at = COALESCE(c.hidden_at, $4), hidden_by = $2, hidden_reason = $3, pinned_at = NULL
           FROM before b WHERE c.id = b.id
           RETURNING b.was_visible AS "was_visible!""#,
        id,
        by,
        reason,
        now
    )
    .fetch_optional(conn)
    .await
}

/// 取消隐藏，只在当前隐藏原因是 `reason` 时生效。返回所属帖子。
pub(crate) async fn unhide(
    conn: &mut PgConnection,
    id: i64,
    reason: &str,
) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar!(
        "UPDATE comments SET hidden_at = NULL, hidden_by = NULL, hidden_reason = NULL
         WHERE id = $1 AND hidden_at IS NOT NULL AND hidden_reason = $2
         RETURNING post_id",
        id,
        reason
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct CommentStateRow {
    pub id: i64,
    pub hidden: bool,
}

pub(crate) async fn moderation_states(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<CommentStateRow>> {
    sqlx::query_as!(
        CommentStateRow,
        r#"SELECT id, (hidden_at IS NOT NULL) AS "hidden!" FROM comments WHERE id = ANY($1)"#,
        ids
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn comment_counts(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
) -> sqlx::Result<(i64, i64)> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "total!", count(*) FILTER (WHERE created_at >= $1) AS "since!"
           FROM comments"#,
        since,
    )
    .fetch_one(conn)
    .await?;
    Ok((row.total, row.since))
}

// ---------- 管理后台：评论管理 ----------

pub(crate) struct AdminCommentFilter<'a> {
    /// 正文包含（ILIKE 模式，已转义）。
    pub pattern: Option<&'a str>,
    /// 关键词是数字时也按评论 id 匹配。
    pub id: Option<i64>,
    pub post_id: Option<i64>,
    /// 按作者筛选时排除匿名评论（AGENTS.md 规则 10）。
    pub author_id: Option<i64>,
    pub hidden: Option<bool>,
}

pub(crate) struct AdminCommentRow {
    pub id: i64,
    pub post_id: i64,
    pub reply_to_id: Option<i64>,
    pub floor: i32,
    pub author_id: i64,
    pub body: String,
    pub image_ids: Vec<i64>,
    pub is_anonymous: bool,
    pub anonymous_seed: Option<String>,
    pub like_count: i32,
    pub pinned_at: Option<DateTime<Utc>>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub hidden_at: Option<DateTime<Utc>>,
    pub hidden_by: Option<i64>,
    pub hidden_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn count_admin_comments(
    conn: &mut PgConnection,
    f: &AdminCommentFilter<'_>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM comments c
           WHERE ($1::text IS NULL OR c.body ILIKE $1 OR c.id = $2)
             AND ($3::bigint IS NULL OR c.post_id = $3)
             AND ($4::bigint IS NULL OR (c.author_id = $4 AND NOT c.is_anonymous))
             AND ($5::bool IS NULL OR (c.hidden_at IS NOT NULL) = $5)"#,
        f.pattern,
        f.id,
        f.post_id,
        f.author_id,
        f.hidden,
    )
    .fetch_one(conn)
    .await
}

/// 评论（含隐藏的），新的在前。
pub(crate) async fn admin_comments(
    conn: &mut PgConnection,
    f: &AdminCommentFilter<'_>,
    limit: i64,
    offset: i64,
) -> sqlx::Result<Vec<AdminCommentRow>> {
    sqlx::query_as!(
        AdminCommentRow,
        r#"SELECT c.id, c.post_id, c.reply_to_id, c.floor, c.author_id, c.body, c.image_ids, c.is_anonymous,
                  c.anonymous_seed, c.like_count, c.pinned_at, c.accepted_at, c.hidden_at, c.hidden_by,
                  c.hidden_reason, c.created_at
           FROM comments c
           WHERE ($1::text IS NULL OR c.body ILIKE $1 OR c.id = $2)
             AND ($3::bigint IS NULL OR c.post_id = $3)
             AND ($4::bigint IS NULL OR (c.author_id = $4 AND NOT c.is_anonymous))
             AND ($5::bool IS NULL OR (c.hidden_at IS NOT NULL) = $5)
           ORDER BY c.created_at DESC, c.id DESC
           LIMIT $6 OFFSET $7"#,
        f.pattern,
        f.id,
        f.post_id,
        f.author_id,
        f.hidden,
        limit,
        offset,
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct ModerationTargetRow {
    pub author_id: i64,
    pub post_id: i64,
    pub is_anonymous: bool,
    pub hidden: bool,
    pub hidden_reason: Option<String>,
}

/// 审核用：锁住评论行，返回作者、所属帖子、是否匿名与隐藏状态。调用方先锁帖子行（与删除评论同序）。
pub(crate) async fn lock_moderation_target(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<ModerationTargetRow>> {
    sqlx::query_as!(
        ModerationTargetRow,
        r#"SELECT author_id, post_id, is_anonymous, hidden_at IS NOT NULL AS "hidden!", hidden_reason
           FROM comments WHERE id = $1 FOR UPDATE"#,
        id,
    )
    .fetch_optional(conn)
    .await
}

/// 每个业务日发表的评论数（仪表盘趋势）。
pub(crate) async fn daily_comments(
    conn: &mut PgConnection,
    bounds: &[DateTime<Utc>],
) -> sqlx::Result<Vec<(i32, i64)>> {
    let (Some(first), Some(last)) = (bounds.first(), bounds.last()) else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query!(
        r#"SELECT width_bucket(created_at, $1::timestamptz[]) AS "bucket!", count(*) AS "n!"
           FROM comments
           WHERE created_at >= $2 AND created_at < $3
           GROUP BY 1"#,
        bounds,
        first,
        last,
    )
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|r| (r.bucket, r.n)).collect())
}
