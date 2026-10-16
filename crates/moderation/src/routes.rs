//! HTTP 接口：`/v1/reports*`、`/v1/me/reports`、`/v1/admin/reports*`、`/v1/admin/users/{id}/block`、
//! `/v1/admin/users/{id}/unblock`。

use axum::extract::{FromRef, State};
use http::StatusCode;
use ik_auth::{AuthState, CurrentUser, StaffOperator};
use ik_kernel::cursor::Page;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::id::marker::User;
use ik_kernel::{Id, Problem};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::ModerationState;
use crate::dto::{
    BlockRequest, BlockResult, CheckQuery, CheckResult, ClaimResult, CreateReportRequest,
    GroupPage, GroupQuery, MyReport, PageQuery, ReportCreated, ReportGroup, ResolveRequest,
    ResolveResult, TargetType, UnblockResult, UnclaimResult,
};
use crate::{admin, service};

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    ModerationState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(create_report))
        .routes(routes!(check))
        .routes(routes!(my_reports))
        .routes(routes!(list_groups))
        .routes(routes!(group_detail))
        .routes(routes!(claim))
        .routes(routes!(unclaim))
        .routes(routes!(resolve))
        .routes(routes!(unblock))
        .routes(routes!(block))
}

/// 举报
///
/// 帖子、评论（必须公开可见）或用户（不能是已注销的）。不要求通过考试。不能举报自己；
/// 同一目标有处理中的举报时 409。每人每分钟 3 次、每天 20 次。
#[utoipa::path(
    post,
    path = "/v1/reports",
    tag = "moderation",
    request_body = CreateReportRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = ReportCreated),
        (status = 400, description = "VALIDATION_FAILED（说明缺失或过长、举报自己）", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "目标不存在或不可见", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "REPORT_DUPLICATE", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn create_report(
    State(state): State<ModerationState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<CreateReportRequest>,
) -> Result<(StatusCode, Json<ReportCreated>), Problem> {
    let created = service::report(&state, &principal, request).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// 是否已举报
///
/// 我对这些目标是否有处理中的举报（已结案的可以再举报）。
#[utoipa::path(
    get,
    path = "/v1/reports/check",
    tag = "moderation",
    params(CheckQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = CheckResult),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn check(
    State(state): State<ModerationState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<CheckQuery>,
) -> Result<Json<CheckResult>, Problem> {
    Ok(Json(service::check(&state, &principal, query).await?))
}

/// 我的举报
#[utoipa::path(
    get,
    path = "/v1/me/reports",
    tag = "moderation",
    params(PageQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Page<MyReport>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn my_reports(
    State(state): State<ModerationState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<MyReport>>, Problem> {
    Ok(Json(service::mine(&state, &principal, query).await?))
}

/// 举报列表（审核）
///
/// 按目标聚合，组按最新举报时间倒序。需要版主或管理员。
#[utoipa::path(
    get,
    path = "/v1/admin/reports",
    tag = "moderation",
    params(GroupQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = GroupPage),
        (status = 403, description = "FORBIDDEN", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn list_groups(
    State(state): State<ModerationState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<GroupQuery>,
) -> Result<Json<GroupPage>, Problem> {
    Ok(Json(admin::list(&state, &principal, query).await?))
}

/// 举报详情（审核）
///
/// 一个目标的全部举报（各种状态）。
#[utoipa::path(
    get,
    path = "/v1/admin/reports/{targetType}/{targetId}",
    tag = "moderation",
    params(
        ("targetType" = TargetType, Path, description = "post / comment / user"),
        ("targetId" = String, Path, description = "目标 id"),
    ),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ReportGroup),
        (status = 403, description = "FORBIDDEN", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "这个目标没有举报", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn group_detail(
    State(state): State<ModerationState>,
    CurrentUser(principal): CurrentUser,
    Path((target_type, target_id)): Path<(TargetType, Id)>,
) -> Result<Json<ReportGroup>, Problem> {
    Ok(Json(
        admin::detail(&state, &principal, target_type, target_id.get()).await?,
    ))
}

/// 领取举报（审核）
///
/// 整组领取。组里有被别人领取的举报时 409；重复领取自己的是幂等的。
#[utoipa::path(
    post,
    path = "/v1/admin/reports/{targetType}/{targetId}/claim",
    tag = "moderation",
    params(
        ("targetType" = TargetType, Path, description = "post / comment / user"),
        ("targetId" = String, Path, description = "目标 id"),
    ),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ClaimResult),
        (status = 403, description = "FORBIDDEN", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "REPORT_CLOSED / REPORT_ALREADY_CLAIMED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn claim(
    State(state): State<ModerationState>,
    CurrentUser(principal): CurrentUser,
    Path((target_type, target_id)): Path<(TargetType, Id)>,
) -> Result<Json<ClaimResult>, Problem> {
    Ok(Json(
        admin::claim(&state, &principal, target_type, target_id.get()).await?,
    ))
}

/// 释放举报（审核）
///
/// 自己领取的改回待处理。
#[utoipa::path(
    post,
    path = "/v1/admin/reports/{targetType}/{targetId}/unclaim",
    tag = "moderation",
    params(
        ("targetType" = TargetType, Path, description = "post / comment / user"),
        ("targetId" = String, Path, description = "目标 id"),
    ),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = UnclaimResult),
        (status = 403, description = "FORBIDDEN", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unclaim(
    State(state): State<ModerationState>,
    CurrentUser(principal): CurrentUser,
    Path((target_type, target_id)): Path<(TargetType, Id)>,
) -> Result<Json<UnclaimResult>, Problem> {
    Ok(Json(
        admin::unclaim(&state, &principal, target_type, target_id.get()).await?,
    ))
}

/// 结案（审核）
///
/// 同一目标的未结案举报一起结案：驳回、隐藏内容、删除内容、警告、封禁。被别人领取的只有领取人能结案。
/// 对用户的举报不能用内容处理动作；不能封禁版主与管理员。
#[utoipa::path(
    post,
    path = "/v1/admin/reports/{targetType}/{targetId}/resolve",
    tag = "moderation",
    params(
        ("targetType" = TargetType, Path, description = "post / comment / user"),
        ("targetId" = String, Path, description = "目标 id"),
    ),
    request_body = ResolveRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ResolveResult),
        (status = 400, description = "VALIDATION_FAILED（动作与目标不符）", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "FORBIDDEN / CANNOT_SANCTION_STAFF", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "REPORT_CLOSED / REPORT_ALREADY_CLAIMED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn resolve(
    State(state): State<ModerationState>,
    CurrentUser(principal): CurrentUser,
    Path((target_type, target_id)): Path<(TargetType, Id)>,
    Json(request): Json<ResolveRequest>,
) -> Result<Json<ResolveResult>, Problem> {
    Ok(Json(
        admin::resolve(&state, &principal, target_type, target_id.get(), request).await?,
    ))
}

/// 解封用户（审核）
#[utoipa::path(
    post,
    path = "/v1/admin/users/{id}/unblock",
    tag = "moderation",
    params(("id" = String, Path, description = "用户 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = UnblockResult),
        (status = 403, description = "FORBIDDEN", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unblock(
    State(state): State<ModerationState>,
    StaffOperator(op): StaffOperator,
    Path(user_id): Path<Id<User>>,
) -> Result<Json<UnblockResult>, Problem> {
    Ok(Json(admin::unblock(&state, &op, user_id.get()).await?))
}

/// 封禁用户（审核，不经举报）
///
/// 立即下线；不能封禁版主与管理员（403 `CANNOT_SANCTION_STAFF`）。已封禁时 `blocked: false`。
#[utoipa::path(
    post,
    path = "/v1/admin/users/{id}/block",
    tag = "moderation",
    params(("id" = String, Path, description = "用户 id")),
    request_body = BlockRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = BlockResult),
        (status = 403, description = "FORBIDDEN / CANNOT_SANCTION_STAFF", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn block(
    State(state): State<ModerationState>,
    StaffOperator(op): StaffOperator,
    Path(user_id): Path<Id<User>>,
    Json(request): Json<BlockRequest>,
) -> Result<Json<BlockResult>, Problem> {
    Ok(Json(
        admin::block(&state, &op, user_id.get(), request).await?,
    ))
}
