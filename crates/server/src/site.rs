//! 站点状态（公告、只读维护模式）：主站启动时读 `GET /v1/site`；只读模式与 IP 封禁由 [`guard`] 中间件执行。
//! 都在后台修改（`ik_kernel::site`），最长 60 秒后各实例生效。

use axum::Json;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use http::{Method, StatusCode};
use ik_auth::service::Resolution;
use ik_kernel::Problem;
use ik_kernel::site::AnnouncementTone;
use serde::Serialize;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AppState;

const DEFAULT_READ_ONLY_MESSAGE: &str = "绳网正在维护，暂时只能浏览，请稍后再来";

/// 只读模式下照常放行的写接口：登录登出（管理员要能登录进来关掉它）。
fn exempt(path: &str) -> bool {
    path.starts_with("/v1/auth/login") || path == "/v1/auth/logout"
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(site_status))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SiteStatus {
    /// 当前要显示的公告；没有时为空。
    pub announcement: Option<AnnouncementView>,
    pub read_only: Option<ReadOnlyView>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnnouncementView {
    /// 内容的摘要：内容或链接变了就变，客户端用它记住「已关闭」。
    pub id: String,
    pub text: String,
    pub link: Option<String>,
    pub tone: AnnouncementTone,
    pub ends_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadOnlyView {
    pub message: String,
}

/// 站点公告与维护状态
///
/// 公开接口，不需要登录。
#[utoipa::path(
    get,
    path = "/v1/site",
    tag = "system",
    responses((status = 200, body = SiteStatus))
)]
async fn site_status(State(state): State<AppState>) -> Json<SiteStatus> {
    let values = state.site.get().await;
    let a = &values.announcement;
    let announcement = a.active_at(Utc::now()).then(|| {
        let digest = Sha256::digest(format!("{}\n{}", a.text, a.link).as_bytes());
        AnnouncementView {
            id: hex(&digest[..8]),
            text: a.text.clone(),
            link: (!a.link.is_empty()).then(|| a.link.clone()),
            tone: a.tone,
            ends_at: a.ends_at,
        }
    });
    let read_only = values.read_only.enabled.then(|| ReadOnlyView {
        message: read_only_message(&values.read_only.message),
    });
    Json(SiteStatus {
        announcement,
        read_only,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn read_only_message(message: &str) -> String {
    if message.trim().is_empty() {
        DEFAULT_READ_ONLY_MESSAGE.to_owned()
    } else {
        message.trim().to_owned()
    }
}

/// 写请求的站点级门控（普通用户与游客；版主、管理员不受影响）：
/// - IP 封禁：命中的地址不能注册、登录与执行写操作，403 `IP_BANNED`；
/// - 只读维护模式：除登录登出外的写请求，503 `READ_ONLY`。
pub async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let write = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    // 后台只对版主与管理员开放；QQ 回调与剧情库 MCP 不是用户在站内写内容。
    if !write
        || !path.starts_with("/v1/")
        || path.starts_with("/v1/admin/")
        || path == "/v1/qq/webhook"
        || path == "/v1/story/mcp"
    {
        return next.run(request).await;
    }
    let values = state.site.get().await;
    let read_only = values.read_only.enabled && !exempt(path);
    let banned = !values.ip_bans.is_empty() && {
        let peer = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |info| info.0.ip());
        let ip = ik_kernel::http::client_ip(
            request.headers(),
            peer,
            state.config.http.trusted_proxy_hops,
        );
        values.ip_banned(ip, Utc::now())
    };
    if !read_only && !banned {
        return next.run(request).await;
    }
    let staff = match ik_auth::credential_from(request.headers(), &state.auth.settings.cookie_name)
    {
        Some(token) => matches!(
            ik_auth::service::resolve(&state.auth, &token).await,
            Ok(Resolution::Active(principal)) if principal.role.is_staff()
        ),
        None => false,
    };
    if staff {
        return next.run(request).await;
    }
    if banned {
        return Problem::new(
            StatusCode::FORBIDDEN,
            "IP_BANNED",
            "你所在的网络已被限制，暂时不能执行此操作",
        )
        .into_response();
    }
    // 5xx 的 detail 是统一文案；客户端按 code 显示 `GET /v1/site` 里的维护说明。
    Problem::new(StatusCode::SERVICE_UNAVAILABLE, "READ_ONLY", "站点维护中")
        .with_retry_after(300)
        .into_response()
}
