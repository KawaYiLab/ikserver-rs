//! 全站默认限流（RL-002～RL-005）：`/v1/` 下的所有请求，按「游客 / 登录用户 × 读 / 写」四个桶，
//! 每分钟次数见 `config.rate_limit`。登录用户按用户计（换 IP 不换桶），游客按客户端 IP 计。
//!
//! - 每个响应带 `RateLimit-Limit`（每分钟次数）、`RateLimit-Remaining`、`RateLimit-Reset`（几秒后完全恢复）；
//!   超限 429 `RATE_LIMITED` + `Retry-After`，不进入下游。
//! - 令牌桶（GCRA）而不是固定窗口，没有窗口边界放行两倍的问题（RL-007）。
//! - 身份要查会话：令牌摘要 → 用户 id 缓存 30 秒，所以大多数请求不多查一次库。令牌无效、账号封禁、
//!   查库失败都按游客处理（RL-004）；随便编的令牌落在 IP 桶里，拿不到新配额。
//! - 进程内计数（不使用 Redis）：多实例时每个实例各自计。
//! - 读 = GET / HEAD，外加剧情库 MCP 的 POST（JSON-RPC 只读查询）。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use governor::clock::{Clock, DefaultClock};
use governor::middleware::StateInformationMiddleware;
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};
use http::{HeaderMap, HeaderValue, Method};
use ik_auth::AuthState;
use ik_auth::service::Resolution;
use ik_kernel::Problem;
use ik_kernel::config::RateLimit;
use moka::future::Cache;
use sha2::{Digest, Sha256};

use crate::AppState;

const SESSION_CACHE_TTL: Duration = Duration::from_secs(30);
const SESSION_CACHE_CAPACITY: u64 = 100_000;
/// 按读计的 POST 接口。
const STORY_MCP_PATH: &str = "/v1/story/mcp";
/// 多久清理一次长时间没有请求的桶。
const SWEEP_EVERY: Duration = Duration::from_secs(60);

type Limiter =
    RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock, StateInformationMiddleware>;

struct Bucket {
    limit: u32,
    limiter: Limiter,
}

impl Bucket {
    fn new(per_minute: u32) -> Self {
        let per_minute = NonZeroU32::new(per_minute).unwrap_or(NonZeroU32::MIN);
        Self {
            limit: per_minute.get(),
            limiter: RateLimiter::keyed(Quota::per_minute(per_minute))
                .with_middleware::<StateInformationMiddleware>(),
        }
    }
}

pub struct GlobalLimiter {
    enabled: bool,
    anon_read: Bucket,
    anon_write: Bucket,
    user_read: Bucket,
    user_write: Bucket,
    /// 令牌 SHA-256 → 用户 id（`None`：令牌无效或账号不可用）。不保存令牌原文。
    sessions: Cache<[u8; 32], Option<i64>>,
}

impl GlobalLimiter {
    pub fn new(config: &RateLimit) -> Arc<Self> {
        let limiter = Arc::new(Self {
            enabled: config.enabled,
            anon_read: Bucket::new(config.anon_read_per_min),
            anon_write: Bucket::new(config.anon_write_per_min),
            user_read: Bucket::new(config.user_read_per_min),
            user_write: Bucket::new(config.user_write_per_min),
            sessions: Cache::builder()
                .max_capacity(SESSION_CACHE_CAPACITY)
                .time_to_live(SESSION_CACHE_TTL)
                .build(),
        });
        // 定期丢掉已经恢复满额的桶，防止键无限增长。CLI 等没有运行时的场景不需要。
        if limiter.enabled
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let weak = Arc::downgrade(&limiter);
            runtime.spawn(async move {
                let mut tick = tokio::time::interval(SWEEP_EVERY);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    let Some(limiter) = weak.upgrade() else { break };
                    for bucket in limiter.buckets() {
                        bucket.limiter.retain_recent();
                        bucket.limiter.shrink_to_fit();
                    }
                }
            });
        }
        limiter
    }

    fn buckets(&self) -> [&Bucket; 4] {
        [
            &self.anon_read,
            &self.anon_write,
            &self.user_read,
            &self.user_write,
        ]
    }

    /// 已登录请求的用户 id。
    async fn user_id(&self, auth: &AuthState, headers: &HeaderMap) -> Option<i64> {
        let token = ik_auth::credential_from(headers, &auth.settings.cookie_name)?;
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        if let Some(cached) = self.sessions.get(&digest).await {
            return cached;
        }
        let user_id = match ik_auth::service::resolve(auth, &token).await {
            Ok(Resolution::Active(principal)) => Some(principal.user_id.get()),
            Ok(Resolution::Invalid | Resolution::Blocked) => None,
            // 查库失败不缓存，按游客处理；接口自己的鉴权会报出真正的错误。
            Err(_) => return None,
        };
        self.sessions.insert(digest, user_id).await;
        user_id
    }
}

pub async fn layer(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let limiter = state.rate_limit.clone();
    // QQ 回调来自少量平台出口 IP；由 qqbot 在认证后按已配置 AppID 限流。
    if !limiter.enabled
        || !request.uri().path().starts_with("/v1/")
        || request.uri().path() == "/v1/qq/webhook"
    {
        return next.run(request).await;
    }
    // 剧情库 MCP 是只读查询，只是 JSON-RPC 规定用 POST：按读计，否则总结一章要读几十次就会撞上写的限额。
    let read = matches!(*request.method(), Method::GET | Method::HEAD)
        || request.uri().path() == STORY_MCP_PATH;
    let (bucket, key) = match limiter.user_id(&state.auth, request.headers()).await {
        Some(user_id) => (
            if read {
                &limiter.user_read
            } else {
                &limiter.user_write
            },
            format!("u:{user_id}"),
        ),
        None => {
            let peer = request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |info| info.0.ip());
            let ip = ik_kernel::http::client_ip(
                request.headers(),
                peer,
                state.config.http.trusted_proxy_hops,
            );
            (
                if read {
                    &limiter.anon_read
                } else {
                    &limiter.anon_write
                },
                format!("ip:{ip}"),
            )
        }
    };
    match bucket.limiter.check_key(&key) {
        Ok(snapshot) => {
            let remaining = snapshot.remaining_burst_capacity();
            let quota = snapshot.quota();
            let mut response = next.run(request).await;
            set_headers(
                response.headers_mut(),
                bucket.limit,
                remaining,
                reset_secs(quota, bucket.limit.saturating_sub(remaining)),
            );
            response
        }
        Err(denied) => {
            let wait = denied.wait_time_from(DefaultClock::default().now());
            let retry_after = wait.as_secs().saturating_add(1);
            let mut response = Problem::rate_limited()
                .with_retry_after(retry_after)
                .into_response();
            set_headers(
                response.headers_mut(),
                bucket.limit,
                0,
                reset_secs(denied.quota(), bucket.limit),
            );
            response
        }
    }
}

/// 用掉的 `used` 次额度全部恢复要几秒（向上取整）。
fn reset_secs(quota: Quota, used: u32) -> u64 {
    let nanos = quota
        .replenish_interval()
        .as_nanos()
        .saturating_mul(u128::from(used));
    u64::try_from(nanos.div_ceil(1_000_000_000)).unwrap_or(u64::MAX)
}

fn set_headers(headers: &mut HeaderMap, limit: u32, remaining: u32, reset: u64) {
    headers.insert("ratelimit-limit", HeaderValue::from(limit));
    headers.insert("ratelimit-remaining", HeaderValue::from(remaining));
    headers.insert("ratelimit-reset", HeaderValue::from(reset));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_is_the_time_to_refill_what_was_used() {
        let quota = Quota::per_minute(NonZeroU32::new(60).unwrap());
        assert_eq!(reset_secs(quota, 0), 0);
        assert_eq!(reset_secs(quota, 1), 1);
        assert_eq!(reset_secs(quota, 60), 60);
        let quota = Quota::per_minute(NonZeroU32::new(120).unwrap());
        assert_eq!(reset_secs(quota, 1), 1, "半秒向上取整");
        assert_eq!(reset_secs(quota, 3), 2);
    }
}
