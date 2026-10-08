//! 认证域的全部 SQL。只在本 crate 内可见（AGENTS.md 规则 5）。

use chrono::{DateTime, Utc};
use ik_kernel::Role;
use sqlx::PgConnection;

// ---------- 邮箱验证码 ----------

pub(crate) struct OpenCode {
    pub id: i64,
    pub code_hash: Vec<u8>,
    pub attempts: i32,
    pub sent_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// 最新一条未消费的验证码（同秒双发取 id 大者）。
pub(crate) async fn latest_open_code(
    conn: &mut PgConnection,
    email: &str,
    purpose: &str,
) -> sqlx::Result<Option<OpenCode>> {
    sqlx::query_as!(
        OpenCode,
        r#"SELECT id, code_hash, attempts, sent_at, expires_at
           FROM email_codes
           WHERE email = $1 AND purpose = $2 AND consumed_at IS NULL
           ORDER BY sent_at DESC, id DESC
           LIMIT 1"#,
        email,
        purpose,
    )
    .fetch_optional(conn)
    .await
}

/// 同上，但加行锁：并发校验同一验证码时串行执行，计数与消费不会互相覆盖。
pub(crate) async fn lock_latest_open_code(
    conn: &mut PgConnection,
    email: &str,
    purpose: &str,
) -> sqlx::Result<Option<OpenCode>> {
    sqlx::query_as!(
        OpenCode,
        r#"SELECT id, code_hash, attempts, sent_at, expires_at
           FROM email_codes
           WHERE email = $1 AND purpose = $2 AND consumed_at IS NULL
           ORDER BY sent_at DESC, id DESC
           LIMIT 1
           FOR UPDATE"#,
        email,
        purpose,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn count_codes_from_ip_since(
    conn: &mut PgConnection,
    ip: &str,
    since: DateTime<Utc>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM email_codes WHERE request_ip = $1 AND sent_at > $2"#,
        ip,
        since,
    )
    .fetch_one(conn)
    .await
}

/// 发新码前废弃同邮箱同用途的旧码（CODE-005）。
pub(crate) async fn invalidate_open_codes(
    conn: &mut PgConnection,
    email: &str,
    purpose: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE email_codes SET consumed_at = $3
         WHERE email = $1 AND purpose = $2 AND consumed_at IS NULL",
        email,
        purpose,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "一一对应表列，拆结构体反而更难读"
)]
pub(crate) async fn insert_code(
    conn: &mut PgConnection,
    id: i64,
    email: &str,
    purpose: &str,
    code_hash: &[u8],
    sent_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    request_ip: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO email_codes (id, email, purpose, code_hash, sent_at, expires_at, request_ip)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        id,
        email,
        purpose,
        code_hash,
        sent_at,
        expires_at,
        request_ip,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn force_consume_code(
    conn: &mut PgConnection,
    id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE email_codes SET consumed_at = $2 WHERE id = $1 AND consumed_at IS NULL",
        id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 错误尝试 +1，达到上限的当次同时作废（CODE-010）。返回递增后的次数；行已失效时返回 None。
pub(crate) async fn bump_code_attempts(
    conn: &mut PgConnection,
    id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<Option<i32>> {
    sqlx::query_scalar!(
        "UPDATE email_codes
         SET attempts = attempts + 1,
             consumed_at = CASE WHEN attempts + 1 >= 5 THEN $2 ELSE consumed_at END
         WHERE id = $1 AND consumed_at IS NULL AND attempts < 5
         RETURNING attempts",
        id,
        now,
    )
    .fetch_optional(conn)
    .await
}

/// 原子消费（CODE-009）：只有仍有效的码才能被消费一次。返回是否成功。
pub(crate) async fn consume_code(
    conn: &mut PgConnection,
    id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE email_codes SET consumed_at = $2
         WHERE id = $1 AND consumed_at IS NULL AND attempts < 5 AND expires_at > $2",
        id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

// ---------- 用户 ----------

pub(crate) async fn email_taken(conn: &mut PgConnection, email: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM users WHERE email = $1) AS "exists!""#,
        email,
    )
    .fetch_one(conn)
    .await
}

pub(crate) async fn active_user_id_by_email(
    conn: &mut PgConnection,
    email: &str,
) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar!(
        "SELECT id FROM users WHERE email = $1 AND status = 'active'",
        email,
    )
    .fetch_optional(conn)
    .await
}

/// 插入用户。用户名冲突时返回 false（调用方换一个再试）；邮箱冲突抛唯一约束错误。
pub(crate) async fn insert_user(
    conn: &mut PgConnection,
    id: i64,
    username: &str,
    email: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "INSERT INTO users (id, username, email, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $4)
         ON CONFLICT ON CONSTRAINT users_username_uq DO NOTHING",
        id,
        username,
        email,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub(crate) async fn upsert_password(
    conn: &mut PgConnection,
    user_id: i64,
    hash: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO user_passwords (user_id, hash, updated_at) VALUES ($1, $2, $3)
         ON CONFLICT (user_id) DO UPDATE SET hash = EXCLUDED.hash, updated_at = EXCLUDED.updated_at",
        user_id,
        hash,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct LoginCandidate {
    pub id: i64,
    pub status: String,
    pub hash: Option<String>,
}

pub(crate) async fn login_candidate_by_email(
    conn: &mut PgConnection,
    email: &str,
) -> sqlx::Result<Option<LoginCandidate>> {
    sqlx::query_as!(
        LoginCandidate,
        r#"SELECT u.id, u.status, p.hash AS "hash?"
           FROM users u LEFT JOIN user_passwords p ON p.user_id = u.id
           WHERE u.email = $1"#,
        email,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn login_candidate_by_username(
    conn: &mut PgConnection,
    username: &str,
) -> sqlx::Result<Option<LoginCandidate>> {
    sqlx::query_as!(
        LoginCandidate,
        r#"SELECT u.id, u.status, p.hash AS "hash?"
           FROM users u LEFT JOIN user_passwords p ON p.user_id = u.id
           WHERE u.username = $1"#,
        username,
    )
    .fetch_optional(conn)
    .await
}

/// 锁住用户行，串行化同一用户的会话变更（SESS-005）。用户不存在或非活跃时返回 None。
pub(crate) async fn lock_active_user(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar!(
        "SELECT id FROM users WHERE id = $1 AND status = 'active' FOR UPDATE",
        id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct MeRow {
    pub id: i64,
    pub username: String,
    pub email: Option<String>,
    pub role: Role,
    pub exam_passed_at: Option<DateTime<Utc>>,
    pub is_ai_agent: bool,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn me(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<MeRow>> {
    sqlx::query_as!(
        MeRow,
        r#"SELECT id, username, email, role AS "role: Role", exam_passed_at, is_ai_agent, created_at
           FROM users WHERE id = $1"#,
        id,
    )
    .fetch_optional(conn)
    .await
}

// ---------- 会话 ----------

/// 只保留最近活跃的 `keep` 个有效会话，其余撤销（SESS-004）。在插入新会话之前调用。
pub(crate) async fn revoke_overflow_sessions(
    conn: &mut PgConnection,
    user_id: i64,
    keep: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        "UPDATE sessions SET revoked_at = $3, revoke_reason = 'overflow'
         WHERE id IN (
             SELECT id FROM sessions
             WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > $3
             ORDER BY last_seen_at DESC, id DESC
             OFFSET $2
         )",
        user_id,
        keep,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

pub(crate) struct NewSession<'a> {
    pub id: i64,
    pub user_id: i64,
    pub token_hash: &'a [u8],
    pub login_method: &'a str,
    pub user_agent: Option<&'a str>,
    pub ip: &'a str,
    pub now: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub max_expires_at: DateTime<Utc>,
}

pub(crate) async fn insert_session(conn: &mut PgConnection, s: NewSession<'_>) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO sessions
             (id, user_id, token_hash, login_method, user_agent, ip,
              created_at, last_seen_at, expires_at, max_expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $7, $8, $9)",
        s.id,
        s.user_id,
        s.token_hash,
        s.login_method,
        s.user_agent,
        s.ip,
        s.now,
        s.expires_at,
        s.max_expires_at,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct ResolvedSession {
    pub session_id: i64,
    pub user_id: i64,
    pub last_seen_at: DateTime<Utc>,
    pub status: String,
    pub role: Role,
    pub exam_passed_at: Option<DateTime<Utc>>,
    pub is_ai_agent: bool,
}

pub(crate) async fn resolve_session(
    conn: &mut PgConnection,
    token_hash: &[u8],
    now: DateTime<Utc>,
) -> sqlx::Result<Option<ResolvedSession>> {
    sqlx::query_as!(
        ResolvedSession,
        r#"SELECT s.id AS session_id, s.user_id, s.last_seen_at,
                  u.status, u.role AS "role: Role", u.exam_passed_at, u.is_ai_agent
           FROM sessions s JOIN users u ON u.id = s.user_id
           WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > $2"#,
        token_hash,
        now,
    )
    .fetch_optional(conn)
    .await
}

/// 滑动续期：过期时间推到 `new_expires_at`，但不超过绝对上限。
pub(crate) async fn touch_session(
    conn: &mut PgConnection,
    id: i64,
    now: DateTime<Utc>,
    new_expires_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE sessions SET last_seen_at = $2, expires_at = LEAST($3, max_expires_at)
         WHERE id = $1 AND revoked_at IS NULL",
        id,
        now,
        new_expires_at,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct SessionRow {
    pub id: i64,
    pub login_method: String,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
    pub location: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

pub(crate) async fn session_by_id(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<SessionRow>> {
    sqlx::query_as!(
        SessionRow,
        "SELECT id, login_method, user_agent, ip, location, created_at, last_seen_at, expires_at
         FROM sessions WHERE id = $1",
        id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn active_sessions(
    conn: &mut PgConnection,
    user_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<Vec<SessionRow>> {
    sqlx::query_as!(
        SessionRow,
        "SELECT id, login_method, user_agent, ip, location, created_at, last_seen_at, expires_at
         FROM sessions
         WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > $2
         ORDER BY last_seen_at DESC, id DESC
         LIMIT 100",
        user_id,
        now,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn revoke_session(
    conn: &mut PgConnection,
    user_id: i64,
    session_id: i64,
    now: DateTime<Utc>,
    reason: &str,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE sessions SET revoked_at = $3, revoke_reason = $4
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
        session_id,
        user_id,
        now,
        reason,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// 撤销用户的全部会话；`except` 为要保留的会话（如"下线其他设备"时保留当前会话）。
pub(crate) async fn revoke_user_sessions(
    conn: &mut PgConnection,
    user_id: i64,
    except: Option<i64>,
    now: DateTime<Utc>,
    reason: &str,
) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        "UPDATE sessions SET revoked_at = $3, revoke_reason = $4
         WHERE user_id = $1 AND revoked_at IS NULL AND ($2::BIGINT IS NULL OR id <> $2)",
        user_id,
        except,
        now,
        reason,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}
