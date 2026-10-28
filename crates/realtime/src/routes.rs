//! HTTP 接口：`GET /v1/realtime`（WebSocket 握手）。

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::{FromRef, State};
use axum::response::Response;
use http::{HeaderMap, StatusCode, header};
use ik_auth::{AuthState, UserIfPresent};
use ik_kernel::Problem;
use ik_kernel::error::ProblemBody;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::RealtimeState;
use crate::conn::Connection;
use crate::protocol::MAX_FRAME_BYTES;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    RealtimeState: FromRef<S>,
{
    OpenApiRouter::new().routes(routes!(realtime))
}

/// 用 Cookie 鉴权的网页连接必须来自同源页面或白名单来源，防止跨站 WebSocket 劫持
/// （握手是 GET，不经过 CSRF 中间件；RL-005）。Bearer 令牌不会被浏览器自动携带，不受此限制。
fn check_origin(state: &RealtimeState, headers: &HeaderMap) -> Result<(), Problem> {
    if headers.contains_key(header::AUTHORIZATION) {
        return Ok(());
    }
    match headers.get(header::ORIGIN).map(|v| v.to_str()) {
        None => Ok(()),
        Some(Ok(origin)) if state.allowed_origins.iter().any(|o| o == origin) => Ok(()),
        Some(Ok(origin)) if same_origin(origin, headers) => Ok(()),
        Some(_) => Err(Problem::new(
            StatusCode::FORBIDDEN,
            "CSRF_REJECTED",
            "跨站请求被拒绝",
        )),
    }
}

/// `Origin` 的主机（含端口）与请求的 `Host` 相同：页面和接口挂在同一个域名下。
/// 经 CDN / 反向代理时要求回源 `Host` 保持原域名（见 deploy/interknot/README.md「EdgeOne」）。
fn same_origin(origin: &str, headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    origin
        .split_once("://")
        .is_some_and(|(_, authority)| authority.eq_ignore_ascii_case(host))
}

/// 实时通道（WebSocket）
///
/// 网页用同站 Cookie，App 用 `Authorization: Bearer`。不带凭据时以游客身份连接，只收广播与公开频道；
/// 带了凭据但已失效时握手失败（401），不会静默降级为游客。协议见接口规范「实时」一节。
#[utoipa::path(
    get,
    path = "/v1/realtime",
    tag = "realtime",
    responses(
        (status = 101, description = "升级为 WebSocket"),
        (status = 401, description = "SESSION_EXPIRED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "ACCOUNT_BLOCKED / CSRF_REJECTED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 426, description = "WEBSOCKET_REQUIRED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "SERVICE_UNAVAILABLE：游客连接已满或正在停机", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn realtime(
    State(state): State<RealtimeState>,
    State(auth): State<AuthState>,
    UserIfPresent(principal): UserIfPresent,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, Problem> {
    check_origin(&state, &headers)?;
    let upgrade = upgrade.map_err(|_| {
        Problem::new(
            StatusCode::UPGRADE_REQUIRED,
            "WEBSOCKET_REQUIRED",
            "请使用 WebSocket 连接",
        )
    })?;
    let identity = principal
        .as_ref()
        .map(|p| (p.user_id.get(), p.session_id.get()));
    let registration = state
        .hub
        .register(identity)
        .ok_or_else(|| Problem::service_unavailable().with_retry_after(30))?;
    let id = registration.id;
    let hub = state.hub.clone();
    let connection = Connection {
        hub: state.hub.clone(),
        auth,
        principal,
        registration,
        typing: state.typing.clone(),
        last_typing: None,
    };
    Ok(upgrade
        .max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_failed_upgrade(move |_| hub.unregister(id))
        .on_upgrade(move |socket| connection.run(socket)))
}
