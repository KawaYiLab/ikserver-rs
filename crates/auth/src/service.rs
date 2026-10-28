//! 认证业务逻辑。所有入口都返回 `Problem`，handler 只负责协议转换。

use std::collections::HashMap;
use std::net::IpAddr;

use chrono::{DateTime, Duration, Utc};
use http::StatusCode;
use ik_kernel::Id;
use ik_kernel::bus::{Audience, Event};
use ik_kernel::db::is_unique_violation;
use ik_kernel::id::marker::User;
use ik_kernel::mail::{OutgoingMail, mask_email};
use ik_kernel::{Problem, Role};
use sqlx::{PgConnection, PgPool};

use crate::code::{
    self, CODE_TTL_SECS, CodePurpose, MAX_ATTEMPTS, MAX_CODES_PER_IP_PER_HOUR, SEND_COOLDOWN_SECS,
};
use crate::dto::{
    BindEmailCodeRequest, BindEmailRequest, ChangePasswordRequest, CodeLoginRequest, MeView,
    PasswordLoginRequest, RegisterRequest, ResetPasswordRequest, SendCodeRequest, SendCodeResponse,
    SessionView,
};
use crate::extract::Principal;
use crate::repo::{self, MeRow, NewSession, SessionRow};
use crate::{AuthState, password, token, validate};

/// 每个用户最多保留的活跃会话数（SESS-004）。
pub const MAX_ACTIVE_SESSIONS: i64 = 100;
/// 自动生成用户名（REG-004）。
const USERNAME_PREFIX: &str = "绳网用户";
const USERNAME_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const USERNAME_SHORT_TRIES: usize = 20;

/// 距上次续期超过这么久才写库，避免每个请求都产生一次写入。
fn touch_interval() -> Duration {
    Duration::hours(1)
}

#[derive(Debug, Clone)]
pub struct RequestMeta {
    pub ip: IpAddr,
    pub user_agent: Option<String>,
}

/// 登录/注册成功的结果。`token` 是令牌原文，只在这里出现一次。
pub struct Issued {
    pub user: MeView,
    pub session: SessionView,
    pub token: String,
}

impl std::fmt::Debug for Issued {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Issued")
            .field("user", &self.user)
            .field("session", &self.session)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug)]
pub enum Resolution {
    Active(Principal),
    /// 令牌不存在、已过期或已撤销。
    Invalid,
    /// 令牌有效但账号已封禁/注销/合并。
    Blocked,
}

// ---------- 错误 ----------

pub fn invalid_credentials() -> Problem {
    Problem::new(
        StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
        "账号或密码错误",
    )
}

pub fn account_blocked() -> Problem {
    Problem::new(
        StatusCode::FORBIDDEN,
        "ACCOUNT_BLOCKED",
        "该账号已被封禁或已注销",
    )
}

fn email_taken() -> Problem {
    Problem::conflict("EMAIL_TAKEN", "该邮箱已注册")
}

/// 注册邮箱的域名在后台的黑名单里（一次性邮箱等）。
async fn check_email_domain(state: &AuthState, email: &str) -> Result<(), Problem> {
    match state.site_values().await {
        Some(values) if values.email_domain_blocked(email) => Err(Problem::field(
            "email",
            "EMAIL_DOMAIN_BLOCKED",
            "这个邮箱域名不能用来注册，请换一个邮箱",
        )),
        _ => Ok(()),
    }
}

pub fn registration_closed() -> Problem {
    Problem::new(
        StatusCode::FORBIDDEN,
        "REGISTRATION_CLOSED",
        "当前不允许注册",
    )
}

fn code_cooldown(retry_after_secs: i64) -> Problem {
    Problem::new(
        StatusCode::TOO_MANY_REQUESTS,
        "CODE_COOLDOWN",
        format!("验证码发送过于频繁，请 {retry_after_secs} 秒后再试"),
    )
    .with_retry_after(u64::try_from(retry_after_secs).unwrap_or(1))
}

fn mail_failed() -> Problem {
    Problem::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "MAIL_SEND_FAILED",
        "验证码邮件发送失败，请稍后再试",
    )
}

// ---------- 验证码 ----------

/// 发送验证码。注册用途对占用/冷却显式报错（CODE-014）；登录与重置用途对未注册、
/// 不可用账号及冷却命中一律静默返回相同结果，防止账号枚举（CODE-012/013）。
pub async fn send_code(
    state: &AuthState,
    request: SendCodeRequest,
    meta: &RequestMeta,
) -> Result<SendCodeResponse, Problem> {
    let email = validate::normalize_email(&request.email)?;
    let purpose = request.purpose;
    let now = Utc::now();
    let ip = meta.ip.to_string();
    let accepted = SendCodeResponse {
        sent: true,
        expires_in_secs: CODE_TTL_SECS,
        cooldown_secs: SEND_COOLDOWN_SECS,
    };

    let mut conn = state.db.acquire().await?;
    if repo::count_codes_from_ip_since(&mut conn, &ip, now - Duration::hours(1)).await?
        >= MAX_CODES_PER_IP_PER_HOUR
    {
        return Err(Problem::rate_limited());
    }

    let eligible = match purpose {
        CodePurpose::Register => {
            if !state.registration_open().await {
                return Err(registration_closed());
            }
            check_email_domain(state, &email).await?;
            if repo::email_taken(&mut conn, &email).await? {
                return Err(email_taken());
            }
            true
        }
        CodePurpose::Login | CodePurpose::ResetPassword => {
            repo::active_user_id_by_email(&mut conn, &email)
                .await?
                .is_some()
        }
        // 反序列化时已经拒绝；这两种用途有各自需要登录的接口。
        CodePurpose::BindEmail | CodePurpose::DeleteAccount => {
            return Err(Problem::field("purpose", "INVALID", "不支持的用途"));
        }
    };

    if !eligible {
        return Ok(accepted);
    }
    drop(conn);
    match deliver_code(state, &email, purpose, &ip, now).await? {
        Delivery::Sent => Ok(accepted),
        Delivery::Cooldown { retry_after_secs } => match purpose {
            CodePurpose::Login | CodePurpose::ResetPassword => Ok(accepted),
            _ => Err(code_cooldown(retry_after_secs)),
        },
    }
}

/// 发码结果：冷却期内的重复请求不发信，由调用方决定报错还是静默返回。
enum Delivery {
    Sent,
    Cooldown { retry_after_secs: i64 },
}

/// 生成、保存并发出一条验证码（作废同邮箱同用途的旧码，CODE-005）。
/// 冷却判断与写入新码在同一事务、同一把 (email, purpose) 锁下，并发请求冷却期内只发一封。
async fn deliver_code(
    state: &AuthState,
    email: &str,
    purpose: CodePurpose,
    ip: &str,
    now: DateTime<Utc>,
) -> Result<Delivery, Problem> {
    let code = code::generate_code()?;
    let hash = code::code_hash(&state.settings.code_key, purpose, email, &code)?;
    let id = state.ids.next_raw();
    let mut tx = state.db.begin().await?;
    repo::lock_code_slot(&mut tx, email, purpose.as_str()).await?;
    if let Some(latest) = repo::latest_open_code(&mut tx, email, purpose.as_str()).await? {
        // 等锁期间别人可能刚写入新码（sent_at 晚于 now），按 0 秒已过计。
        let elapsed = (now - latest.sent_at).num_seconds().max(0);
        if elapsed < SEND_COOLDOWN_SECS {
            return Ok(Delivery::Cooldown {
                retry_after_secs: (SEND_COOLDOWN_SECS - elapsed).max(1),
            });
        }
    }
    repo::invalidate_open_codes(&mut tx, email, purpose.as_str(), now).await?;
    repo::insert_code(
        &mut tx,
        id,
        email,
        purpose.as_str(),
        &hash,
        now,
        now + Duration::seconds(CODE_TTL_SECS),
        ip,
    )
    .await?;
    tx.commit().await?;

    // 事务提交后再发信（AGENTS.md 规则 6）。发信失败则作废这条码，不让它占用冷却窗口（CODE-006）。
    let content = code::mail_content(purpose, &code);
    let mail = OutgoingMail {
        to: email.to_owned(),
        subject: content.subject,
        text: content.text,
        html: Some(content.html),
    };
    if let Err(error) = state.mailer.send(mail).await {
        tracing::error!(%error, to = %mask_email(email), purpose = purpose.as_str(), "验证码邮件发送失败");
        let mut conn = state.db.acquire().await?;
        repo::force_consume_code(&mut conn, id, Utc::now()).await?;
        return Err(mail_failed());
    }
    Ok(Delivery::Sent)
}

/// 需要登录的发码（绑定邮箱、注销）：每 IP 每小时上限与冷却都显式报错。
async fn send_own_code(
    state: &AuthState,
    email: &str,
    purpose: CodePurpose,
    meta: &RequestMeta,
) -> Result<SendCodeResponse, Problem> {
    let now = Utc::now();
    let ip = meta.ip.to_string();
    let mut conn = state.db.acquire().await?;
    if repo::count_codes_from_ip_since(&mut conn, &ip, now - Duration::hours(1)).await?
        >= MAX_CODES_PER_IP_PER_HOUR
    {
        return Err(Problem::rate_limited());
    }
    drop(conn);
    if let Delivery::Cooldown { retry_after_secs } =
        deliver_code(state, email, purpose, &ip, now).await?
    {
        return Err(code_cooldown(retry_after_secs));
    }
    Ok(SendCodeResponse {
        sent: true,
        expires_in_secs: CODE_TTL_SECS,
        cooldown_secs: SEND_COOLDOWN_SECS,
    })
}

/// 第一阶段：核对验证码（不消费）。错误尝试计数在独立的短事务里提交，
/// 这样即使随后返回错误，计数也不会随请求失败而回滚（CODE-008/010）。
/// 返回验证码记录 id，调用方在业务事务里用 [`repo::consume_code`] 原子消费（CODE-009）。
async fn check_code(
    state: &AuthState,
    email: &str,
    purpose: CodePurpose,
    code: &str,
) -> Result<i64, Problem> {
    validate::validate_code_format(code)?;
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let Some(row) = repo::lock_latest_open_code(&mut tx, email, purpose.as_str()).await? else {
        return Err(code::code_invalid());
    };
    if row.expires_at <= now {
        repo::force_consume_code(&mut tx, row.id, now).await?;
        tx.commit().await?;
        return Err(code::code_invalid());
    }
    if row.attempts >= MAX_ATTEMPTS {
        repo::force_consume_code(&mut tx, row.id, now).await?;
        tx.commit().await?;
        return Err(code::code_too_many_attempts());
    }
    let expected = code::code_hash(&state.settings.code_key, purpose, email, code)?;
    if !code::hashes_match(&expected, &row.code_hash) {
        let attempts = repo::bump_code_attempts(&mut tx, row.id, now).await?;
        tx.commit().await?;
        return Err(match attempts {
            Some(n) if n < MAX_ATTEMPTS => code::code_invalid(),
            _ => code::code_too_many_attempts(),
        });
    }
    tx.commit().await?;
    Ok(row.id)
}

async fn consume_code(
    conn: &mut PgConnection,
    code_id: i64,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    if repo::consume_code(conn, code_id, now).await? {
        Ok(())
    } else {
        // 被并发请求抢先消费或恰好过期。
        Err(code::code_invalid())
    }
}

// ---------- 注册与登录 ----------

/// 邮箱验证码注册（REG-003）。成功后直接登录。
pub async fn register(
    state: &AuthState,
    request: RegisterRequest,
    meta: &RequestMeta,
) -> Result<Issued, Problem> {
    let email = validate::normalize_email(&request.email)?;
    validate::validate_code_format(&request.code)?;
    validate::validate_password("password", &request.password)?;
    if !state.registration_open().await {
        return Err(registration_closed());
    }
    check_email_domain(state, &email).await?;
    {
        let mut conn = state.db.acquire().await?;
        if repo::email_taken(&mut conn, &email).await? {
            return Err(email_taken());
        }
    }

    let code_id = check_code(state, &email, CodePurpose::Register, &request.code).await?;
    let password_hash = password::hash(request.password).await?;

    let now = Utc::now();
    let user_id = state.ids.next_raw();
    let mut tx = state.db.begin().await?;
    consume_code(&mut tx, code_id, now).await?;
    insert_user_with_generated_name(&mut tx, user_id, &email, now).await?;
    repo::upsert_password(&mut tx, user_id, &password_hash, now).await?;
    let issued = issue_session(state, &mut tx, user_id, "register", meta, now).await?;
    tx.commit().await?;
    Ok(issued)
}

/// 用户名 = `绳网用户` + 4 位随机串，最多试 20 次，仍冲突则用 8 位（REG-004）。
async fn insert_user_with_generated_name(
    conn: &mut PgConnection,
    user_id: i64,
    email: &str,
    now: DateTime<Utc>,
) -> Result<String, Problem> {
    let lengths = std::iter::repeat_n(4, USERNAME_SHORT_TRIES).chain(std::iter::once(8));
    for len in lengths {
        let username = format!("{USERNAME_PREFIX}{}", random_suffix(len)?);
        match repo::insert_user(conn, user_id, &username, email, now).await {
            Ok(true) => return Ok(username),
            Ok(false) => continue,
            Err(error) if is_unique_violation(&error, "users_email_uq") => {
                return Err(email_taken());
            }
            Err(error) => return Err(error.into()),
        }
    }
    tracing::error!("自动生成用户名连续冲突");
    Err(Problem::internal())
}

fn random_suffix(len: usize) -> Result<String, Problem> {
    let mut out = String::with_capacity(len);
    let mut buf = [0u8; 16];
    while out.len() < len {
        getrandom::fill(&mut buf).map_err(|e| Problem::internal_from(&e))?;
        // 248 = 62 × 4，丢弃 248..=255 以免取模偏差。
        for &byte in buf.iter().filter(|&&b| b < 248) {
            if out.len() == len {
                break;
            }
            out.push(char::from(
                USERNAME_ALPHABET[usize::from(byte) % USERNAME_ALPHABET.len()],
            ));
        }
    }
    Ok(out)
}

/// 密码登录（LOGIN-001）。账号不存在、没有密码、密码错误一律同样的错误与相近的耗时；
/// 只有密码正确后才透露账号被封禁。
pub async fn login_password(
    state: &AuthState,
    request: PasswordLoginRequest,
    meta: &RequestMeta,
) -> Result<Issued, Problem> {
    let identifier = request.identifier.trim();
    if identifier.is_empty() || request.password.is_empty() {
        return Err(invalid_credentials());
    }

    let candidate = {
        let mut conn = state.db.acquire().await?;
        if identifier.contains('@') {
            match validate::normalize_email(identifier) {
                Ok(email) => repo::login_candidate_by_email(&mut conn, &email).await?,
                Err(_) => None,
            }
        } else {
            repo::login_candidate_by_username(&mut conn, identifier).await?
        }
    };
    let Some((candidate, stored)) = candidate.and_then(|c| c.hash.clone().map(|h| (c, h))) else {
        password::verify_dummy(request.password).await;
        return Err(invalid_credentials());
    };

    let verification = password::verify(request.password.clone(), stored).await?;
    if !verification.ok {
        return Err(invalid_credentials());
    }
    if candidate.status != "active" {
        return Err(account_blocked());
    }
    let upgraded = if verification.needs_rehash {
        Some(password::hash(request.password).await?)
    } else {
        None
    };

    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    if let Some(hash) = upgraded {
        repo::upsert_password(&mut tx, candidate.id, &hash, now).await?;
    }
    let issued = issue_session(state, &mut tx, candidate.id, "password", meta, now).await?;
    tx.commit().await?;
    Ok(issued)
}

/// 邮箱验证码登录（LOGIN-006/007）。账号不存在或不可用与验证码错误返回同一个错误。
pub async fn login_email_code(
    state: &AuthState,
    request: CodeLoginRequest,
    meta: &RequestMeta,
) -> Result<Issued, Problem> {
    let email = validate::normalize_email(&request.email)?;
    let code_id = check_code(state, &email, CodePurpose::Login, &request.code).await?;

    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let Some(user_id) = repo::active_user_id_by_email(&mut tx, &email).await? else {
        return Err(code::code_invalid());
    };
    consume_code(&mut tx, code_id, now).await?;
    let issued = issue_session(state, &mut tx, user_id, "email_code", meta, now).await?;
    tx.commit().await?;
    Ok(issued)
}

/// 用验证码重置密码（LOGIN-008/009）。成功后撤销该账号全部会话，不自动登录。
pub async fn reset_password(
    state: &AuthState,
    request: ResetPasswordRequest,
) -> Result<(), Problem> {
    let email = validate::normalize_email(&request.email)?;
    validate::validate_code_format(&request.code)?;
    validate::validate_password("newPassword", &request.new_password)?;
    let code_id = check_code(state, &email, CodePurpose::ResetPassword, &request.code).await?;
    let password_hash = password::hash(request.new_password).await?;

    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let Some(user_id) = repo::active_user_id_by_email(&mut tx, &email).await? else {
        return Err(code::code_invalid());
    };
    if repo::lock_active_user(&mut tx, user_id).await?.is_none() {
        return Err(code::code_invalid());
    }
    consume_code(&mut tx, code_id, now).await?;
    repo::upsert_password(&mut tx, user_id, &password_hash, now).await?;
    repo::revoke_user_sessions(&mut tx, user_id, None, now, "password_reset").await?;
    tx.commit().await?;
    announce_sessions_changed(state, user_id);
    Ok(())
}

/// 在调用方的事务里签发会话：锁用户行 → 淘汰超额会话 → 插入新会话。
async fn issue_session(
    state: &AuthState,
    conn: &mut PgConnection,
    user_id: i64,
    login_method: &'static str,
    meta: &RequestMeta,
    now: DateTime<Utc>,
) -> Result<Issued, Problem> {
    if repo::lock_active_user(conn, user_id).await?.is_none() {
        return Err(account_blocked());
    }
    insert_session(
        state,
        conn,
        user_id,
        login_method,
        meta,
        now,
        token::generate()?,
    )
    .await
}

/// 确定性会话（米游社扫码登录，SESS-019）：令牌由调用方给的 32 字节秘密决定，同一秘密永远兑换出
/// 同一个会话——并发轮询、断线重连拿到的是同一个。这个会话已经被登出、吊销或过期时返回
/// 409 `QR_SESSION_CONSUMED`，不会另起新会话。在调用方的事务里执行；账号不可用返回 403 `ACCOUNT_BLOCKED`。
pub async fn issue_session_with_secret(
    state: &AuthState,
    conn: &mut PgConnection,
    user_id: i64,
    login_method: &'static str,
    secret: &[u8; 32],
    meta: &RequestMeta,
    now: DateTime<Utc>,
) -> Result<Issued, Problem> {
    if repo::lock_active_user(conn, user_id).await?.is_none() {
        return Err(account_blocked());
    }
    let new_token = token::from_secret(secret);
    let Some(existing) = repo::session_by_hash(conn, &new_token.hash).await? else {
        return insert_session(state, conn, user_id, login_method, meta, now, new_token).await;
    };
    if existing.user_id != user_id {
        tracing::error!("确定性会话的令牌属于另一个账号");
        return Err(Problem::internal());
    }
    if existing.revoked_at.is_some() || existing.session.expires_at <= now {
        return Err(Problem::conflict(
            "QR_SESSION_CONSUMED",
            "这次扫码登录已经失效，请重新扫码",
        ));
    }
    let me = repo::me(conn, user_id)
        .await?
        .ok_or_else(Problem::internal)?;
    let session_id = existing.session.id;
    Ok(Issued {
        user: me_view(me),
        session: session_view(existing.session, session_id),
        token: new_token.token,
    })
}

async fn insert_session(
    state: &AuthState,
    conn: &mut PgConnection,
    user_id: i64,
    login_method: &'static str,
    meta: &RequestMeta,
    now: DateTime<Utc>,
    new_token: token::NewToken,
) -> Result<Issued, Problem> {
    repo::revoke_overflow_sessions(conn, user_id, MAX_ACTIVE_SESSIONS - 1, now).await?;
    let session_id = state.ids.next_raw();
    let max_expires_at = now + state.settings.session_max_lifetime;
    let expires_at = (now + state.settings.session_ttl).min(max_expires_at);
    let ip = meta.ip.to_string();
    repo::insert_session(
        conn,
        NewSession {
            id: session_id,
            user_id,
            token_hash: &new_token.hash,
            login_method,
            user_agent: meta.user_agent.as_deref(),
            ip: &ip,
            now,
            expires_at,
            max_expires_at,
        },
    )
    .await?;

    let me = repo::me(conn, user_id)
        .await?
        .ok_or_else(Problem::internal)?;
    let session = SessionRow {
        id: session_id,
        login_method: login_method.to_owned(),
        user_agent: meta.user_agent.clone(),
        ip: Some(ip),
        location: None,
        created_at: now,
        last_seen_at: now,
        expires_at,
    };
    Ok(Issued {
        user: me_view(me),
        session: session_view(session, session_id),
        token: new_token.token,
    })
}

// ---------- 账号安全 ----------

fn email_unchanged() -> Problem {
    Problem::field("email", "UNCHANGED", "该邮箱已绑定当前账号")
}

pub fn account_changed() -> Problem {
    Problem::conflict("ACCOUNT_CHANGED", "账号信息已变更，请重新验证")
}

async fn current_email(state: &AuthState, user_id: i64) -> Result<Option<String>, Problem> {
    let mut conn = state.db.acquire().await?;
    Ok(repo::me(&mut conn, user_id)
        .await?
        .ok_or_else(account_blocked)?
        .email)
}

/// 给新邮箱发绑定验证码（DEL-002）：与当前邮箱相同 → 400；已被其他账号使用 → 409 `EMAIL_TAKEN`；
/// 冷却 → 429 `CODE_COOLDOWN`。米游社首登的账号由此绑定第一个邮箱。
pub async fn send_bind_email_code(
    state: &AuthState,
    principal: &Principal,
    request: BindEmailCodeRequest,
    meta: &RequestMeta,
) -> Result<SendCodeResponse, Problem> {
    let email = validate::normalize_email(&request.email)?;
    if current_email(state, principal.user_id.get())
        .await?
        .as_deref()
        == Some(email.as_str())
    {
        return Err(email_unchanged());
    }
    {
        let mut conn = state.db.acquire().await?;
        if repo::email_taken(&mut conn, &email).await? {
            return Err(email_taken());
        }
    }
    send_own_code(state, &email, CodePurpose::BindEmail, meta).await
}

/// 绑定 / 更换邮箱（DEL-002）。验证码在锁内原子消费；之后可以用这个邮箱登录、找回密码。
pub async fn bind_email(
    state: &AuthState,
    principal: &Principal,
    request: BindEmailRequest,
) -> Result<MeView, Problem> {
    let email = validate::normalize_email(&request.email)?;
    let user_id = principal.user_id.get();
    let code_id = check_code(state, &email, CodePurpose::BindEmail, &request.code).await?;
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    if repo::lock_active_user(&mut tx, user_id).await?.is_none() {
        return Err(account_blocked());
    }
    let me = repo::me(&mut tx, user_id)
        .await?
        .ok_or_else(account_blocked)?;
    if me.email.as_deref() == Some(email.as_str()) {
        return Err(email_unchanged());
    }
    consume_code(&mut tx, code_id, now).await?;
    match repo::set_email(&mut tx, user_id, &email, now).await {
        Ok(()) => {}
        Err(error) if is_unique_violation(&error, "users_email_uq") => return Err(email_taken()),
        Err(error) => return Err(error.into()),
    }
    let me = repo::me(&mut tx, user_id)
        .await?
        .ok_or_else(Problem::internal)?;
    tx.commit().await?;
    Ok(me_view(me))
}

/// 修改密码（LOGIN-011）：校验当前密码，所有会话下线，再给当前设备签一个新会话。
/// 没有密码的账号（米游社首登）用「重置密码」设置第一个密码。
pub async fn change_password(
    state: &AuthState,
    principal: &Principal,
    request: ChangePasswordRequest,
    meta: &RequestMeta,
) -> Result<Issued, Problem> {
    let user_id = principal.user_id.get();
    validate::validate_password("newPassword", &request.new_password)?;
    let stored = {
        let mut conn = state.db.acquire().await?;
        repo::password_hash(&mut conn, user_id).await?
    };
    let Some(stored) = stored else {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "PASSWORD_NOT_SET",
            "当前账号还没有设置密码，请通过重置密码设置",
        ));
    };
    if !password::verify(request.current_password.clone(), stored.clone())
        .await?
        .ok
    {
        return Err(Problem::field(
            "currentPassword",
            "INVALID",
            "当前密码不正确",
        ));
    }
    if request.current_password == request.new_password {
        return Err(Problem::field(
            "newPassword",
            "UNCHANGED",
            "新密码不能与当前密码相同",
        ));
    }
    let new_hash = password::hash(request.new_password).await?;

    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    if repo::lock_active_user(&mut tx, user_id).await?.is_none() {
        return Err(account_blocked());
    }
    // 校验之后密码被别处改过（并发的重置或修改）。
    if repo::password_hash(&mut tx, user_id).await?.as_deref() != Some(stored.as_str()) {
        return Err(account_changed());
    }
    repo::upsert_password(&mut tx, user_id, &new_hash, now).await?;
    repo::revoke_user_sessions(&mut tx, user_id, None, now, "password_change").await?;
    let issued = insert_session(
        state,
        &mut tx,
        user_id,
        "password_change",
        meta,
        now,
        token::generate()?,
    )
    .await?;
    tx.commit().await?;
    announce_sessions_changed(state, user_id);
    Ok(issued)
}

/// 账号安全概览要的凭证信息。
#[derive(Debug, Clone)]
pub struct Credentials {
    pub email: Option<String>,
    pub has_password: bool,
}

pub async fn credentials(state: &AuthState, user_id: i64) -> Result<Credentials, Problem> {
    let mut conn = state.db.acquire().await?;
    let email = repo::me(&mut conn, user_id)
        .await?
        .ok_or_else(account_blocked)?
        .email;
    let has_password = repo::password_hash(&mut conn, user_id).await?.is_some();
    Ok(Credentials {
        email,
        has_password,
    })
}

fn no_email_for_deletion() -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "DELETE_ACCOUNT_NO_EMAIL",
        "当前账号未绑定邮箱，请通过米游社扫码注销",
    )
}

/// 发注销验证码到自己绑定的邮箱（DEL-003）。
pub async fn send_delete_code(
    state: &AuthState,
    principal: &Principal,
    meta: &RequestMeta,
) -> Result<SendCodeResponse, Problem> {
    let email = current_email(state, principal.user_id.get())
        .await?
        .ok_or_else(no_email_for_deletion)?;
    send_own_code(state, &email, CodePurpose::DeleteAccount, meta).await
}

/// 核对过、还没消费的注销验证码。在注销事务里交给 [`crate::interop::consume_delete_code`]。
#[derive(Debug, Clone)]
pub struct DeleteCode {
    pub(crate) code_id: i64,
    pub(crate) email: String,
}

/// 第一阶段：核对注销验证码（锁外，不消费，DEL-004）。
pub async fn check_delete_code(
    state: &AuthState,
    principal: &Principal,
    code: &str,
) -> Result<DeleteCode, Problem> {
    let email = current_email(state, principal.user_id.get())
        .await?
        .ok_or_else(no_email_for_deletion)?;
    let code_id = check_code(state, &email, CodePurpose::DeleteAccount, code).await?;
    Ok(DeleteCode { code_id, email })
}

/// 会话被吊销的内部事件（实时网关据此断开连接）。在调用方的事务提交后发布。
pub fn sessions_changed_event(user_id: i64) -> Result<Event, Problem> {
    Event::new(SESSIONS_CHANGED, Audience::User(user_id), &())
}

// ---------- 会话 ----------

/// 校验请求携带的令牌。活跃会话按需滑动续期。
pub async fn resolve(state: &AuthState, raw_token: &str) -> Result<Resolution, Problem> {
    if !token::looks_valid(raw_token) {
        return Ok(Resolution::Invalid);
    }
    let now = Utc::now();
    let mut conn = state.db.acquire().await?;
    let Some(row) = repo::resolve_session(&mut conn, &token::hash(raw_token), now).await? else {
        return Ok(Resolution::Invalid);
    };
    if row.status != "active" {
        return Ok(Resolution::Blocked);
    }
    if now - row.last_seen_at > touch_interval() {
        repo::touch_session(
            &mut conn,
            row.session_id,
            now,
            now + state.settings.session_ttl,
        )
        .await?;
    }
    Ok(Resolution::Active(Principal {
        user_id: Id::new(row.user_id),
        session_id: Id::new(row.session_id),
        role: row.role,
        exam_passed: row.exam_passed_at.is_some() || !row.exam_required,
        is_ai_agent: row.is_ai_agent,
        level: row.level,
    }))
}

/// 内部事件：某个用户的会话被吊销（登出、踢设备、重置密码）。实时网关收到后复查该用户的连接，
/// 断开已失效的；不会推给客户端。
pub const SESSIONS_CHANGED: &str = "auth.sessions_changed";

/// 吊销已提交后调用。
fn announce_sessions_changed(state: &AuthState, user_id: i64) {
    match Event::new(SESSIONS_CHANGED, Audience::User(user_id), &()) {
        Ok(event) => state.bus.publish(event),
        Err(error) => tracing::error!(?error, "构造会话吊销事件失败"),
    }
}

/// 会话仍然有效（未吊销、未过期、账号正常）。实时网关定期复查长连接时使用。
pub async fn session_active(
    state: &AuthState,
    user_id: i64,
    session_id: i64,
) -> Result<bool, Problem> {
    let mut conn = state.db.acquire().await?;
    Ok(repo::session_active(&mut conn, user_id, session_id, Utc::now()).await?)
}

pub async fn logout(state: &AuthState, principal: &Principal) -> Result<(), Problem> {
    let mut conn = state.db.acquire().await?;
    repo::revoke_session(
        &mut conn,
        principal.user_id.get(),
        principal.session_id.get(),
        Utc::now(),
        "logout",
    )
    .await?;
    announce_sessions_changed(state, principal.user_id.get());
    Ok(())
}

pub async fn me(state: &AuthState, principal: &Principal) -> Result<MeView, Problem> {
    let mut conn = state.db.acquire().await?;
    let row = repo::me(&mut conn, principal.user_id.get())
        .await?
        .ok_or_else(Problem::unauthorized)?;
    Ok(me_view(row))
}

pub async fn list_sessions(
    state: &AuthState,
    principal: &Principal,
) -> Result<Vec<SessionView>, Problem> {
    let mut conn = state.db.acquire().await?;
    let rows = repo::active_sessions(&mut conn, principal.user_id.get(), Utc::now()).await?;
    Ok(rows
        .into_iter()
        .map(|row| session_view(row, principal.session_id.get()))
        .collect())
}

/// 撤销自己的某个会话。返回被撤销的是否就是当前会话（调用方据此清 Cookie）。
pub async fn revoke_session(
    state: &AuthState,
    principal: &Principal,
    session_id: i64,
) -> Result<bool, Problem> {
    let mut conn = state.db.acquire().await?;
    let revoked = repo::revoke_session(
        &mut conn,
        principal.user_id.get(),
        session_id,
        Utc::now(),
        "user_revoked",
    )
    .await?;
    if !revoked {
        return Err(Problem::not_found());
    }
    announce_sessions_changed(state, principal.user_id.get());
    Ok(session_id == principal.session_id.get())
}

pub async fn revoke_other_sessions(
    state: &AuthState,
    principal: &Principal,
) -> Result<u64, Problem> {
    let mut conn = state.db.acquire().await?;
    let revoked = repo::revoke_user_sessions(
        &mut conn,
        principal.user_id.get(),
        Some(principal.session_id.get()),
        Utc::now(),
        "user_revoked_others",
    )
    .await?;
    if revoked > 0 {
        announce_sessions_changed(state, principal.user_id.get());
    }
    Ok(revoked)
}

/// 仅供测试与运维脚本：按 id 读取会话。
pub async fn session(state: &AuthState, session_id: i64) -> Result<Option<SessionView>, Problem> {
    let mut conn = state.db.acquire().await?;
    Ok(repo::session_by_id(&mut conn, session_id)
        .await?
        .map(|row| session_view(row, session_id)))
}

fn me_view(row: MeRow) -> MeView {
    MeView {
        id: Id::new(row.id),
        member_no: row.member_no,
        username: row.username,
        email: row.email,
        role: row.role,
        exam_passed: row.exam_passed_at.is_some(),
        exam_required: row.exam_required,
        is_ai_agent: row.is_ai_agent,
        level: row.level,
        created_at: row.created_at,
    }
}

/// 其他域展示作者用的公开资料。
#[derive(Debug, Clone)]
pub struct Author {
    pub id: Id<User>,
    pub username: String,
    pub level: i16,
    /// 已注销或已合并的账号：调用方应显示为「已注销用户」，不链接主页。
    pub gone: bool,
    /// 展示用的头像（媒体 id）：装备的系统头像优先，其次自定义头像（COSMETIC-008）。
    /// 已注销的账号恒为 None。
    pub avatar_media_id: Option<i64>,
}

/// 批量读取作者资料；不存在的 id 不出现在结果里。
pub async fn authors(db: &PgPool, ids: &[i64]) -> Result<HashMap<i64, Author>, Problem> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut conn = db.acquire().await?;
    let rows = repo::authors(&mut conn, ids).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let gone = matches!(row.status.as_str(), "deleted" | "merged");
            let author = Author {
                id: Id::new(row.id),
                gone,
                username: row.username,
                level: row.level,
                avatar_media_id: row.avatar_media_id.filter(|_| !gone),
            };
            (row.id, author)
        })
        .collect())
}

fn session_view(row: SessionRow, current_session_id: i64) -> SessionView {
    SessionView {
        id: Id::new(row.id),
        login_method: row.login_method,
        user_agent: row.user_agent,
        ip: row.ip,
        location: row.location,
        created_at: row.created_at,
        last_seen_at: row.last_seen_at,
        expires_at: row.expires_at,
        current: row.id == current_session_id,
    }
}

/// 运维命令用：按邮箱设置角色（例如创建第一个管理员）。返回是否找到该用户。
/// 站里有没有管理员。没有时进入首次安装（`POST /v1/setup`、`ik-server setup`）。
pub async fn admin_exists(db: &PgPool) -> Result<bool, Problem> {
    let mut conn = db.acquire().await?;
    Ok(repo::admin_exists(&mut conn).await?)
}

/// 首次安装：还没有管理员时直接建一个管理员账号，不发验证码。
/// 用事务级咨询锁让同时提交的几次排队，只有第一次成功，其余返回 409 `ALREADY_SET_UP`。
/// `username` 由调用方校验（`ik_profile::service::validate_username`）。
pub async fn create_first_admin(
    state: &AuthState,
    email: &str,
    username: &str,
    password: String,
) -> Result<i64, Problem> {
    let email = validate::normalize_email(email)?;
    validate::validate_password("password", &password)?;
    let password_hash = password::hash(password).await?;
    let now = Utc::now();
    let user_id = state.ids.next_raw();
    let mut tx = state.db.begin().await?;
    sqlx::query!("SELECT pg_advisory_xact_lock($1)", SETUP_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    if repo::admin_exists(&mut tx).await? {
        return Err(already_set_up());
    }
    if repo::email_taken(&mut tx, &email).await? {
        return Err(email_taken());
    }
    if !repo::insert_user(&mut tx, user_id, username, &email, now).await? {
        return Err(Problem::conflict("USERNAME_TAKEN", "该用户名已被占用"));
    }
    repo::upsert_password(&mut tx, user_id, &password_hash, now).await?;
    repo::set_role_by_email(&mut tx, &email, Role::Admin, now).await?;
    tx.commit().await?;
    Ok(user_id)
}

/// 首次安装的咨询锁编号（任意固定值，只要不和别处的咨询锁重复）。
const SETUP_LOCK_KEY: i64 = 0x494b_5345_5455_5000;

pub fn already_set_up() -> Problem {
    Problem::conflict("ALREADY_SET_UP", "站点已经安装过了")
}

pub async fn set_role_by_email(db: &PgPool, email: &str, role: Role) -> Result<bool, Problem> {
    let email = validate::normalize_email(email)?;
    let mut conn = db.acquire().await?;
    Ok(repo::set_role_by_email(&mut conn, &email, role, Utc::now()).await?)
}

/// 用户名 → 用户 id（精确匹配、大小写敏感，只含正常状态的账号）。用于 @ 提及解析。
pub async fn active_user_ids_by_usernames(
    db: &PgPool,
    names: &[String],
) -> Result<HashMap<String, Id<User>>, Problem> {
    if names.is_empty() {
        return Ok(HashMap::new());
    }
    let mut conn = db.acquire().await?;
    Ok(repo::active_users_by_usernames(&mut conn, names)
        .await?
        .into_iter()
        .map(|row| (row.username, Id::new(row.id)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_suffix_uses_alphabet() {
        for len in [4, 8] {
            let suffix = random_suffix(len).unwrap();
            assert_eq!(suffix.len(), len);
            assert!(suffix.bytes().all(|b| USERNAME_ALPHABET.contains(&b)));
        }
    }
}
