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

/// 没有邮箱的账号（米游社首登，ACC-013）。用户名冲突时返回 false。
pub(crate) async fn insert_user_without_email(
    conn: &mut PgConnection,
    id: i64,
    username: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "INSERT INTO users (id, username, created_at, updated_at)
         VALUES ($1, $2, $3, $3)
         ON CONFLICT ON CONSTRAINT users_username_uq DO NOTHING",
        id,
        username,
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

pub(crate) struct UserStateRow {
    pub id: i64,
    pub role: Role,
    pub is_ai_agent: bool,
    pub exam_passed_at: Option<DateTime<Utc>>,
    pub level: i16,
}

/// 锁住活跃用户行并读出权限相关状态（供其他域在自己的事务里使用）。
///
/// 用 `FOR NO KEY UPDATE` 而不是 `FOR UPDATE`：调用方的事务往往先插入了引用该用户的行
/// （帖子、评论……），外键检查会对用户行加 `FOR KEY SHARE`。`FOR UPDATE` 与之冲突，
/// 同一用户的两个并发事务会互相等待对方的 KEY SHARE 而死锁；`FOR NO KEY UPDATE` 与之兼容，
/// 彼此之间仍然互斥，足以串行化同一用户的经验、丁尼写入。
pub(crate) async fn lock_active_user_state(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<UserStateRow>> {
    sqlx::query_as!(
        UserStateRow,
        r#"SELECT id, role AS "role: Role", is_ai_agent, exam_passed_at, level
           FROM users WHERE id = $1 AND status = 'active' FOR NO KEY UPDATE"#,
        id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct UserRefRow {
    pub id: i64,
    pub status: String,
    pub is_ai_agent: bool,
}

pub(crate) async fn user_ref(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<UserRefRow>> {
    sqlx::query_as!(
        UserRefRow,
        "SELECT id, status, is_ai_agent FROM users WHERE id = $1",
        id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct ActiveUserRow {
    pub id: i64,
    pub username: String,
    pub is_ai_agent: bool,
}

pub(crate) async fn active_users(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<ActiveUserRow>> {
    sqlx::query_as!(
        ActiveUserRow,
        "SELECT id, username, is_ai_agent FROM users WHERE id = ANY($1) AND status = 'active'",
        ids,
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct ProfileUserRow {
    pub id: i64,
    pub username: String,
    pub level: i16,
    pub status: String,
    pub is_ai_agent: bool,
    pub created_at: DateTime<Utc>,
    pub avatar_media_id: Option<i64>,
    pub equipped_avatar_id: Option<i64>,
    pub equipped_card_id: Option<i64>,
}

pub(crate) async fn profile_user(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<ProfileUserRow>> {
    sqlx::query_as!(
        ProfileUserRow,
        "SELECT id, username, level, status, is_ai_agent, created_at, avatar_media_id,
                equipped_avatar_id, equipped_card_id
         FROM users WHERE id = $1",
        id,
    )
    .fetch_optional(conn)
    .await
}

/// 只改状态正常的账号；返回是否命中。
pub(crate) async fn rename(
    conn: &mut PgConnection,
    id: i64,
    username: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE users SET username = $2, updated_at = $3 WHERE id = $1 AND status = 'active'",
        id,
        username,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) struct FoundUserRow {
    pub id: i64,
    pub username: String,
    pub level: i16,
}

/// `query` 原文用于完全匹配与前缀排序，`escaped` 是转义过 LIKE 元字符的同一个词。
pub(crate) async fn search_users(
    conn: &mut PgConnection,
    query: &str,
    escaped: &str,
    limit: i64,
) -> sqlx::Result<Vec<FoundUserRow>> {
    sqlx::query_as!(
        FoundUserRow,
        r"SELECT id, username, level FROM users
          WHERE status = 'active' AND username ILIKE '%' || $2 || '%' ESCAPE '\'
          ORDER BY (lower(username) = lower($1)) DESC,
                   (username ILIKE $2 || '%' ESCAPE '\') DESC,
                   username, id
          LIMIT $3",
        query,
        escaped,
        limit,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn exam_passed_at(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<Option<DateTime<Utc>>>> {
    sqlx::query_scalar!("SELECT exam_passed_at FROM users WHERE id = $1", id)
        .fetch_optional(conn)
        .await
}

pub(crate) async fn set_level(
    conn: &mut PgConnection,
    id: i64,
    level: i16,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE users SET level = $2, updated_at = $3 WHERE id = $1 AND level <> $2",
        id,
        level,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 首次通过考试时写入通过时间；已通过的不改写。返回是否命中（即首次通过）。
pub(crate) async fn mark_exam_passed(
    conn: &mut PgConnection,
    id: i64,
    at: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE users SET exam_passed_at = $2, updated_at = $2
         WHERE id = $1 AND exam_passed_at IS NULL",
        id,
        at,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) struct MeRow {
    pub id: i64,
    pub username: String,
    pub email: Option<String>,
    pub role: Role,
    pub exam_passed_at: Option<DateTime<Utc>>,
    pub is_ai_agent: bool,
    pub level: i16,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn me(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<MeRow>> {
    sqlx::query_as!(
        MeRow,
        r#"SELECT id, username, email, role AS "role: Role", exam_passed_at, is_ai_agent, level, created_at
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
    pub level: i16,
}

pub(crate) async fn resolve_session(
    conn: &mut PgConnection,
    token_hash: &[u8],
    now: DateTime<Utc>,
) -> sqlx::Result<Option<ResolvedSession>> {
    sqlx::query_as!(
        ResolvedSession,
        r#"SELECT s.id AS session_id, s.user_id, s.last_seen_at,
                  u.status, u.role AS "role: Role", u.exam_passed_at, u.is_ai_agent, u.level
           FROM sessions s JOIN users u ON u.id = s.user_id
           WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > $2"#,
        token_hash,
        now,
    )
    .fetch_optional(conn)
    .await
}

/// 会话仍然有效：未吊销、未过期，且账号状态正常。
pub(crate) async fn session_active(
    conn: &mut PgConnection,
    user_id: i64,
    session_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM sessions s JOIN users u ON u.id = s.user_id
               WHERE s.id = $2 AND s.user_id = $1 AND s.revoked_at IS NULL
                 AND s.expires_at > $3 AND u.status = 'active'
           ) AS "active!""#,
        user_id,
        session_id,
        now,
    )
    .fetch_one(conn)
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

pub(crate) struct SessionByHashRow {
    pub user_id: i64,
    pub revoked_at: Option<DateTime<Utc>>,
    pub session: SessionRow,
}

/// 按令牌摘要找会话（不论是否已吊销、过期）。
pub(crate) async fn session_by_hash(
    conn: &mut PgConnection,
    token_hash: &[u8],
) -> sqlx::Result<Option<SessionByHashRow>> {
    let row = sqlx::query!(
        "SELECT id, user_id, revoked_at, login_method, user_agent, ip, location, created_at,
                last_seen_at, expires_at
         FROM sessions WHERE token_hash = $1",
        token_hash,
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|r| SessionByHashRow {
        user_id: r.user_id,
        revoked_at: r.revoked_at,
        session: SessionRow {
            id: r.id,
            login_method: r.login_method,
            user_agent: r.user_agent,
            ip: r.ip,
            location: r.location,
            created_at: r.created_at,
            last_seen_at: r.last_seen_at,
            expires_at: r.expires_at,
        },
    }))
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

// ---------- 供其他域读取的公开资料 ----------

pub(crate) struct AuthorRow {
    pub id: i64,
    pub username: String,
    pub status: String,
    pub level: i16,
    pub avatar_media_id: Option<i64>,
}

pub(crate) async fn authors(conn: &mut PgConnection, ids: &[i64]) -> sqlx::Result<Vec<AuthorRow>> {
    sqlx::query_as!(
        AuthorRow,
        r#"SELECT id, username, status, level,
                  COALESCE(equipped_avatar_media_id, avatar_media_id) AS avatar_media_id
           FROM users WHERE id = ANY($1)"#,
        ids,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn set_role_by_email(
    conn: &mut PgConnection,
    email: &str,
    role: Role,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE users SET role = $2, updated_at = $3 WHERE email = $1",
        email,
        role as Role,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) struct UsernameRow {
    pub id: i64,
    pub username: String,
}

/// 按用户名精确匹配（大小写敏感，D-018），只返回正常状态的账号。
pub(crate) async fn active_users_by_usernames(
    conn: &mut PgConnection,
    names: &[String],
) -> sqlx::Result<Vec<UsernameRow>> {
    sqlx::query_as!(
        UsernameRow,
        "SELECT id, username FROM users WHERE username = ANY($1) AND status = 'active'",
        names,
    )
    .fetch_all(conn)
    .await
}

/// 设置自定义头像并卸下系统头像（COSMETIC-006）。只改状态正常的账号；返回是否命中。
pub(crate) async fn set_avatar(
    conn: &mut PgConnection,
    user_id: i64,
    media_id: i64,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE users SET avatar_media_id = $2, equipped_avatar_id = NULL, equipped_avatar_media_id = NULL
         WHERE id = $1 AND status = 'active'",
        user_id,
        media_id,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 清除自定义头像（装备的系统头像不动）。
pub(crate) async fn clear_avatar(conn: &mut PgConnection, user_id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE users SET avatar_media_id = NULL WHERE id = $1 AND status = 'active'",
        user_id,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn equip_avatar(
    conn: &mut PgConnection,
    user_id: i64,
    avatar: Option<(i64, i64)>,
) -> sqlx::Result<bool> {
    let (avatar_id, media_id) = avatar.unzip();
    let result = sqlx::query!(
        "UPDATE users SET equipped_avatar_id = $2, equipped_avatar_media_id = $3
         WHERE id = $1 AND status = 'active'",
        user_id,
        avatar_id,
        media_id,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn equip_card(
    conn: &mut PgConnection,
    user_id: i64,
    card_id: Option<i64>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE users SET equipped_card_id = $2 WHERE id = $1 AND status = 'active'",
        user_id,
        card_id,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn refresh_equipped_avatar(
    conn: &mut PgConnection,
    avatar_id: i64,
    media_id: i64,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "UPDATE users SET equipped_avatar_media_id = $2
         WHERE equipped_avatar_id = $1 AND equipped_avatar_media_id <> $2",
        avatar_id,
        media_id,
    )
    .execute(conn)
    .await?
    .rows_affected())
}

pub(crate) async fn has_email(conn: &mut PgConnection, user_id: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND email IS NOT NULL) AS "has!""#,
        user_id
    )
    .fetch_one(conn)
    .await
}

pub(crate) async fn password_hash(
    conn: &mut PgConnection,
    user_id: i64,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar!(
        "SELECT hash FROM user_passwords WHERE user_id = $1",
        user_id
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn set_email(
    conn: &mut PgConnection,
    user_id: i64,
    email: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE users SET email = $2, updated_at = $3 WHERE id = $1",
        user_id,
        email,
        now
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 注销的账号脱敏（DEL-006）：改名、清邮箱（邮箱可以再注册）、清头像与装扮、状态改为 deleted，删除密码。
pub(crate) async fn anonymize(
    conn: &mut PgConnection,
    user_id: i64,
    username: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE users
         SET username = $2, email = NULL, status = 'deleted', avatar_media_id = NULL,
             equipped_avatar_id = NULL, equipped_avatar_media_id = NULL, equipped_card_id = NULL,
             updated_at = $3
         WHERE id = $1",
        user_id,
        username,
        now
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!("DELETE FROM user_passwords WHERE user_id = $1", user_id)
        .execute(conn)
        .await?;
    Ok(())
}

// ---------- 审核（moderation） ----------

pub(crate) struct UserStatusRow {
    pub id: i64,
    pub username: String,
    pub status: String,
    pub role: Role,
}

pub(crate) async fn user_statuses(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<UserStatusRow>> {
    sqlx::query_as!(
        UserStatusRow,
        r#"SELECT id, username, status, role AS "role: Role" FROM users WHERE id = ANY($1)"#,
        ids
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn lock_user_status(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<UserStatusRow>> {
    sqlx::query_as!(
        UserStatusRow,
        r#"SELECT id, username, status, role AS "role: Role" FROM users WHERE id = $1 FOR UPDATE"#,
        id
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn set_status(
    conn: &mut PgConnection,
    id: i64,
    status: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE users SET status = $2, updated_at = $3 WHERE id = $1",
        id,
        status,
        now
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 用户名包含 `needle` 的账号（不区分大小写，最多 `limit` 个）。审核后台按举报人搜索用。
pub(crate) async fn ids_by_username_fragment(
    conn: &mut PgConnection,
    needle: &str,
    limit: i64,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT id FROM users WHERE strpos(lower(username), lower($1)) > 0 ORDER BY id LIMIT $2",
        needle,
        limit
    )
    .fetch_all(conn)
    .await
}

/// 把活跃账号标为 AI 账号（AI-012），返回其 id。
pub(crate) async fn mark_ai_agent(
    conn: &mut PgConnection,
    username: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar!(
        "UPDATE users SET is_ai_agent = TRUE, updated_at = $2
         WHERE username = $1 AND status = 'active'
         RETURNING id",
        username,
        now,
    )
    .fetch_optional(conn)
    .await
}

// ---------- 管理后台审计 ----------

pub(crate) struct NewAudit<'a> {
    pub id: i64,
    pub actor_id: i64,
    pub action: &'a str,
    pub target_type: &'a str,
    pub target_id: Option<&'a str>,
    pub details: &'a serde_json::Value,
    pub ip: String,
    pub user_agent: Option<&'a str>,
    pub now: DateTime<Utc>,
}

pub(crate) async fn insert_audit(conn: &mut PgConnection, a: &NewAudit<'_>) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO admin_audit_log (id, actor_id, action, target_type, target_id, details, ip,
                                      user_agent, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7::text::inet, $8, $9)",
        a.id,
        a.actor_id,
        a.action,
        a.target_type,
        a.target_id,
        a.details,
        a.ip,
        a.user_agent,
        a.now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct AuditFilter {
    pub actor_id: Option<i64>,
    pub action_prefix: Option<String>,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
}

pub(crate) struct AuditRow {
    pub id: i64,
    pub actor_id: Option<i64>,
    pub actor_name: Option<String>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub details: serde_json::Value,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn count_audit(conn: &mut PgConnection, f: &AuditFilter) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM admin_audit_log
           WHERE ($1::bigint IS NULL OR actor_id = $1)
             AND ($2::text IS NULL OR starts_with(action, $2))
             AND ($3::text IS NULL OR target_type = $3)
             AND ($4::text IS NULL OR target_id = $4)"#,
        f.actor_id,
        f.action_prefix,
        f.target_type,
        f.target_id,
    )
    .fetch_one(conn)
    .await
}

pub(crate) async fn list_audit(
    conn: &mut PgConnection,
    f: &AuditFilter,
    limit: i64,
    offset: i64,
) -> sqlx::Result<Vec<AuditRow>> {
    sqlx::query_as!(
        AuditRow,
        r#"SELECT a.id, a.actor_id, u.username AS "actor_name?", a.action, a.target_type, a.target_id,
                  a.details, host(a.ip) AS ip, a.user_agent, a.created_at
           FROM admin_audit_log a LEFT JOIN users u ON u.id = a.actor_id
           WHERE ($1::bigint IS NULL OR a.actor_id = $1)
             AND ($2::text IS NULL OR starts_with(a.action, $2))
             AND ($3::text IS NULL OR a.target_type = $3)
             AND ($4::text IS NULL OR a.target_id = $4)
           ORDER BY a.created_at DESC, a.id DESC
           LIMIT $5 OFFSET $6"#,
        f.actor_id,
        f.action_prefix,
        f.target_type,
        f.target_id,
        limit,
        offset,
    )
    .fetch_all(conn)
    .await
}

// ---------- 管理后台：账号 ----------

pub(crate) struct AdminUserRow {
    pub id: i64,
    pub username: String,
    pub email: Option<String>,
    pub status: String,
    pub role: Role,
    pub is_ai_agent: bool,
    pub exam_passed_at: Option<DateTime<Utc>>,
    pub level: i16,
    pub created_at: DateTime<Utc>,
}

pub(crate) struct UserFilter {
    /// 用户名或邮箱包含（ILIKE 模式，已转义）。
    pub pattern: Option<String>,
    /// 精确 id（关键词是数字时）。
    pub id: Option<i64>,
    pub role: Option<String>,
    pub status: Option<String>,
    pub ai: Option<bool>,
}

pub(crate) async fn count_admin_users(
    conn: &mut PgConnection,
    f: &UserFilter,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM users
           WHERE ($1::text IS NULL OR username ILIKE $1 OR email ILIKE $1 OR id = $2)
             AND ($3::text IS NULL OR role = $3)
             AND ($4::text IS NULL OR status = $4)
             AND ($5::bool IS NULL OR is_ai_agent = $5)"#,
        f.pattern,
        f.id,
        f.role,
        f.status,
        f.ai,
    )
    .fetch_one(conn)
    .await
}

pub(crate) async fn list_admin_users(
    conn: &mut PgConnection,
    f: &UserFilter,
    limit: i64,
    offset: i64,
) -> sqlx::Result<Vec<AdminUserRow>> {
    sqlx::query_as!(
        AdminUserRow,
        r#"SELECT id, username, email, status, role AS "role: Role", is_ai_agent, exam_passed_at,
                  level, created_at
           FROM users
           WHERE ($1::text IS NULL OR username ILIKE $1 OR email ILIKE $1 OR id = $2)
             AND ($3::text IS NULL OR role = $3)
             AND ($4::text IS NULL OR status = $4)
             AND ($5::bool IS NULL OR is_ai_agent = $5)
           ORDER BY created_at DESC, id DESC
           LIMIT $6 OFFSET $7"#,
        f.pattern,
        f.id,
        f.role,
        f.status,
        f.ai,
        limit,
        offset,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn admin_user(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<AdminUserRow>> {
    sqlx::query_as!(
        AdminUserRow,
        r#"SELECT id, username, email, status, role AS "role: Role", is_ai_agent, exam_passed_at,
                  level, created_at
           FROM users WHERE id = $1"#,
        id,
    )
    .fetch_optional(conn)
    .await
}

/// 改角色（只改未注销、未合并的账号）。
pub(crate) async fn set_role(
    conn: &mut PgConnection,
    id: i64,
    role: Role,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE users SET role = $2, updated_at = $3
         WHERE id = $1 AND status IN ('active', 'blocked')",
        id,
        role as Role,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn set_ai_agent(
    conn: &mut PgConnection,
    id: i64,
    on: bool,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE users SET is_ai_agent = $2, updated_at = $3
         WHERE id = $1 AND status IN ('active', 'blocked')",
        id,
        on,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 删除系统头像前卸下（外键不级联）。
pub(crate) async fn unequip_system_avatar(
    conn: &mut PgConnection,
    avatar_id: i64,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "UPDATE users SET equipped_avatar_id = NULL, equipped_avatar_media_id = NULL
         WHERE equipped_avatar_id = $1",
        avatar_id,
    )
    .execute(conn)
    .await?
    .rows_affected())
}

/// 删除名片前卸下。
pub(crate) async fn unequip_card(conn: &mut PgConnection, card_id: i64) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "UPDATE users SET equipped_card_id = NULL WHERE equipped_card_id = $1",
        card_id,
    )
    .execute(conn)
    .await?
    .rows_affected())
}

pub(crate) struct UserCountsRow {
    pub total: i64,
    pub blocked: i64,
    pub new_since: i64,
}

pub(crate) async fn user_counts(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
) -> sqlx::Result<UserCountsRow> {
    sqlx::query_as!(
        UserCountsRow,
        r#"SELECT count(*) FILTER (WHERE status IN ('active', 'blocked')) AS "total!",
                  count(*) FILTER (WHERE status = 'blocked') AS "blocked!",
                  count(*) FILTER (WHERE created_at >= $1 AND status IN ('active', 'blocked')) AS "new_since!"
           FROM users"#,
        since,
    )
    .fetch_one(conn)
    .await
}
