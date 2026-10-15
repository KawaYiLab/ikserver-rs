//! SQL。只被本 crate 的 service / interop 调用。

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

pub(crate) struct MediaRow {
    pub id: i64,
    pub object_key: String,
    pub mime: String,
    pub size_bytes: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn by_receipt(
    conn: &mut PgConnection,
    receipt: &str,
) -> sqlx::Result<Option<MediaRow>> {
    sqlx::query_as!(
        MediaRow,
        "SELECT id, object_key, mime, size_bytes, width, height, name, created_at
         FROM media_files WHERE receipt = $1",
        receipt,
    )
    .fetch_optional(conn)
    .await
}

/// 同内容的文件（最早的那个）。`owner` 给定时只找这个人上传过的（签名阶段，D-077 方案 A）。
pub(crate) async fn by_sha256(
    conn: &mut PgConnection,
    sha256: &str,
    owner: Option<i64>,
) -> sqlx::Result<Option<MediaRow>> {
    sqlx::query_as!(
        MediaRow,
        "SELECT f.id, f.object_key, f.mime, f.size_bytes, f.width, f.height, f.name, f.created_at
         FROM media_files f
         WHERE f.sha256 = $1
           AND ($2::bigint IS NULL OR EXISTS (
               SELECT 1 FROM media_uploaders u WHERE u.media_id = f.id AND u.user_id = $2))
         ORDER BY f.id
         LIMIT 1",
        sha256,
        owner,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct NewMedia<'a> {
    pub id: i64,
    pub object_key: &'a str,
    pub mime: &'a str,
    pub size_bytes: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub sha256: &'a str,
    pub receipt: &'a str,
    pub name: &'a str,
    pub now: DateTime<Utc>,
}

/// 写入文件行。收据冲突（同一令牌并发完成）时不写，返回 false。
pub(crate) async fn insert(conn: &mut PgConnection, m: &NewMedia<'_>) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "INSERT INTO media_files (id, object_key, mime, size_bytes, width, height, sha256,
                                  receipt, name, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (receipt) DO NOTHING",
        m.id,
        m.object_key,
        m.mime,
        m.size_bytes,
        m.width,
        m.height,
        m.sha256,
        m.receipt,
        m.name,
        m.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 把用户加入上传者（幂等）。
pub(crate) async fn add_uploader(
    conn: &mut PgConnection,
    media_id: i64,
    user_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO media_uploaders (media_id, user_id, created_at) VALUES ($1, $2, $3)
         ON CONFLICT (media_id, user_id) DO NOTHING",
        media_id,
        user_id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn remove_uploader(
    conn: &mut PgConnection,
    media_id: i64,
    user_id: i64,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "DELETE FROM media_uploaders WHERE media_id = $1 AND user_id = $2",
        media_id,
        user_id,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) struct GalleryRow {
    pub id: i64,
    pub object_key: String,
    pub mime: String,
    pub size_bytes: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub added_at: DateTime<Utc>,
}

/// 我的图库：按加入时间倒序（MEUP-001）。
pub(crate) async fn gallery(
    conn: &mut PgConnection,
    user_id: i64,
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<GalleryRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        GalleryRow,
        "SELECT f.id, f.object_key, f.mime, f.size_bytes, f.width, f.height, f.name, f.created_at,
                u.created_at AS added_at
         FROM media_uploaders u JOIN media_files f ON f.id = u.media_id
         WHERE u.user_id = $1
           AND ($2::timestamptz IS NULL OR (u.created_at, u.media_id) < ($2, $3::bigint))
         ORDER BY u.created_at DESC, u.media_id DESC
         LIMIT $4",
        user_id,
        after_at,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 这些文件中由 `user_id` 上传过的（引用媒体时的归属校验，D-012 方案 A）。
pub(crate) async fn owned_among(
    conn: &mut PgConnection,
    user_id: i64,
    ids: &[i64],
) -> sqlx::Result<Vec<MediaRow>> {
    sqlx::query_as!(
        MediaRow,
        "SELECT f.id, f.object_key, f.mime, f.size_bytes, f.width, f.height, f.name, f.created_at
         FROM media_files f JOIN media_uploaders u ON u.media_id = f.id AND u.user_id = $1
         WHERE f.id = ANY($2)",
        user_id,
        ids,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn by_ids(conn: &mut PgConnection, ids: &[i64]) -> sqlx::Result<Vec<MediaRow>> {
    sqlx::query_as!(
        MediaRow,
        "SELECT id, object_key, mime, size_bytes, width, height, name, created_at
         FROM media_files WHERE id = ANY($1)",
        ids,
    )
    .fetch_all(conn)
    .await
}
