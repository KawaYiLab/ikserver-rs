//! SQL。只被本 crate 的 service 调用。

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

pub(crate) struct ProfileRow {
    pub bio: String,
    pub hidden: bool,
    pub pinned_post_ids: Option<Vec<i64>>,
}

pub(crate) async fn get(conn: &mut PgConnection, user_id: i64) -> sqlx::Result<Option<ProfileRow>> {
    sqlx::query_as!(
        ProfileRow,
        "SELECT bio, hidden, pinned_post_ids FROM user_profiles WHERE user_id = $1",
        user_id,
    )
    .fetch_optional(conn)
    .await
}

/// 修改签名 / 隐藏（为 None 的字段保持不变），没有行时先按默认值创建。返回修改后的行。
pub(crate) async fn update_settings(
    conn: &mut PgConnection,
    user_id: i64,
    bio: Option<&str>,
    hidden: Option<bool>,
    now: DateTime<Utc>,
) -> sqlx::Result<ProfileRow> {
    sqlx::query_as!(
        ProfileRow,
        "INSERT INTO user_profiles (user_id, bio, hidden, updated_at)
         VALUES ($1, COALESCE($2, ''), COALESCE($3, FALSE), $4)
         ON CONFLICT (user_id) DO UPDATE SET
             bio = COALESCE($2, user_profiles.bio),
             hidden = COALESCE($3, user_profiles.hidden),
             updated_at = $4
         RETURNING bio, hidden, pinned_post_ids",
        user_id,
        bio,
        hidden,
        now,
    )
    .fetch_one(conn)
    .await
}

pub(crate) async fn set_pinned(
    conn: &mut PgConnection,
    user_id: i64,
    pinned: Option<&[i64]>,
    now: DateTime<Utc>,
) -> sqlx::Result<ProfileRow> {
    sqlx::query_as!(
        ProfileRow,
        "INSERT INTO user_profiles (user_id, pinned_post_ids, updated_at) VALUES ($1, $2, $3)
         ON CONFLICT (user_id) DO UPDATE SET pinned_post_ids = $2, updated_at = $3
         RETURNING bio, hidden, pinned_post_ids",
        user_id,
        pinned,
        now,
    )
    .fetch_one(conn)
    .await
}

/// `ids` 中隐藏了主页的用户。
pub(crate) async fn hidden_among(conn: &mut PgConnection, ids: &[i64]) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT user_id FROM user_profiles WHERE user_id = ANY($1) AND hidden",
        ids,
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct BioRow {
    pub user_id: i64,
    pub bio: String,
}

pub(crate) async fn bios(conn: &mut PgConnection, ids: &[i64]) -> sqlx::Result<Vec<BioRow>> {
    sqlx::query_as!(
        BioRow,
        "SELECT user_id, bio FROM user_profiles WHERE user_id = ANY($1)",
        ids,
    )
    .fetch_all(conn)
    .await
}

/// 注销：清空签名与精选、隐藏主页（DEL-006）。没有资料行时什么都不做。
pub(crate) async fn clear(conn: &mut PgConnection, user_id: i64) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE user_profiles SET bio = '', hidden = TRUE, pinned_post_ids = NULL, updated_at = now()
         WHERE user_id = $1",
        user_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 清空签名。返回原本是否有签名。
pub(crate) async fn clear_bio(conn: &mut PgConnection, user_id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE user_profiles SET bio = '', updated_at = now() WHERE user_id = $1 AND bio <> ''",
        user_id
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}
