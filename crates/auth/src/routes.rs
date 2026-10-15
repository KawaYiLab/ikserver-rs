//! HTTP 接口：`/v1/auth/*` 与 `/v1/me`、`/v1/me/sessions*`。

use axum::extract::{FromRef, Path, State};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::Utc;
use http::StatusCode;
use ik_kernel::Problem;
use ik_kernel::cursor::Page;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::Json;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::dto::{
    AuthResponse, ClientKind, CodeLoginRequest, MeView, PasswordLoginRequest, RegisterRequest,
    ResetPasswordRequest, SendCodeRequest, SendCodeResponse, SessionView,
};
use crate::extract::{AuthRateLimit, ClientMeta, CurrentUser};
use crate::service::{self, Issued};
use crate::{AuthSettings, AuthState};

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(send_code))
        .routes(routes!(register))
        .routes(routes!(login_password))
        .routes(routes!(login_email_code))
        .routes(routes!(reset_password))
        .routes(routes!(logout))
        .routes(routes!(me))
        .routes(routes!(list_sessions))
        .routes(routes!(revoke_session))
        .routes(routes!(revoke_other_sessions))
}

fn session_cookie(settings: &AuthSettings, token: String, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build((settings.cookie_name.clone(), token))
        .http_only(true)
        .secure(settings.cookie_secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(cookie::time::Duration::seconds(max_age_secs.max(0)))
        .build()
}

fn removal_cookie(settings: &AuthSettings) -> Cookie<'static> {
    Cookie::build(settings.cookie_name.clone())
        .path("/")
        .build()
}

/// 按客户端类型交付令牌：网页写 HttpOnly Cookie，App 放进响应体。其他登录方式（米游社扫码）也用它。
pub fn deliver(
    settings: &AuthSettings,
    jar: CookieJar,
    issued: Issued,
    client: ClientKind,
) -> (CookieJar, AuthResponse) {
    let response = |token| AuthResponse {
        user: issued.user.clone(),
        session: issued.session.clone(),
        token,
    };
    match client {
        ClientKind::App => (jar, response(Some(issued.token.clone()))),
        ClientKind::Web => {
            let max_age = (issued.session.expires_at - Utc::now()).num_seconds();
            let jar = jar.add(session_cookie(settings, issued.token.clone(), max_age));
            (jar, response(None))
        }
    }
}

/// 发送邮箱验证码
///
/// 注册用途：邮箱已注册返回 409，冷却中返回 429。登录与重置密码用途：
/// 无论邮箱是否注册、是否在冷却中，都返回相同的 202 结果。
#[utoipa::path(
    post,
    path = "/v1/auth/email-codes",
    tag = "auth",
    request_body = SendCodeRequest,
    responses(
        (status = 202, description = "已受理", body = SendCodeResponse),
        (status = 400, description = "参数不合法", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "REGISTRATION_CLOSED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "EMAIL_TAKEN", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "CODE_COOLDOWN / RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "MAIL_SEND_FAILED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn send_code(
    _limit: AuthRateLimit,
    State(state): State<AuthState>,
    ClientMeta(meta): ClientMeta,
    Json(request): Json<SendCodeRequest>,
) -> Result<(StatusCode, Json<SendCodeResponse>), Problem> {
    let response = service::send_code(&state, request, &meta).await?;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

/// 邮箱验证码注册（成功即登录）
#[utoipa::path(
    post,
    path = "/v1/auth/register",
    tag = "auth",
    request_body = RegisterRequest,
    responses(
        (status = 201, description = "注册成功并已登录", body = AuthResponse),
        (status = 400, description = "参数不合法 / CODE_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "REGISTRATION_CLOSED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "EMAIL_TAKEN", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "CODE_TOO_MANY_ATTEMPTS / RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn register(
    _limit: AuthRateLimit,
    State(state): State<AuthState>,
    ClientMeta(meta): ClientMeta,
    jar: CookieJar,
    Json(request): Json<RegisterRequest>,
) -> Result<(StatusCode, CookieJar, Json<AuthResponse>), Problem> {
    let client = request.client;
    let issued = service::register(&state, request, &meta).await?;
    let (jar, body) = deliver(&state.settings, jar, issued, client);
    Ok((StatusCode::CREATED, jar, Json(body)))
}

/// 密码登录（邮箱或用户名）
#[utoipa::path(
    post,
    path = "/v1/auth/login",
    tag = "auth",
    request_body = PasswordLoginRequest,
    responses(
        (status = 200, description = "登录成功", body = AuthResponse),
        (status = 401, description = "INVALID_CREDENTIALS", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "ACCOUNT_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn login_password(
    _limit: AuthRateLimit,
    State(state): State<AuthState>,
    ClientMeta(meta): ClientMeta,
    jar: CookieJar,
    Json(request): Json<PasswordLoginRequest>,
) -> Result<(CookieJar, Json<AuthResponse>), Problem> {
    let client = request.client;
    let issued = service::login_password(&state, request, &meta).await?;
    let (jar, body) = deliver(&state.settings, jar, issued, client);
    Ok((jar, Json(body)))
}

/// 邮箱验证码登录
#[utoipa::path(
    post,
    path = "/v1/auth/login/email-code",
    tag = "auth",
    request_body = CodeLoginRequest,
    responses(
        (status = 200, description = "登录成功", body = AuthResponse),
        (status = 400, description = "CODE_INVALID（账号不存在也返回此错误）", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "CODE_TOO_MANY_ATTEMPTS / RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn login_email_code(
    _limit: AuthRateLimit,
    State(state): State<AuthState>,
    ClientMeta(meta): ClientMeta,
    jar: CookieJar,
    Json(request): Json<CodeLoginRequest>,
) -> Result<(CookieJar, Json<AuthResponse>), Problem> {
    let client = request.client;
    let issued = service::login_email_code(&state, request, &meta).await?;
    let (jar, body) = deliver(&state.settings, jar, issued, client);
    Ok((jar, Json(body)))
}

/// 用邮箱验证码重置密码（成功后所有设备下线，需重新登录）
#[utoipa::path(
    post,
    path = "/v1/auth/password/reset",
    tag = "auth",
    request_body = ResetPasswordRequest,
    responses(
        (status = 204, description = "已重置"),
        (status = 400, description = "参数不合法 / CODE_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "CODE_TOO_MANY_ATTEMPTS / RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn reset_password(
    _limit: AuthRateLimit,
    State(state): State<AuthState>,
    Json(request): Json<ResetPasswordRequest>,
) -> Result<StatusCode, Problem> {
    service::reset_password(&state, request).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 退出登录（撤销当前会话）
#[utoipa::path(
    post,
    path = "/v1/auth/logout",
    tag = "auth",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已退出"),
        (status = 401, description = "UNAUTHORIZED / SESSION_EXPIRED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn logout(
    State(state): State<AuthState>,
    CurrentUser(principal): CurrentUser,
    jar: CookieJar,
) -> Result<(StatusCode, CookieJar), Problem> {
    service::logout(&state, &principal).await?;
    Ok((
        StatusCode::NO_CONTENT,
        jar.remove(removal_cookie(&state.settings)),
    ))
}

/// 当前登录用户
#[utoipa::path(
    get,
    path = "/v1/me",
    tag = "me",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "当前用户", body = MeView),
        (status = 401, description = "UNAUTHORIZED / SESSION_EXPIRED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "ACCOUNT_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn me(
    State(state): State<AuthState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<MeView>, Problem> {
    Ok(Json(service::me(&state, &principal).await?))
}

/// 已登录设备（活跃会话）
#[utoipa::path(
    get,
    path = "/v1/me/sessions",
    tag = "me",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "按最近活跃排序，最多 100 条", body = Page<SessionView>),
        (status = 401, description = "UNAUTHORIZED / SESSION_EXPIRED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn list_sessions(
    State(state): State<AuthState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<Page<SessionView>>, Problem> {
    let items = service::list_sessions(&state, &principal).await?;
    Ok(Json(Page {
        items,
        next_cursor: None,
    }))
}

/// 下线某个设备
#[utoipa::path(
    delete,
    path = "/v1/me/sessions/{id}",
    tag = "me",
    security(("session_cookie" = []), ("bearer" = [])),
    params(("id" = String, Path, description = "会话 ID")),
    responses(
        (status = 204, description = "已下线（若是当前会话，同时清除 Cookie）"),
        (status = 401, description = "UNAUTHORIZED / SESSION_EXPIRED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "不存在或不属于你", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn revoke_session(
    State(state): State<AuthState>,
    CurrentUser(principal): CurrentUser,
    jar: CookieJar,
    Path(id): Path<String>,
) -> Result<(StatusCode, CookieJar), Problem> {
    let session_id: ik_kernel::Id<ik_kernel::id::marker::Session> =
        id.parse().map_err(|_| Problem::not_found())?;
    let was_current = service::revoke_session(&state, &principal, session_id.get()).await?;
    let jar = if was_current {
        jar.remove(removal_cookie(&state.settings))
    } else {
        jar
    };
    Ok((StatusCode::NO_CONTENT, jar))
}

/// 下线除当前设备外的所有设备
#[utoipa::path(
    post,
    path = "/v1/me/sessions/revoke-others",
    tag = "me",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已下线"),
        (status = 401, description = "UNAUTHORIZED / SESSION_EXPIRED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn revoke_other_sessions(
    State(state): State<AuthState>,
    CurrentUser(principal): CurrentUser,
) -> Result<StatusCode, Problem> {
    service::revoke_other_sessions(&state, &principal).await?;
    Ok(StatusCode::NO_CONTENT)
}
