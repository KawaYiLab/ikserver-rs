//! HTTP 接口：`/v1/me/security`、`/v1/me/delete-account*`。

use axum::extract::{FromRef, State};
use axum_extra::extract::cookie::CookieJar;
use http::StatusCode;
use ik_auth::dto::SendCodeResponse;
use ik_auth::{AuthRateLimit, AuthState, ClientMeta, CurrentUser};
use ik_kernel::Problem;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::Json;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::dto::{DeleteAccountRequest, SecurityView};
use crate::service;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(security))
        .routes(routes!(send_delete_code))
        .routes(routes!(delete_account))
}

/// 账号安全概览
///
/// 邮箱、是否设置过密码、是否绑定米游社，以及注销时用哪种方式核验。
#[utoipa::path(
    get,
    path = "/v1/me/security",
    tag = "me",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = SecurityView),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn security(
    State(state): State<AuthState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<SecurityView>, Problem> {
    Ok(Json(service::security(&state, &principal).await?))
}

/// 发注销验证码
///
/// 发到自己绑定的邮箱。没有邮箱的账号返回 400 `DELETE_ACCOUNT_NO_EMAIL`，改用米游社扫码核验
/// （`POST /v1/auth/mihoyo/qr`，`mode = delete`）。
#[utoipa::path(
    post,
    path = "/v1/me/delete-account/code",
    tag = "me",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = SendCodeResponse),
        (status = 400, description = "DELETE_ACCOUNT_NO_EMAIL", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "CODE_COOLDOWN / RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn send_delete_code(
    _limit: AuthRateLimit,
    State(state): State<AuthState>,
    CurrentUser(principal): CurrentUser,
    ClientMeta(meta): ClientMeta,
) -> Result<Json<SendCodeResponse>, Problem> {
    Ok(Json(
        service::send_delete_code(&state, &principal, &meta).await?,
    ))
}

/// 注销账号
///
/// 立即生效、不可撤回。有邮箱的账号给 `code`，没有的给扫码核验通过的 `qrId`。用户名改为「已注销用户_<id>」，
/// 邮箱、密码、头像、装扮、签名、米游社绑定清除，所有设备下线；帖子、评论等内容保留。
#[utoipa::path(
    post,
    path = "/v1/me/delete-account",
    tag = "me",
    request_body = DeleteAccountRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已注销"),
        (status = 400, description = "CODE_INVALID / VALIDATION_FAILED / MIHOYO_DELETE_TICKET_EXPIRED / MIHOYO_DELETE_TICKET_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "MIHOYO_BIND_USER_MISMATCH / MIHOYO_DELETE_ACCOUNT_MISMATCH", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "ACCOUNT_CHANGED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "CODE_TOO_MANY_ATTEMPTS / RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn delete_account(
    _limit: AuthRateLimit,
    State(state): State<AuthState>,
    CurrentUser(principal): CurrentUser,
    jar: CookieJar,
    Json(request): Json<DeleteAccountRequest>,
) -> Result<(StatusCode, CookieJar), Problem> {
    service::delete_account(&state, &principal, request).await?;
    Ok((
        StatusCode::NO_CONTENT,
        ik_auth::forget_session(&state.settings, jar),
    ))
}
