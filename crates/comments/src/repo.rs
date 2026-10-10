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
}

pub(crate) async fn get(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, parent_id, author_id, floor, body, is_anonymous, anonymous_seed,
                like_count, pinned_at, accepted_at, hidden_at, created_at
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
                like_count, pinned_at, accepted_at, hidden_at, created_at
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
    pub now: DateTime<Utc>,
}

pub(crate) async fn insert(conn: &mut PgConnection, c: &NewComment<'_>) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO comments (id, post_id, parent_id, author_id, floor, body, is_anonymous,
                               anonymous_seed, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        c.id,
        c.post_id,
        c.parent_id,
        c.author_id,
        c.floor,
        c.body,
        c.is_anonymous,
        c.anonymous_seed,
        c.now,
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
pub(crate) async fn top_level_page(
    conn: &mut PgConnection,
    post_id: i64,
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, parent_id, author_id, floor, body, is_anonymous, anonymous_seed,
                like_count, pinned_at, accepted_at, hidden_at, created_at
         FROM comments
         WHERE post_id = $1 AND parent_id IS NULL AND hidden_at IS NULL
           AND pinned_at IS NULL AND accepted_at IS NULL
           AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3::bigint))
         ORDER BY created_at DESC, id DESC
         LIMIT $4",
        post_id,
        after_at,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn accepted(
    conn: &mut PgConnection,
    post_id: i64,
) -> sqlx::Result<Option<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, parent_id, author_id, floor, body, is_anonymous, anonymous_seed,
                like_count, pinned_at, accepted_at, hidden_at, created_at
         FROM comments
         WHERE post_id = $1 AND parent_id IS NULL AND hidden_at IS NULL AND accepted_at IS NOT NULL",
        post_id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn pinned(
    conn: &mut PgConnection,
    post_id: i64,
) -> sqlx::Result<Option<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, parent_id, author_id, floor, body, is_anonymous, anonymous_seed,
                like_count, pinned_at, accepted_at, hidden_at, created_at
         FROM comments
         WHERE post_id = $1 AND parent_id IS NULL AND hidden_at IS NULL
           AND pinned_at IS NOT NULL AND accepted_at IS NULL",
        post_id,
    )
    .fetch_optional(conn)
    .await
}

/// 每个一级评论各自的前 `per_parent` 条可见回复（不是整页共享名额，CMT-069/146）。
pub(crate) async fn inline_replies(
    conn: &mut PgConnection,
    parent_ids: &[i64],
    per_parent: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    sqlx::query_as!(
        CommentRow,
        r#"SELECT r.id AS "id!", r.post_id AS "post_id!", r.parent_id, r.author_id AS "author_id!",
                  r.floor, r.body AS "body!", r.is_anonymous AS "is_anonymous!", r.anonymous_seed,
                  r.like_count AS "like_count!", r.pinned_at, r.accepted_at, r.hidden_at,
                  r.created_at AS "created_at!"
           FROM unnest($1::bigint[]) AS p(id)
           CROSS JOIN LATERAL (
               SELECT * FROM comments c
               WHERE c.parent_id = p.id AND c.hidden_at IS NULL
               ORDER BY c.created_at, c.id
               LIMIT $2
           ) r
           ORDER BY r.parent_id, r.created_at, r.id"#,
        parent_ids,
        per_parent,
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
) -> sqlx::Result<Vec<ReplyCount>> {
    sqlx::query_as!(
        ReplyCount,
        r#"SELECT parent_id AS "parent_id!", count(*) AS "n!"
           FROM comments
           WHERE parent_id = ANY($1) AND hidden_at IS NULL
           GROUP BY parent_id"#,
        parent_ids,
    )
    .fetch_all(conn)
    .await
}

/// 某条一级评论的可见回复，旧的在前，严格晚于游标（CMT-072）。
pub(crate) async fn replies_page(
    conn: &mut PgConnection,
    parent_id: i64,
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<CommentRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        CommentRow,
        "SELECT id, post_id, parent_id, author_id, floor, body, is_anonymous, anonymous_seed,
                like_count, pinned_at, accepted_at, hidden_at, created_at
         FROM comments
         WHERE parent_id = $1 AND hidden_at IS NULL
           AND ($2::timestamptz IS NULL OR (created_at, id) > ($2, $3::bigint))
         ORDER BY created_at, id
         LIMIT $4",
        parent_id,
        after_at,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}
