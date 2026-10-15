use chrono::{DateTime, Utc};
use sqlx::PgConnection;

pub(crate) struct BindingRow {
    pub aid: String,
    pub zzz_uid: Option<String>,
    pub zzz_nickname: Option<String>,
    pub zzz_level: Option<i32>,
    pub zzz_region: Option<String>,
    pub zzz_region_name: Option<String>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn binding_of(
    conn: &mut PgConnection,
    user_id: i64,
) -> sqlx::Result<Option<BindingRow>> {
    sqlx::query_as!(
        BindingRow,
        "SELECT aid, zzz_uid, zzz_nickname, zzz_level, zzz_region, zzz_region_name,
                last_synced_at, created_at
         FROM mihoyo_bindings WHERE user_id = $1",
        user_id
    )
    .fetch_optional(conn)
    .await
}

/// aid 或 mid 指向的账号（去重）。
pub(crate) async fn users_by_identity(
    conn: &mut PgConnection,
    aid: &str,
    mid: Option<&str>,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT DISTINCT user_id FROM mihoyo_bindings
         WHERE aid = $1 OR ($2::text IS NOT NULL AND mid = $2)",
        aid,
        mid
    )
    .fetch_all(conn)
    .await
}

/// 写入或更新身份；mid 只补不清。
pub(crate) async fn upsert_identity(
    conn: &mut PgConnection,
    user_id: i64,
    aid: &str,
    mid: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO mihoyo_bindings (user_id, aid, mid) VALUES ($1, $2, $3)
         ON CONFLICT (user_id) DO UPDATE
         SET aid = EXCLUDED.aid, mid = COALESCE(EXCLUDED.mid, mihoyo_bindings.mid)",
        user_id,
        aid,
        mid
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct NewRole<'a> {
    pub uid: &'a str,
    pub nickname: &'a str,
    pub level: i32,
    pub region: &'a str,
    pub region_name: &'a str,
}

pub(crate) async fn sync_role(
    conn: &mut PgConnection,
    user_id: i64,
    role: &NewRole<'_>,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE mihoyo_bindings
         SET zzz_uid = $2, zzz_nickname = $3, zzz_level = $4, zzz_region = $5,
             zzz_region_name = $6, last_synced_at = $7
         WHERE user_id = $1",
        user_id,
        role.uid,
        role.nickname,
        role.level,
        role.region,
        role.region_name,
        now
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn delete_binding(conn: &mut PgConnection, user_id: i64) -> sqlx::Result<bool> {
    Ok(
        sqlx::query!("DELETE FROM mihoyo_bindings WHERE user_id = $1", user_id)
            .execute(conn)
            .await?
            .rows_affected()
            > 0,
    )
}

// ---------- 扫码会话 ----------

pub(crate) struct NewQr<'a> {
    pub id_hash: &'a [u8],
    pub ticket: &'a str,
    pub mode: &'a str,
    pub user_id: Option<i64>,
    pub device: serde_json::Value,
    pub expires_at: DateTime<Utc>,
}

pub(crate) async fn insert_qr(conn: &mut PgConnection, qr: NewQr<'_>) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO mihoyo_qr_logins (id_hash, ticket, mode, user_id, device, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
        qr.id_hash,
        qr.ticket,
        qr.mode,
        qr.user_id,
        qr.device,
        qr.expires_at
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct QrRow {
    pub ticket: String,
    pub mode: String,
    pub user_id: Option<i64>,
    pub device: serde_json::Value,
    pub expires_at: DateTime<Utc>,
    pub last_status: Option<String>,
    pub last_polled_at: Option<DateTime<Utc>>,
    pub claimed_until: Option<DateTime<Utc>>,
    pub result: Option<serde_json::Value>,
    pub result_expires_at: Option<DateTime<Utc>>,
}

pub(crate) async fn qr(conn: &mut PgConnection, id_hash: &[u8]) -> sqlx::Result<Option<QrRow>> {
    sqlx::query_as!(
        QrRow,
        "SELECT ticket, mode, user_id, device, expires_at, last_status, last_polled_at,
                claimed_until, result, result_expires_at
         FROM mihoyo_qr_logins WHERE id_hash = $1",
        id_hash
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct QrLockRow {
    pub mode: String,
    pub user_id: Option<i64>,
    pub result: Option<serde_json::Value>,
    pub result_expires_at: Option<DateTime<Utc>>,
}

pub(crate) async fn lock_qr(
    conn: &mut PgConnection,
    id_hash: &[u8],
) -> sqlx::Result<Option<QrLockRow>> {
    sqlx::query_as!(
        QrLockRow,
        "SELECT mode, user_id, result, result_expires_at FROM mihoyo_qr_logins
         WHERE id_hash = $1 FOR UPDATE",
        id_hash
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn delete_qr(conn: &mut PgConnection, id_hash: &[u8]) -> sqlx::Result<()> {
    sqlx::query!("DELETE FROM mihoyo_qr_logins WHERE id_hash = $1", id_hash)
        .execute(conn)
        .await?;
    Ok(())
}

pub(crate) async fn cache_status(
    conn: &mut PgConnection,
    id_hash: &[u8],
    status: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE mihoyo_qr_logins SET last_status = $2, last_polled_at = $3 WHERE id_hash = $1",
        id_hash,
        status,
        now
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 抢占「已确认」的处理权（MHY-007）。还没有结果、也没有别人正在处理时成功。
pub(crate) async fn claim(
    conn: &mut PgConnection,
    id_hash: &[u8],
    now: DateTime<Utc>,
    until: DateTime<Utc>,
) -> sqlx::Result<bool> {
    Ok(sqlx::query!(
        "UPDATE mihoyo_qr_logins SET claimed_until = $3
         WHERE id_hash = $1 AND result IS NULL
           AND (claimed_until IS NULL OR claimed_until < $2)",
        id_hash,
        now,
        until
    )
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

pub(crate) async fn store_result(
    conn: &mut PgConnection,
    id_hash: &[u8],
    result: serde_json::Value,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE mihoyo_qr_logins SET result = $2, result_expires_at = $3, claimed_until = NULL
         WHERE id_hash = $1",
        id_hash,
        result,
        expires_at
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 删除二维码与结果都已过期的扫码会话。
pub(crate) async fn prune(conn: &mut PgConnection, now: DateTime<Utc>) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "DELETE FROM mihoyo_qr_logins
         WHERE expires_at < $1 AND (result_expires_at IS NULL OR result_expires_at < $1)",
        now
    )
    .execute(conn)
    .await?
    .rows_affected())
}

pub(crate) struct ZzzRow {
    pub zzz_uid: Option<String>,
    pub zzz_nickname: Option<String>,
    pub zzz_level: Option<i32>,
    pub zzz_region_name: Option<String>,
}

pub(crate) async fn zzz_of(conn: &mut PgConnection, user_id: i64) -> sqlx::Result<Option<ZzzRow>> {
    sqlx::query_as!(
        ZzzRow,
        "SELECT zzz_uid, zzz_nickname, zzz_level, zzz_region_name
         FROM mihoyo_bindings WHERE user_id = $1",
        user_id
    )
    .fetch_optional(conn)
    .await
}
