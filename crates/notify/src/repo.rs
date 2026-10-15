//! SQL。只被本 crate 的 service / interop 调用。

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

pub(crate) struct NewRow<'a> {
    pub id: i64,
    pub recipient_id: i64,
    pub sender_id: i64,
    pub kind: &'a str,
    pub bucket_key: &'a str,
    pub post_id: Option<i64>,
    pub comment_id: Option<i64>,
    pub post_title: Option<&'a str>,
    pub excerpt: &'a str,
    pub now: DateTime<Utc>,
}

pub(crate) async fn insert(conn: &mut PgConnection, row: &NewRow<'_>) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO notifications (id, recipient_id, sender_id, kind, bucket_key, post_id,
                                    comment_id, post_title, excerpt, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        row.id,
        row.recipient_id,
        row.sender_id,
        row.kind,
        row.bucket_key,
        row.post_id,
        row.comment_id,
        row.post_title,
        row.excerpt,
        row.now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct NewSystemBatch<'a> {
    pub ids: &'a [i64],
    pub recipients: &'a [i64],
    pub post_id: Option<i64>,
    pub post_title: Option<&'a str>,
    pub excerpt: &'a str,
    /// 幂等键；为空时不去重。
    pub dedup_key: Option<&'a str>,
    pub now: DateTime<Utc>,
}

/// 批量写系统通知，按 `(recipient_id, dedup_key)` 幂等。返回真正写入的收件人。
pub(crate) async fn insert_system_batch(
    conn: &mut PgConnection,
    batch: &NewSystemBatch<'_>,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "INSERT INTO notifications (id, recipient_id, kind, bucket_key, post_id, post_title,
                                    excerpt, dedup_key, created_at)
         SELECT t.id, t.recipient_id, 'system', 'system', $3, $4, $5, $6, $7
         FROM unnest($1::bigint[], $2::bigint[]) AS t(id, recipient_id)
         ON CONFLICT (recipient_id, dedup_key) WHERE dedup_key IS NOT NULL DO NOTHING
         RETURNING recipient_id",
        batch.ids,
        batch.recipients,
        batch.post_id,
        batch.post_title,
        batch.excerpt,
        batch.dedup_key,
        batch.now,
    )
    .fetch_all(conn)
    .await
}

/// 撤回某人对某个目标的某类通知（取消点赞、取消收藏），返回受影响的收件人。
/// 目标是评论时传 `comment_id`；目标是帖子时传 `post_id`、`comment_id = None`。
pub(crate) async fn delete_by_target(
    conn: &mut PgConnection,
    sender_id: i64,
    kind: &str,
    post_id: Option<i64>,
    comment_id: Option<i64>,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "DELETE FROM notifications
         WHERE sender_id = $1 AND kind = $2
           AND CASE WHEN $4::bigint IS NULL THEN comment_id IS NULL AND post_id = $3
                    ELSE comment_id = $4 END
         RETURNING recipient_id",
        sender_id,
        kind,
        post_id,
        comment_id,
    )
    .fetch_all(conn)
    .await
}

// ---------- 读取 ----------

pub(crate) struct NotificationRow {
    pub id: i64,
    pub sender_id: Option<i64>,
    pub kind: String,
    pub bucket_key: String,
    pub post_id: Option<i64>,
    pub comment_id: Option<i64>,
    pub post_title: Option<String>,
    pub excerpt: String,
    pub is_read: bool,
    pub created_at: DateTime<Utc>,
}

/// 我的通知（或某个分组内的通知），新的在前。`unread_only` 时只取未读。
/// 游标 `(时间, id)` 严格更小；id 为 None 时取严格早于该时间的（游标落在收件箱时间线的私信消息上，
/// 同一时刻的通知排在消息前面，已经出过了）。
pub(crate) async fn list(
    conn: &mut PgConnection,
    recipient_id: i64,
    bucket_key: Option<&str>,
    unread_only: bool,
    after: Option<(DateTime<Utc>, Option<i64>)>,
    limit: i64,
) -> sqlx::Result<Vec<NotificationRow>> {
    let (after_at, after_id) = after.unzip();
    let after_id = after_id.flatten();
    sqlx::query_as!(
        NotificationRow,
        "SELECT id, sender_id, kind, bucket_key, post_id, comment_id, post_title, excerpt,
                is_read, created_at
         FROM notifications
         WHERE recipient_id = $1
           AND ($2::text IS NULL OR bucket_key = $2)
           AND (NOT $3 OR NOT is_read)
           AND ($4::timestamptz IS NULL OR created_at < $4
                OR (created_at = $4 AND $5::bigint IS NOT NULL AND id < $5))
         ORDER BY created_at DESC, id DESC
         LIMIT $6",
        recipient_id,
        bucket_key,
        unread_only,
        after_at,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn unread_count(conn: &mut PgConnection, recipient_id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM notifications WHERE recipient_id = $1 AND NOT is_read"#,
        recipient_id,
    )
    .fetch_one(conn)
    .await
}

pub(crate) struct BucketRow {
    pub bucket_key: String,
    pub last_at: DateTime<Utc>,
    pub kind: String,
    pub comment_id: Option<i64>,
    pub excerpt: String,
    pub unread: i64,
}

/// 敲敲会话：按分组键聚合，按最后一条通知的时间倒序。
///
/// 分组键用递归 CTE 做松散索引扫描（代价与「不同发送者数」成正比，而不是通知总数），
/// 每组的最后一条与未读数走 `notifications_bucket_idx` / `notifications_unread_idx`。
/// 删除通知后分组自然一致，不需要维护分组表（旧系统分组表回退逻辑的坑见 NOTIF-025）。
pub(crate) async fn buckets(
    conn: &mut PgConnection,
    recipient_id: i64,
    after: Option<(DateTime<Utc>, String)>,
    limit: i64,
) -> sqlx::Result<Vec<BucketRow>> {
    let (after_at, after_key) = after.unzip();
    sqlx::query_as!(
        BucketRow,
        r#"WITH RECURSIVE keys(bucket_key) AS (
               (SELECT n.bucket_key FROM notifications n
                WHERE n.recipient_id = $1 ORDER BY n.bucket_key LIMIT 1)
               UNION ALL
               SELECT (SELECT n.bucket_key FROM notifications n
                       WHERE n.recipient_id = $1 AND n.bucket_key > k.bucket_key
                       ORDER BY n.bucket_key LIMIT 1)
               FROM keys k WHERE k.bucket_key IS NOT NULL
           )
           SELECT k.bucket_key AS "bucket_key!", l.created_at AS "last_at!", l.kind AS "kind!",
                  l.comment_id AS "comment_id?", l.excerpt AS "excerpt!",
                  (SELECT count(*) FROM notifications u
                   WHERE u.recipient_id = $1 AND u.bucket_key = k.bucket_key AND NOT u.is_read) AS "unread!"
           FROM keys k
           CROSS JOIN LATERAL (
               SELECT n.created_at, n.kind, n.comment_id, n.excerpt
               FROM notifications n
               WHERE n.recipient_id = $1 AND n.bucket_key = k.bucket_key
               ORDER BY n.created_at DESC, n.id DESC
               LIMIT 1
           ) l
           WHERE k.bucket_key IS NOT NULL
             AND ($2::timestamptz IS NULL OR (l.created_at, k.bucket_key) < ($2, $3::text))
           ORDER BY l.created_at DESC, k.bucket_key DESC
           LIMIT $4"#,
        recipient_id,
        after_at,
        after_key,
        limit,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn mark_read(
    conn: &mut PgConnection,
    recipient_id: i64,
    id: i64,
) -> sqlx::Result<Option<bool>> {
    // 返回 None：不存在或不是我的；Some(true)：这次从未读变已读。
    let row = sqlx::query_scalar!(
        r#"WITH target AS (
               SELECT id, is_read FROM notifications WHERE id = $2 AND recipient_id = $1
           ), updated AS (
               UPDATE notifications n SET is_read = TRUE
               FROM target t WHERE n.id = t.id AND NOT t.is_read
               RETURNING n.id
           )
           SELECT EXISTS (SELECT 1 FROM updated) AS "changed!" FROM target"#,
        recipient_id,
        id,
    )
    .fetch_optional(conn)
    .await?;
    Ok(row)
}

/// 全部（或某个分组内）标为已读，返回更新条数。`up_to` 给定时只标这个时刻及之前的，
/// 不误标并发到达的新通知（CONV-035）。
pub(crate) async fn mark_all_read(
    conn: &mut PgConnection,
    recipient_id: i64,
    bucket_key: Option<&str>,
    up_to: Option<DateTime<Utc>>,
) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        "UPDATE notifications SET is_read = TRUE
         WHERE recipient_id = $1 AND NOT is_read AND ($2::text IS NULL OR bucket_key = $2)
           AND ($3::timestamptz IS NULL OR created_at <= $3)",
        recipient_id,
        bucket_key,
        up_to,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

// ---------- 统一收件箱 ----------

/// 收件箱里的通知分组：排除 `excluded`（与之有在场私聊的人，他们的通知合并进私聊会话），
/// 按 `(最后一条时间, 分组键)` 倒序，严格早于游标。游标的分组键为 None 时取严格早于该时间的
/// （游标落在私聊会话上，同一时刻的分组排在会话前面，已经出过了）。
pub(crate) async fn inbox_buckets(
    conn: &mut PgConnection,
    recipient_id: i64,
    excluded: &[String],
    after: Option<(DateTime<Utc>, Option<String>)>,
    limit: i64,
) -> sqlx::Result<Vec<BucketRow>> {
    let (after_at, after_key) = after.unzip();
    let after_key = after_key.flatten();
    sqlx::query_as!(
        BucketRow,
        r#"WITH RECURSIVE keys(bucket_key) AS (
               (SELECT n.bucket_key FROM notifications n
                WHERE n.recipient_id = $1 ORDER BY n.bucket_key LIMIT 1)
               UNION ALL
               SELECT (SELECT n.bucket_key FROM notifications n
                       WHERE n.recipient_id = $1 AND n.bucket_key > k.bucket_key
                       ORDER BY n.bucket_key LIMIT 1)
               FROM keys k WHERE k.bucket_key IS NOT NULL
           )
           SELECT k.bucket_key AS "bucket_key!", l.created_at AS "last_at!", l.kind AS "kind!",
                  l.comment_id AS "comment_id?", l.excerpt AS "excerpt!",
                  (SELECT count(*) FROM notifications u
                   WHERE u.recipient_id = $1 AND u.bucket_key = k.bucket_key AND NOT u.is_read) AS "unread!"
           FROM keys k
           CROSS JOIN LATERAL (
               SELECT n.created_at, n.kind, n.comment_id, n.excerpt
               FROM notifications n
               WHERE n.recipient_id = $1 AND n.bucket_key = k.bucket_key
               ORDER BY n.created_at DESC, n.id DESC
               LIMIT 1
           ) l
           WHERE k.bucket_key IS NOT NULL AND k.bucket_key <> ALL($2)
             AND ($3::timestamptz IS NULL OR l.created_at < $3
                  OR (l.created_at = $3 AND $4::text IS NOT NULL AND k.bucket_key < $4))
           ORDER BY l.created_at DESC, k.bucket_key DESC
           LIMIT $5"#,
        recipient_id,
        excluded,
        after_at,
        after_key,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 指定分组的最后一条与未读数（没有通知的分组不出现）。用于合并进私聊会话。
pub(crate) async fn bucket_stats(
    conn: &mut PgConnection,
    recipient_id: i64,
    keys: &[String],
) -> sqlx::Result<Vec<BucketRow>> {
    sqlx::query_as!(
        BucketRow,
        r#"SELECT k.bucket_key AS "bucket_key!", l.created_at AS "last_at!", l.kind AS "kind!",
                  l.comment_id AS "comment_id?", l.excerpt AS "excerpt!",
                  (SELECT count(*) FROM notifications u
                   WHERE u.recipient_id = $1 AND u.bucket_key = k.bucket_key AND NOT u.is_read) AS "unread!"
           FROM unnest($2::text[]) AS k(bucket_key)
           CROSS JOIN LATERAL (
               SELECT n.created_at, n.kind, n.comment_id, n.excerpt
               FROM notifications n
               WHERE n.recipient_id = $1 AND n.bucket_key = k.bucket_key
               ORDER BY n.created_at DESC, n.id DESC
               LIMIT 1
           ) l"#,
        recipient_id,
        keys,
    )
    .fetch_all(conn)
    .await
}
