use std::net::IpAddr;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use http::StatusCode;
use ik_auth::Principal;
use ik_auth::service::{Issued, RequestMeta};
use ik_kernel::Problem;
use ik_kernel::db::is_unique_violation;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;

use crate::MihoyoState;
use crate::device::new_device;
use crate::dto::{
    BindingView, CreateQrRequest, MyBinding, PollQrRequest, QrCreated, QrMode, QrStatus, ZzzRole,
};
use crate::repo::{self, BindingRow, NewQr, NewRole};
use crate::upstream::{self, Confirmed, Device, GameRole};

/// 上游拿不到真实有效期时给前端的提示（MHY-003）。
const QR_FALLBACK_EXPIRES_SECS: i64 = 180;
/// 扫码会话至少保留 180 秒，并比二维码多 30 秒（MHY-002）。
const QR_SESSION_MIN_SECS: i64 = 180;
const QR_SESSION_MARGIN_SECS: i64 = 30;
/// 2 秒内的重复轮询不查上游（MHY-006）。
const STATUS_CACHE_SECS: i64 = 2;
/// 处理「已确认」的占用时长（MHY-007）。
const CLAIM_SECS: i64 = 60;
/// 结果保留时长：成功 5 分钟，失败 1 分钟（MHY-008）。
const RESULT_OK_SECS: i64 = 300;
const RESULT_FAILED_SECS: i64 = 60;
/// 客户端的 qrId 是 32 字节的 base64url（43 个字符）；更长的直接按不存在处理。
const QR_ID_MAX_LEN: usize = 64;

const NICKNAME_MIN_CHARS: usize = 2;
const USERNAME_MAX_CHARS: usize = 20;
const NICKNAME_VARIANTS: usize = 5;
const GENERIC_TRIES: usize = 20;
const GENERIC_PREFIX: &str = "绳网用户";

// ---------- 错误 ----------

fn disabled() -> Problem {
    Problem::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "MIHOYO_DISABLED",
        "米游社登录暂不可用",
    )
}

fn upstream_failed(error: &upstream::UpstreamError) -> Problem {
    tracing::warn!(%error, "米游社上游请求失败");
    Problem::new(
        StatusCode::BAD_GATEWAY,
        "MIHOYO_UPSTREAM_ERROR",
        "米游社服务暂时不可用，请稍后再试",
    )
}

fn rate_limited(what: &str) -> Problem {
    Problem::new(
        StatusCode::TOO_MANY_REQUESTS,
        "RATE_LIMITED",
        format!("{what}过于频繁，请稍后再试"),
    )
    .with_retry_after(60)
}

fn bind_requires_auth() -> Problem {
    Problem::new(
        StatusCode::UNAUTHORIZED,
        "MIHOYO_BIND_REQUIRES_AUTH",
        "该操作需要先登录",
    )
}

fn user_mismatch() -> Problem {
    Problem::new(
        StatusCode::FORBIDDEN,
        "MIHOYO_BIND_USER_MISMATCH",
        "扫码会话与当前登录用户不一致",
    )
}

fn identity_already_bound() -> Problem {
    Problem::conflict(
        "IDENTITY_ALREADY_BOUND",
        "该米游社账号已绑定其他绳网账号，请先在原账号解绑或注销",
    )
}

fn user_already_bound() -> Problem {
    Problem::conflict(
        "USER_ALREADY_BOUND",
        "当前账号已绑定其他米游社账号，请先解绑",
    )
}

fn identity_conflict() -> Problem {
    Problem::conflict(
        "IDENTITY_CONFLICT",
        "这个米游社账号关联了多个绳网账号，请联系管理员",
    )
}

/// 处理失败时保存下来、供重放的错误（MHY-008）。只保存已知的业务错误，其余一律按内部错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Failure {
    RegistrationClosed,
    AccountBlocked,
    IdentityAlreadyBound,
    UserAlreadyBound,
    IdentityConflict,
    Internal,
}

impl Failure {
    fn from_problem(problem: &Problem) -> Self {
        match problem.code() {
            "REGISTRATION_CLOSED" => Self::RegistrationClosed,
            "ACCOUNT_BLOCKED" => Self::AccountBlocked,
            "IDENTITY_ALREADY_BOUND" => Self::IdentityAlreadyBound,
            "USER_ALREADY_BOUND" => Self::UserAlreadyBound,
            "IDENTITY_CONFLICT" => Self::IdentityConflict,
            _ => Self::Internal,
        }
    }

    fn problem(self) -> Problem {
        match self {
            Self::RegistrationClosed => ik_auth::service::registration_closed(),
            Self::AccountBlocked => ik_auth::service::account_blocked(),
            Self::IdentityAlreadyBound => identity_already_bound(),
            Self::UserAlreadyBound => user_already_bound(),
            Self::IdentityConflict => identity_conflict(),
            Self::Internal => Problem::internal(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Outcome {
    LoggedIn { user_id: i64, is_new_user: bool },
    Bound { user_id: i64 },
    Failed { failure: Failure },
}

// ---------- 扫码 ----------

fn check_rate(
    limiter: &governor::DefaultKeyedRateLimiter<IpAddr>,
    ip: IpAddr,
    what: &str,
) -> Result<(), Problem> {
    limiter.check_key(&ip).map_err(|_| rate_limited(what))
}

fn id_hash(qr_id: &str) -> Vec<u8> {
    Sha256::digest(qr_id.as_bytes()).to_vec()
}

/// 生成二维码（MHY-004）。`bind` 需要登录；`login` 即使带着登录态也按登录处理。
pub async fn create_qr(
    state: &MihoyoState,
    viewer: Option<&Principal>,
    request: CreateQrRequest,
    meta: &RequestMeta,
) -> Result<QrCreated, Problem> {
    if !state.enabled {
        return Err(disabled());
    }
    check_rate(&state.limits.create, meta.ip, "二维码生成")?;
    let user_id = match request.mode {
        QrMode::Login => None,
        QrMode::Bind => Some(viewer.ok_or_else(bind_requires_auth)?.user_id.get()),
    };
    let device = new_device(meta.user_agent.as_deref())?;
    let code = state
        .api
        .create_qr(&device)
        .await
        .map_err(|e| upstream_failed(&e))?;

    let now = Utc::now();
    let expires_in = code
        .expires_at
        .map(|at| (at - now).num_seconds())
        .filter(|secs| *secs > 0)
        .unwrap_or(QR_FALLBACK_EXPIRES_SECS);
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).map_err(|e| Problem::internal_from(&e))?;
    let qr_id = URL_SAFE_NO_PAD.encode(secret);
    let mut conn = state.auth.db.acquire().await?;
    repo::insert_qr(
        &mut conn,
        NewQr {
            id_hash: &id_hash(&qr_id),
            ticket: &code.ticket,
            mode: request.mode.as_str(),
            user_id,
            device: serde_json::to_value(&device).map_err(|e| Problem::internal_from(&e))?,
            expires_at: now
                + chrono::Duration::seconds(
                    QR_SESSION_MIN_SECS.max(expires_in + QR_SESSION_MARGIN_SECS),
                ),
        },
    )
    .await?;
    Ok(QrCreated {
        qr_id,
        url: code.url,
        expires_in,
        mode: request.mode,
    })
}

/// 一次轮询的结果。登录成功时 `issued` 带着会话，由路由按客户端类型交付令牌。
#[derive(Debug)]
pub struct Polled {
    pub status: QrStatus,
    pub mode: Option<QrMode>,
    pub is_new_user: Option<bool>,
    pub binding: Option<BindingView>,
    pub issued: Option<Issued>,
}

impl Polled {
    fn pending(status: QrStatus, mode: Option<QrMode>) -> Self {
        Self {
            status,
            mode,
            is_new_user: None,
            binding: None,
            issued: None,
        }
    }
}

fn parse_mode(raw: &str) -> QrMode {
    if raw == "bind" {
        QrMode::Bind
    } else {
        QrMode::Login
    }
}

/// 轮询（MHY-005～008）。顺序：结果重放 → 过期 → 别人正在处理 → 2 秒微缓存 → 查上游。
/// 「已确认」只有抢到处理权的那一次请求去建号 / 绑定，结果保存下来，之后的轮询（包括并发的）都重放它；
/// 登录的会话由二维码决定，重放拿到的是同一个会话。
pub async fn poll(
    state: &MihoyoState,
    viewer: Option<&Principal>,
    request: &PollQrRequest,
    meta: &RequestMeta,
) -> Result<Polled, Problem> {
    if !state.enabled {
        return Err(disabled());
    }
    check_rate(&state.limits.poll, meta.ip, "轮询")?;
    if request.qr_id.is_empty() || request.qr_id.len() > QR_ID_MAX_LEN {
        return Ok(Polled::pending(QrStatus::Expired, None));
    }
    let hash = id_hash(&request.qr_id);
    let now = Utc::now();
    let mut conn = state.auth.db.acquire().await?;
    let Some(row) = repo::qr(&mut conn, &hash).await? else {
        return Ok(Polled::pending(QrStatus::Expired, None));
    };
    let mode = parse_mode(&row.mode);
    if mode == QrMode::Bind && viewer.map(|p| p.user_id.get()) != row.user_id {
        return Err(user_mismatch());
    }
    if let (Some(result), Some(until)) = (row.result, row.result_expires_at) {
        if until <= now {
            repo::delete_qr(&mut conn, &hash).await?;
            return Ok(Polled::pending(QrStatus::Expired, Some(mode)));
        }
        drop(conn);
        let outcome: Outcome =
            serde_json::from_value(result).map_err(|e| Problem::internal_from(&e))?;
        return replay(state, mode, &hash, outcome, meta).await;
    }
    if row.expires_at <= now {
        repo::delete_qr(&mut conn, &hash).await?;
        return Ok(Polled::pending(QrStatus::Expired, Some(mode)));
    }
    if row.claimed_until.is_some_and(|until| until > now) {
        return Ok(Polled::pending(QrStatus::Scanned, Some(mode)));
    }
    if let (Some(status), Some(at)) = (row.last_status.as_deref(), row.last_polled_at)
        && now - at < chrono::Duration::seconds(STATUS_CACHE_SECS)
    {
        let status = if status == "scanned" {
            QrStatus::Scanned
        } else {
            QrStatus::Waiting
        };
        return Ok(Polled::pending(status, Some(mode)));
    }
    let device: Device =
        serde_json::from_value(row.device).map_err(|e| Problem::internal_from(&e))?;
    drop(conn);

    let status = state
        .api
        .query_qr(&row.ticket, &device)
        .await
        .map_err(|e| upstream_failed(&e))?;
    let mut conn = state.auth.db.acquire().await?;
    let confirmed = match status {
        upstream::QrStatus::Created | upstream::QrStatus::Scanned => {
            let (status, text) = if status == upstream::QrStatus::Created {
                (QrStatus::Waiting, "waiting")
            } else {
                (QrStatus::Scanned, "scanned")
            };
            repo::cache_status(&mut conn, &hash, text, now).await?;
            return Ok(Polled::pending(status, Some(mode)));
        }
        upstream::QrStatus::Expired | upstream::QrStatus::Cancelled => {
            repo::delete_qr(&mut conn, &hash).await?;
            let status = if status == upstream::QrStatus::Expired {
                QrStatus::Expired
            } else {
                QrStatus::Cancelled
            };
            return Ok(Polled::pending(status, Some(mode)));
        }
        upstream::QrStatus::Confirmed(confirmed) => confirmed,
    };
    if !repo::claim(
        &mut conn,
        &hash,
        now,
        now + chrono::Duration::seconds(CLAIM_SECS),
    )
    .await?
    {
        return Ok(Polled::pending(QrStatus::Scanned, Some(mode)));
    }
    drop(conn);

    let outcome = match process(state, mode, row.user_id, confirmed).await {
        Ok(outcome) => outcome,
        Err(problem) => Outcome::Failed {
            failure: Failure::from_problem(&problem),
        },
    };
    let ttl = if matches!(outcome, Outcome::Failed { .. }) {
        RESULT_FAILED_SECS
    } else {
        RESULT_OK_SECS
    };
    let mut conn = state.auth.db.acquire().await?;
    repo::store_result(
        &mut conn,
        &hash,
        serde_json::to_value(&outcome).map_err(|e| Problem::internal_from(&e))?,
        Utc::now() + chrono::Duration::seconds(ttl),
    )
    .await?;
    drop(conn);
    replay(state, mode, &hash, outcome, meta).await
}

/// 已确认：拉角色（失败不影响登录与绑定，只是没有角色资料），再建号 / 登录 / 绑定。
async fn process(
    state: &MihoyoState,
    mode: QrMode,
    user_id: Option<i64>,
    confirmed: Confirmed,
) -> Result<Outcome, Problem> {
    let role = match state.api.game_roles(&confirmed.cookies).await {
        Ok(roles) => upstream::main_role(roles),
        Err(error) => {
            tracing::warn!(%error, "拉取绝区零角色失败，跳过角色资料");
            None
        }
    };
    let now = Utc::now();
    let mid = confirmed.mid.as_deref();
    match (mode, user_id) {
        (QrMode::Login, _) => {
            let (user_id, is_new_user) =
                login(state, &confirmed.aid, mid, role.as_ref(), now).await?;
            Ok(Outcome::LoggedIn {
                user_id,
                is_new_user,
            })
        }
        (QrMode::Bind, Some(user_id)) => {
            bind(state, user_id, &confirmed.aid, mid, role.as_ref(), now).await?;
            Ok(Outcome::Bound { user_id })
        }
        (QrMode::Bind, None) => Err(Problem::internal()),
    }
}

async fn replay(
    state: &MihoyoState,
    mode: QrMode,
    id_hash: &[u8],
    outcome: Outcome,
    meta: &RequestMeta,
) -> Result<Polled, Problem> {
    let (user_id, is_new_user, issued) = match outcome {
        Outcome::Failed { failure } => return Err(failure.problem()),
        Outcome::Bound { user_id } => (user_id, None, None),
        Outcome::LoggedIn {
            user_id,
            is_new_user,
        } => {
            let secret = session_secret(state, user_id, id_hash)?;
            let mut tx = state.auth.db.begin().await?;
            let issued = ik_auth::service::issue_session_with_secret(
                &state.auth,
                &mut tx,
                user_id,
                "mihoyo",
                &secret,
                meta,
                Utc::now(),
            )
            .await?;
            tx.commit().await?;
            (user_id, Some(is_new_user), Some(issued))
        }
    };
    let mut conn = state.auth.db.acquire().await?;
    let binding = repo::binding_of(&mut conn, user_id)
        .await?
        .map(binding_view);
    Ok(Polled {
        status: QrStatus::Confirmed,
        mode: Some(mode),
        is_new_user,
        binding,
        issued,
    })
}

/// 确定性会话的秘密：同一个二维码、同一个账号永远是同一个（SESS-019）。
fn session_secret(state: &MihoyoState, user_id: i64, id_hash: &[u8]) -> Result<[u8; 32], Problem> {
    let mut mac = Hmac::<Sha256>::new_from_slice(&state.session_key)
        .map_err(|e| Problem::internal_from(&e))?;
    mac.update(format!("qr-session:{user_id}:").as_bytes());
    mac.update(id_hash);
    Ok(mac.finalize().into_bytes().into())
}

// ---------- 身份 ----------

/// aid 或 mid 指向的账号；指向多个账号时 409 `IDENTITY_CONFLICT`（IDENT-005）。
async fn resolve(
    conn: &mut PgConnection,
    aid: &str,
    mid: Option<&str>,
) -> Result<Option<i64>, Problem> {
    let users = repo::users_by_identity(conn, aid, mid).await?;
    if users.len() > 1 {
        tracing::error!(?users, "米游社身份指向多个账号");
        return Err(identity_conflict());
    }
    Ok(users.first().copied())
}

fn is_identity_taken(error: &sqlx::Error) -> bool {
    is_unique_violation(error, "mihoyo_bindings_aid_uq")
        || is_unique_violation(error, "mihoyo_bindings_mid_uq")
}

async fn sync_role(
    conn: &mut PgConnection,
    user_id: i64,
    role: Option<&GameRole>,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    if let Some(role) = role {
        repo::sync_role(
            conn,
            user_id,
            &NewRole {
                uid: &role.game_uid,
                nickname: &role.nickname,
                level: role.level,
                region: &role.region,
                region_name: &role.region_name,
            },
            now,
        )
        .await?;
    }
    Ok(())
}

/// 扫码登录（MHY-010、MHY-011）：已绑定的账号直接登录并同步角色；没有则建号（注册开关关闭时拒绝）。
/// 并发首登撞上唯一约束时回滚，重新解析一次，以赢家的账号登录——绝不产生第二个账号。
async fn login(
    state: &MihoyoState,
    aid: &str,
    mid: Option<&str>,
    role: Option<&GameRole>,
    now: DateTime<Utc>,
) -> Result<(i64, bool), Problem> {
    for _ in 0..2 {
        let mut tx = state.auth.db.begin().await?;
        if let Some(user_id) = resolve(&mut tx, aid, mid).await? {
            ik_auth::interop::lock_active_user(&mut tx, user_id)
                .await?
                .ok_or_else(ik_auth::service::account_blocked)?;
            repo::upsert_identity(&mut tx, user_id, aid, mid).await?;
            sync_role(&mut tx, user_id, role, now).await?;
            tx.commit().await?;
            return Ok((user_id, false));
        }
        if !state.auth.settings.registration_open {
            return Err(ik_auth::service::registration_closed());
        }
        let user_id = state.auth.ids.next_raw();
        create_user(&mut tx, user_id, role.map(|r| r.nickname.as_str()), now).await?;
        match repo::upsert_identity(&mut tx, user_id, aid, mid).await {
            Ok(()) => {}
            // 另一路同时建好了：回滚，下一轮以它为准。
            Err(error) if is_identity_taken(&error) => continue,
            Err(error) => return Err(error.into()),
        }
        sync_role(&mut tx, user_id, role, now).await?;
        tx.commit().await?;
        return Ok((user_id, true));
    }
    tracing::error!("米游社首登连续撞上唯一约束");
    Err(Problem::internal())
}

/// 用户名（MHY-012）：优先绝区零昵称，冲突时试 5 个 `昵称#四位数字`，再 20 个 `绳网用户四位数字`，
/// 最后 `绳网用户八位数字`。
async fn create_user(
    conn: &mut PgConnection,
    user_id: i64,
    nickname: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    let mut candidates = Vec::new();
    if let Some(base) = nickname.and_then(usable_nickname) {
        let short: String = base.chars().take(USERNAME_MAX_CHARS - 5).collect();
        candidates.push(base);
        for _ in 0..NICKNAME_VARIANTS {
            candidates.push(format!("{short}#{}", digits(4)?));
        }
    }
    for _ in 0..GENERIC_TRIES {
        candidates.push(format!("{GENERIC_PREFIX}{}", digits(4)?));
    }
    candidates.push(format!("{GENERIC_PREFIX}{}", digits(8)?));
    for name in candidates {
        if ik_auth::interop::insert_user_without_email(conn, user_id, &name, now).await? {
            return Ok(());
        }
    }
    tracing::error!("米游社首登的用户名连续冲突");
    Err(Problem::internal())
}

/// 昵称可以直接当用户名：去首尾空白后至少 2 个字符，截到 20 个，不含 `@` 与控制字符（ACC-009）。
fn usable_nickname(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.contains('@') || trimmed.chars().any(char::is_control) {
        return None;
    }
    let name: String = trimmed.chars().take(USERNAME_MAX_CHARS).collect();
    let name = name.trim_end().to_owned();
    (name.chars().count() >= NICKNAME_MIN_CHARS).then_some(name)
}

fn digits(n: usize) -> Result<String, Problem> {
    let mut out = String::with_capacity(n);
    let mut buf = [0u8; 16];
    while out.len() < n {
        getrandom::fill(&mut buf).map_err(|e| Problem::internal_from(&e))?;
        // 250 = 10 × 25，丢弃 250..=255 以免取模偏差。
        for &b in buf.iter().filter(|&&b| b < 250) {
            if out.len() == n {
                break;
            }
            out.push(char::from(b'0' + b % 10));
        }
    }
    Ok(out)
}

/// 绑定（MHY-013）：米游社账号已绑在别的账号上 → 409 `IDENTITY_ALREADY_BOUND`（不接管任何账号）；
/// 当前账号已绑了另一个米游社账号 → 409 `USER_ALREADY_BOUND`；重复绑定同一个是幂等的。
async fn bind(
    state: &MihoyoState,
    user_id: i64,
    aid: &str,
    mid: Option<&str>,
    role: Option<&GameRole>,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    let mut tx = state.auth.db.begin().await?;
    ik_auth::interop::lock_active_user(&mut tx, user_id)
        .await?
        .ok_or_else(ik_auth::service::account_blocked)?;
    if resolve(&mut tx, aid, mid)
        .await?
        .is_some_and(|owner| owner != user_id)
    {
        return Err(identity_already_bound());
    }
    if repo::binding_of(&mut tx, user_id)
        .await?
        .is_some_and(|b| b.aid != aid)
    {
        return Err(user_already_bound());
    }
    match repo::upsert_identity(&mut tx, user_id, aid, mid).await {
        Ok(()) => {}
        Err(error) if is_identity_taken(&error) => return Err(identity_already_bound()),
        Err(error) => return Err(error.into()),
    }
    sync_role(&mut tx, user_id, role, now).await?;
    tx.commit().await?;
    Ok(())
}

// ---------- 自己的绑定 ----------

fn binding_view(row: BindingRow) -> BindingView {
    let zzz = row.zzz_uid.map(|uid| ZzzRole {
        uid,
        nickname: row.zzz_nickname.unwrap_or_default(),
        level: row.zzz_level.unwrap_or(0),
        region: row.zzz_region.unwrap_or_default(),
        region_name: row.zzz_region_name.unwrap_or_default(),
    });
    BindingView {
        aid: row.aid,
        zzz,
        last_synced_at: row.last_synced_at,
        bound_at: row.created_at,
    }
}

/// 自己的米游社绑定（MHY-015）。
pub async fn my_binding(state: &MihoyoState, principal: &Principal) -> Result<MyBinding, Problem> {
    let mut conn = state.auth.db.acquire().await?;
    Ok(MyBinding {
        binding: repo::binding_of(&mut conn, principal.user_id.get())
            .await?
            .map(binding_view),
    })
}

/// 解绑（MHY-014）。没有邮箱的账号只能扫码登录，解绑会把自己锁在外面，所以禁止；
/// 有邮箱即可，不要求设过密码（可以用验证码登录或重置密码）。
pub async fn unbind(state: &MihoyoState, principal: &Principal) -> Result<(), Problem> {
    let user_id = principal.user_id.get();
    let mut tx = state.auth.db.begin().await?;
    ik_auth::interop::lock_active_user(&mut tx, user_id)
        .await?
        .ok_or_else(ik_auth::service::account_blocked)?;
    if !ik_auth::interop::has_email(&mut tx, user_id).await? {
        return Err(Problem::new(
            StatusCode::FORBIDDEN,
            "MIHOYO_UNBIND_FORBIDDEN",
            "当前账号只能通过米游社扫码登录，请先绑定邮箱",
        ));
    }
    if !repo::delete_binding(&mut tx, user_id).await? {
        return Err(Problem::new(
            StatusCode::NOT_FOUND,
            "MIHOYO_NOT_BOUND",
            "当前账号未绑定米游社",
        ));
    }
    tx.commit().await?;
    Ok(())
}

/// 删除二维码与结果都已过期的扫码会话，返回删除数。
pub async fn prune(state: &MihoyoState) -> Result<u64, Problem> {
    let mut conn = state.auth.db.acquire().await?;
    Ok(repo::prune(&mut conn, Utc::now()).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nicknames_become_usernames_when_usable() {
        assert_eq!(usable_nickname("  邦布  ").as_deref(), Some("邦布"));
        assert_eq!(usable_nickname("a").as_deref(), None);
        assert_eq!(usable_nickname("me@x").as_deref(), None);
        assert_eq!(usable_nickname("line\nbreak").as_deref(), None);
        let long = "一二三四五六七八九十一二三四五六七八九十多出来";
        assert_eq!(usable_nickname(long).unwrap().chars().count(), 20);
    }

    #[test]
    fn digits_are_digits() {
        let d = digits(8).unwrap();
        assert_eq!(d.len(), 8);
        assert!(d.bytes().all(|b| b.is_ascii_digit()));
    }

    #[test]
    fn failures_round_trip_known_codes_only() {
        for failure in [
            Failure::RegistrationClosed,
            Failure::AccountBlocked,
            Failure::IdentityAlreadyBound,
            Failure::UserAlreadyBound,
            Failure::IdentityConflict,
            Failure::Internal,
        ] {
            assert_eq!(Failure::from_problem(&failure.problem()), failure);
        }
        assert_eq!(
            Failure::from_problem(&Problem::not_found()),
            Failure::Internal
        );
    }
}
