//! HTTP 接口：`/v1/auth/mihoyo/qr*`、`/v1/me/mihoyo`。

use axum::extract::{FromRef, State};
use axum_extra::extract::cookie::CookieJar;
use http::StatusCode;
use ik_auth::{AuthState, ClientMeta, CurrentUser, MaybeUser};
use ik_kernel::Problem;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::Json;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::MihoyoState;
use crate::dto::{CreateQrRequest, MyBinding, PollQrRequest, QrCreated, QrPollResponse};
use crate::service;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    MihoyoState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(create_qr))
        .routes(routes!(poll_qr))
        .routes(routes!(my_binding, unbind))
}

/// 生成米游社扫码二维码
///
/// `mode` 必填：`login` 扫码登录（没有账号时自动建号；带着登录态也按登录处理），`bind` 给当前账号绑定
/// （需要登录）。每 IP 每分钟最多 30 次。
#[utoipa::path(
    post,
    path = "/v1/auth/mihoyo/qr",
    tag = "auth",
    request_body = CreateQrRequest,
    responses(
        (status = 201, body = QrCreated),
        (status = 401, description = "MIHOYO_BIND_REQUIRES_AUTH", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 502, description = "MIHOYO_UPSTREAM_ERROR", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "MIHOYO_DISABLED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn create_qr(
    State(state): State<MihoyoState>,
    MaybeUser(viewer): MaybeUser,
    ClientMeta(meta): ClientMeta,
    Json(request): Json<CreateQrRequest>,
) -> Result<(StatusCode, Json<QrCreated>), Problem> {
    let created = service::create_qr(&state, viewer.as_ref(), request, &meta).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// 轮询扫码结果
///
/// 返回 `waiting` / `scanned` / `expired` / `cancelled`，或 `confirmed`：登录模式带 `auth`（网页端令牌写进
/// Cookie，App 端在 `auth.token`），绑定模式带 `binding`。确认后 5 分钟内重复轮询返回同一结果、同一个会话。
/// 绑定模式只有发起绑定的账号能轮询。每 IP 每分钟最多 600 次，建议间隔 1.5 秒（扫码后 3 秒）。
#[utoipa::path(
    post,
    path = "/v1/auth/mihoyo/qr/poll",
    tag = "auth",
    request_body = PollQrRequest,
    responses(
        (status = 200, body = QrPollResponse),
        (status = 403, description = "MIHOYO_BIND_USER_MISMATCH / ACCOUNT_BLOCKED / REGISTRATION_CLOSED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "IDENTITY_ALREADY_BOUND / USER_ALREADY_BOUND / IDENTITY_CONFLICT / QR_SESSION_CONSUMED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 502, description = "MIHOYO_UPSTREAM_ERROR", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn poll_qr(
    State(state): State<MihoyoState>,
    MaybeUser(viewer): MaybeUser,
    ClientMeta(meta): ClientMeta,
    jar: CookieJar,
    Json(request): Json<PollQrRequest>,
) -> Result<(CookieJar, Json<QrPollResponse>), Problem> {
    let polled = service::poll(&state, viewer.as_ref(), &request, &meta).await?;
    let (jar, auth) = match polled.issued {
        Some(issued) => {
            let (jar, body) = ik_auth::deliver(&state.auth.settings, jar, issued, request.client);
            (jar, Some(body))
        }
        None => (jar, None),
    };
    Ok((
        jar,
        Json(QrPollResponse {
            status: polled.status,
            mode: polled.mode,
            is_new_user: polled.is_new_user,
            binding: polled.binding,
            auth,
        }),
    ))
}

/// 我的米游社绑定
#[utoipa::path(
    get,
    path = "/v1/me/mihoyo",
    tag = "me",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = MyBinding),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn my_binding(
    State(state): State<MihoyoState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<MyBinding>, Problem> {
    Ok(Json(service::my_binding(&state, &principal).await?))
}

/// 解绑米游社
///
/// 没有邮箱的账号（只能扫码登录）不能解绑。
#[utoipa::path(
    delete,
    path = "/v1/me/mihoyo",
    tag = "me",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已解绑"),
        (status = 403, description = "MIHOYO_UNBIND_FORBIDDEN / ACCOUNT_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "MIHOYO_NOT_BOUND", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unbind(
    State(state): State<MihoyoState>,
    CurrentUser(principal): CurrentUser,
) -> Result<StatusCode, Problem> {
    service::unbind(&state, &principal).await?;
    Ok(StatusCode::NO_CONTENT)
}
