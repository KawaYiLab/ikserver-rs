//! 提取器：当前用户、可选用户、客户端信息、认证接口限流。
//!
//! 令牌来源优先级：`Authorization: Bearer <token>`（App）> 会话 Cookie（网页）。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::extract::{ConnectInfo, FromRef, FromRequestParts};
use axum_extra::extract::cookie::CookieJar;
use governor::clock::{Clock, DefaultClock};
use http::request::Parts;
use http::{StatusCode, header};
use ik_kernel::id::marker::{Session, User};
use ik_kernel::{Id, Problem, Role};

use crate::AuthState;
use crate::service::{self, RequestMeta, Resolution};

const USER_AGENT_MAX_CHARS: usize = 255;
/// 限流器里的键超过这个数量时清理过期键，防止内存无限增长。
const LIMITER_RETAIN_THRESHOLD: usize = 10_000;

/// 已认证请求的身份。
#[derive(Debug, Clone)]
pub struct Principal {
    pub user_id: Id<User>,
    pub session_id: Id<Session>,
    pub role: Role,
    pub exam_passed: bool,
    pub is_ai_agent: bool,
    /// 用户等级 0–7（0 = 未通过考试）。
    pub level: i16,
}

impl Principal {
    /// 是否可以执行写操作：通过入站考试，或为工作人员 / AI 账号（REG-007）。
    pub fn can_write(&self) -> bool {
        self.exam_passed || self.role.is_staff() || self.is_ai_agent
    }

    /// 写操作的入站考试门控（POST-040）。
    pub fn require_write(&self) -> Result<(), Problem> {
        if self.can_write() {
            Ok(())
        } else {
            Err(Problem::new(
                StatusCode::FORBIDDEN,
                "EXAM_REQUIRED",
                "需要先通过入站考试才能执行此操作",
            ))
        }
    }

    /// 权益矩阵使用的等级：管理员与 AI 账号恒为最高档（POST-012）。
    pub fn benefit_level(&self) -> usize {
        if self.role.is_admin() || self.is_ai_agent {
            7
        } else {
            usize::try_from(self.level.clamp(0, 7)).unwrap_or(0)
        }
    }
}

fn credential(parts: &Parts, cookie_name: &str) -> Option<String> {
    if let Some(value) = parts.headers.get(header::AUTHORIZATION) {
        return value
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|t| t.trim().to_owned());
    }
    CookieJar::from_headers(&parts.headers)
        .get(cookie_name)
        .map(|c| c.value().to_owned())
}

/// 必须登录。没带凭据 → 401 `UNAUTHORIZED`；凭据失效 → 401 `SESSION_EXPIRED`；
/// 账号被封禁 → 403 `ACCOUNT_BLOCKED`。
#[derive(Debug, Clone)]
pub struct CurrentUser(pub Principal);

impl<S> FromRequestParts<S> for CurrentUser
where
    S: Send + Sync,
    AuthState: FromRef<S>,
{
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AuthState::from_ref(state);
        let Some(token) = credential(parts, &state.settings.cookie_name) else {
            return Err(Problem::unauthorized());
        };
        match service::resolve(&state, &token).await? {
            Resolution::Active(principal) => Ok(Self(principal)),
            Resolution::Invalid => Err(Problem::new(
                StatusCode::UNAUTHORIZED,
                "SESSION_EXPIRED",
                "登录已过期，请重新登录",
            )),
            Resolution::Blocked => Err(service::account_blocked()),
        }
    }
}

/// 可选登录：凭据缺失、失效或账号不可用时按匿名处理。
#[derive(Debug, Clone)]
pub struct MaybeUser(pub Option<Principal>);

impl<S> FromRequestParts<S> for MaybeUser
where
    S: Send + Sync,
    AuthState: FromRef<S>,
{
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AuthState::from_ref(state);
        let Some(token) = credential(parts, &state.settings.cookie_name) else {
            return Ok(Self(None));
        };
        Ok(Self(match service::resolve(&state, &token).await? {
            Resolution::Active(principal) => Some(principal),
            Resolution::Invalid | Resolution::Blocked => None,
        }))
    }
}

/// 客户端真实 IP 与 User-Agent。
#[derive(Debug, Clone)]
pub struct ClientMeta(pub RequestMeta);

fn client_ip(parts: &Parts, trusted_hops: u32) -> IpAddr {
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |info| info.0.ip());
    ik_kernel::http::client_ip(&parts.headers, peer, trusted_hops)
}

impl<S> FromRequestParts<S> for ClientMeta
where
    S: Send + Sync,
    AuthState: FromRef<S>,
{
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AuthState::from_ref(state);
        let user_agent = parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|ua| ua.chars().take(USER_AGENT_MAX_CHARS).collect());
        Ok(Self(RequestMeta {
            ip: client_ip(parts, state.settings.trusted_proxy_hops),
            user_agent,
        }))
    }
}

/// 认证接口限流：每 IP 每条路由每分钟 20 次（RL-001）。放在 handler 参数里即生效。
/// 键不含请求体里的邮箱/用户名，否则攻击者换个值就能拿到新配额。
#[derive(Debug, Clone, Copy)]
pub struct AuthRateLimit;

impl<S> FromRequestParts<S> for AuthRateLimit
where
    S: Send + Sync,
    AuthState: FromRef<S>,
{
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AuthState::from_ref(state);
        let ip = client_ip(parts, state.settings.trusted_proxy_hops);
        let key = format!(
            "{}|{ip}",
            parts.uri.path().trim_end_matches('/').to_lowercase()
        );
        if state.limiter.len() > LIMITER_RETAIN_THRESHOLD {
            state.limiter.retain_recent();
        }
        state
            .limiter
            .check_key(&key)
            .map(|()| Self)
            .map_err(|denied| {
                let wait = denied.wait_time_from(DefaultClock::default().now());
                Problem::rate_limited().with_retry_after(wait.as_secs().saturating_add(1))
            })
    }
}
