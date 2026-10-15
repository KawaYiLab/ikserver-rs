//! SQL。只被本 crate 的 service 调用。

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

pub(crate) struct CommentRow {
    pub id: i64,
    pub post_id: i64,
    pub parent_id: Option<i64>,
    pub author_id: i64,
    pub floor: Option<i32>,
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
        "SELECT id, post_id, parent_id, author_id, floor, body, is_anonymous, anonymous_seed,
                like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments WHERE id = $1",
        id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn lock(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, parent_id, author_id, floor, body, is_anonymous, anonymous_seed,
                like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments WHERE id = $1 FOR UPDATE",
        id,
    )
    .fetch_optional(conn)
    .await
}

/// 下一个楼层号。调用方必须已锁住帖子行，否则并发评论会抢到同一个号。
pub(crate) async fn next_floor(conn: &mut PgConnection, post_id: i64) -> sqlx::Result<i32> {
    sqlx::query_scalar!(
        r#"SELECT COALESCE(MAX(floor), 0) + 1 AS "floor!" FROM comments WHERE post_id = $1"#,
        post_id,
    )
    .fetch_one(conn)
    .await
}

pub(crate) struct NewComment<'a> {
    pub id: i64,
    pub post_id: i64,
    pub parent_id: Option<i64>,
    pub author_id: i64,
    pub floor: Option<i32>,
    pub body: &'a str,
    pub is_anonymous: bool,
    pub anonymous_seed: Option<&'a str>,
    pub image_ids: &'a [i64],
    pub now: DateTime<Utc>,
}

pub(crate) async fn insert(conn: &mut PgConnection, c: &NewComment<'_>) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO comments (id, post_id, parent_id, author_id, floor, body, is_anonymous,
                               anonymous_seed, created_at, image_ids)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        c.id,
        c.post_id,
        c.parent_id,
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

/// 一条评论的全部回复数（含隐藏的），用于删除时扣减帖子评论数。
pub(crate) async fn count_all_replies(
    conn: &mut PgConnection,
    parent_id: i64,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM comments WHERE parent_id = $1"#,
        parent_id,
    )
    .fetch_one(conn)
    .await
}

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

/// 普通一级评论：可见、未置顶、未采纳，新的在前。
///
/// 本节所有读取都带 `excluded_authors`：与读者有拉黑关系的人的非匿名评论不出现（BLOCK-014）。
pub(crate) async fn top_level_page(
    conn: &mut PgConnection,
    post_id: i64,
    excluded_authors: &[i64],
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, parent_id, author_id, floor, body, is_anonymous, anonymous_seed,
                like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE post_id = $1 AND parent_id IS NULL AND hidden_at IS NULL
           AND pinned_at IS NULL AND accepted_at IS NULL
           AND (is_anonymous OR NOT (author_id = ANY($5)))
           AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3::bigint))
         ORDER BY created_at DESC, id DESC
         LIMIT $4",
        post_id,
        after_at,
        after_id,
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
        "SELECT id, post_id, parent_id, author_id, floor, body, is_anonymous, anonymous_seed,
                like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE post_id = $1 AND parent_id IS NULL AND hidden_at IS NULL AND accepted_at IS NOT NULL
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
        "SELECT id, post_id, parent_id, author_id, floor, body, is_anonymous, anonymous_seed,
                like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE post_id = $1 AND parent_id IS NULL AND hidden_at IS NULL
           AND pinned_at IS NOT NULL AND accepted_at IS NULL
           AND (is_anonymous OR NOT (author_id = ANY($2)))",
        post_id,
        excluded_authors,
    )
    .fetch_optional(conn)
    .await
}

/// 按热度的一级评论（AI 工具 `get_post_comments`）：可见，赞数多的在前，同赞新的在前。
pub(crate) async fn hot_top_level(
    conn: &mut PgConnection,
    post_id: i64,
    excluded_authors: &[i64],
    limit: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, parent_id, author_id, floor, body, is_anonymous, anonymous_seed,
                like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE post_id = $1 AND parent_id IS NULL AND hidden_at IS NULL
           AND (is_anonymous OR NOT (author_id = ANY($2)))
         ORDER BY like_count DESC, created_at DESC, id DESC
         LIMIT $3",
        post_id,
        excluded_authors,
        limit,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn visible_top_level_count(
    conn: &mut PgConnection,
    post_id: i64,
    excluded_authors: &[i64],
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM comments
           WHERE post_id = $1 AND parent_id IS NULL AND hidden_at IS NULL
             AND (is_anonymous OR NOT (author_id = ANY($2)))"#,
        post_id,
        excluded_authors,
    )
    .fetch_one(conn)
    .await
}

/// 每个一级评论各自的前 `per_parent` 条可见回复（不是整页共享名额，CMT-069/146）。
pub(crate) async fn inline_replies(
    conn: &mut PgConnection,
    parent_ids: &[i64],
    excluded_authors: &[i64],
    per_parent: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        r#"SELECT r.id AS "id!", r.post_id AS "post_id!", r.parent_id, r.author_id AS "author_id!",
                  r.floor, r.body AS "body!", r.is_anonymous AS "is_anonymous!", r.anonymous_seed,
                  r.like_count AS "like_count!", r.pinned_at, r.accepted_at, r.hidden_at,
                  r.created_at AS "created_at!", r.image_ids AS "image_ids!"
           FROM unnest($1::bigint[]) AS p(id)
           CROSS JOIN LATERAL (
               SELECT * FROM comments c
               WHERE c.parent_id = p.id AND c.hidden_at IS NULL
                 AND (c.is_anonymous OR NOT (c.author_id = ANY($3)))
               ORDER BY c.created_at, c.id
               LIMIT $2
           ) r
           ORDER BY r.parent_id, r.created_at, r.id"#,
        parent_ids,
        per_parent,
        excluded_authors,
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct ReplyCount {
    pub parent_id: i64,
    pub n: i64,
}

pub(crate) async fn visible_reply_counts(
    conn: &mut PgConnection,
    parent_ids: &[i64],
    excluded_authors: &[i64],
) -> sqlx::Result<Vec<ReplyCount>> {
    sqlx::query_as!(
        ReplyCount,
        r#"SELECT parent_id AS "parent_id!", count(*) AS "n!"
           FROM comments
           WHERE parent_id = ANY($1) AND hidden_at IS NULL
             AND (is_anonymous OR NOT (author_id = ANY($2)))
           GROUP BY parent_id"#,
        parent_ids,
        excluded_authors,
    )
    .fetch_all(conn)
    .await
}

/// 某条一级评论的可见回复，旧的在前，严格晚于游标（CMT-072）。
pub(crate) async fn replies_page(
    conn: &mut PgConnection,
    parent_id: i64,
    excluded_authors: &[i64],
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, parent_id, author_id, floor, body, is_anonymous, anonymous_seed,
                like_count, pinned_at, accepted_at, hidden_at, created_at, image_ids
         FROM comments
         WHERE parent_id = $1 AND hidden_at IS NULL
           AND (is_anonymous OR NOT (author_id = ANY($5)))
           AND ($2::timestamptz IS NULL OR (created_at, id) > ($2, $3::bigint))
         ORDER BY created_at, id
         LIMIT $4",
        parent_id,
        after_at,
        after_id,
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

/// 作者主页展示的评论：非匿名、未隐藏、回复的一级评论也未隐藏。所属帖子是否公开由调用方再过滤。
pub(crate) async fn author_comments(
    conn: &mut PgConnection,
    author_id: i64,
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        CommentRow,
        "SELECT c.id, c.post_id, c.parent_id, c.author_id, c.floor, c.body, c.is_anonymous,
                c.anonymous_seed, c.like_count, c.pinned_at, c.accepted_at, c.hidden_at, c.created_at,
                c.image_ids
         FROM comments c
         LEFT JOIN comments parent ON parent.id = c.parent_id
         WHERE c.author_id = $1 AND NOT c.is_anonymous AND c.hidden_at IS NULL
           AND (c.parent_id IS NULL OR parent.hidden_at IS NULL)
           AND ($2::timestamptz IS NULL OR (c.created_at, c.id) < ($2, $3::bigint))
         ORDER BY c.created_at DESC, c.id DESC
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

/// 窗口内的有效评论：评论未隐藏，回复的一级评论也未隐藏（只有一层楼中楼，HOT-005）。
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
           LEFT JOIN comments parent ON parent.id = c.parent_id
           WHERE c.created_at >= $1 AND c.created_at <= $2 AND c.hidden_at IS NULL
             AND (c.parent_id IS NULL OR parent.hidden_at IS NULL)
           GROUP BY c.post_id, c.author_id"#,
        since,
        until,
    )
    .fetch_all(conn)
    .await
}

// ---------- 搜索（search 域建索引与数据库兜底用）----------

/// 自身未隐藏、回复的一级评论也未隐藏的评论（所属帖子是否公开由调用方经 content 判断）。
pub(crate) async fn listed_by_ids(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT c.id, c.post_id, c.parent_id, c.author_id, c.floor, c.body, c.is_anonymous,
                c.anonymous_seed, c.like_count, c.pinned_at, c.accepted_at, c.hidden_at, c.created_at,
                c.image_ids
         FROM comments c
         LEFT JOIN comments parent ON parent.id = c.parent_id
         WHERE c.id = ANY($1) AND c.hidden_at IS NULL
           AND (c.parent_id IS NULL OR (parent.hidden_at IS NULL AND parent.post_id = c.post_id))",
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
        "SELECT c.id, c.post_id, c.parent_id, c.author_id, c.floor, c.body, c.is_anonymous,
                c.anonymous_seed, c.like_count, c.pinned_at, c.accepted_at, c.hidden_at, c.created_at,
                c.image_ids
         FROM comments c
         LEFT JOIN comments parent ON parent.id = c.parent_id
         WHERE c.id > $1 AND c.hidden_at IS NULL
           AND (c.parent_id IS NULL OR (parent.hidden_at IS NULL AND parent.post_id = c.post_id))
         ORDER BY c.id
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

/// 数据库兜底搜索的候选（SRCH-033）：正文包含 `q`（忽略大小写），自身与父评论未隐藏，
/// 可选限定帖子，新的在前，最多 `cap` 条。所属帖子是否公开与拉黑由调用方再过滤。
pub(crate) async fn search_candidates(
    conn: &mut PgConnection,
    q: &str,
    post_id: Option<i64>,
    cap: i64,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT c.id FROM comments c
         LEFT JOIN comments parent ON parent.id = c.parent_id
         WHERE c.hidden_at IS NULL
           AND (c.parent_id IS NULL OR parent.hidden_at IS NULL)
           AND ($2::bigint IS NULL OR c.post_id = $2)
           AND strpos(lower(c.body), lower($1)) > 0
         ORDER BY c.created_at DESC, c.id DESC
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
