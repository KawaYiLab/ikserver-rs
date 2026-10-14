//! HTTP 接口：`/v1/benefits`、`/v1/me/wallet`、`/v1/me/denny-logs`、`/v1/me/exp`、`/v1/me/check-in`。

use axum::extract::{FromRef, State};
use ik_auth::{AuthState, CurrentUser, MaybeUser};
use ik_kernel::Problem;
use ik_kernel::cursor::Page;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Query};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::EconomyState;
use crate::dto::{
    BenefitsView, CheckInResult, CheckInStatusView, DennyLogView, ExpView, PageQuery, WalletView,
};
use crate::service;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    EconomyState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(benefits))
        .routes(routes!(wallet))
        .routes(routes!(denny_logs))
        .routes(routes!(exp))
        .routes(routes!(check_in_status, check_in))
}

/// 等级权益
///
/// 当前用户等级对应的发帖、评论限额，以及下一级的限额。未登录按 0 级。
#[utoipa::path(
    get,
    path = "/v1/benefits",
    tag = "economy",
    responses((status = 200, body = BenefitsView))
)]
async fn benefits(MaybeUser(viewer): MaybeUser) -> Json<BenefitsView> {
    Json(service::benefits_for(viewer.as_ref()))
}

/// 我的丁尼余额
#[utoipa::path(
    get,
    path = "/v1/me/wallet",
    tag = "economy",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = WalletView),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn wallet(
    State(state): State<EconomyState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<WalletView>, Problem> {
    Ok(Json(service::wallet(&state, &principal).await?))
}

/// 我的丁尼流水（新的在前）
#[utoipa::path(
    get,
    path = "/v1/me/denny-logs",
    tag = "economy",
    params(PageQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Page<DennyLogView>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn denny_logs(
    State(state): State<EconomyState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<DennyLogView>>, Problem> {
    Ok(Json(service::denny_logs(&state, &principal, query).await?))
}

/// 我的经验与今日经验额度
#[utoipa::path(
    get,
    path = "/v1/me/exp",
    tag = "economy",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ExpView),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn exp(
    State(state): State<EconomyState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<ExpView>, Problem> {
    Ok(Json(service::exp(&state, &principal).await?))
}

/// 签到状态
#[utoipa::path(
    get,
    path = "/v1/me/check-in",
    tag = "economy",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = CheckInStatusView),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn check_in_status(
    State(state): State<EconomyState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<CheckInStatusView>, Problem> {
    Ok(Json(service::check_in_status(&state, &principal).await?))
}

/// 签到
///
/// 每个签到日（北京时间 04:00 切日）一次。奖励 6–10 经验（随连签天数增加）和 10 丁尼。
#[utoipa::path(
    post,
    path = "/v1/me/check-in",
    tag = "economy",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = CheckInResult),
        (status = 403, description = "EXAM_REQUIRED / ACCOUNT_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "CHECK_IN_ALREADY_TODAY", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn check_in(
    State(state): State<EconomyState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<CheckInResult>, Problem> {
    Ok(Json(service::check_in(&state, &principal).await?))
}
