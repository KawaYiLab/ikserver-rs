//! HTTP 接口：`/v1/admin/users*`（含流水、登录设备、强制下线、强制改名、清理资料）、`/v1/admin/dashboard`、`/v1/admin/search*`、`/v1/admin/audit`。

use axum::extract::{FromRef, State};
use http::StatusCode;
use ik_auth::admin::{AdminUser, AuditEntry, AuditQuery, UserQuery};
use ik_auth::{AdminOperator, AuthState, StaffOperator};
use ik_kernel::cursor::Paged;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::id::marker::User;
use ik_kernel::{Id, Problem};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AdminState;
use crate::dashboard::{self, Dashboard, SearchStatus, TrendQuery, Trends};
use crate::users::{
    self, AdminUserDetail, AiAgentUpdate, DennyAdjustment, EconomyResult, ExpGrant, LedgerQuery,
    ProfileReset, ProfileResetResult, RevokeResult, RoleUpdate, UsernameUpdate,
};
use ik_auth::dto::SessionView;
use ik_economy::dto::{DennyLogView, ExpLogView};

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    AdminState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(list_users))
        .routes(routes!(user_detail))
        .routes(routes!(put_role))
        .routes(routes!(put_ai_agent))
        .routes(routes!(pass_exam))
        .routes(routes!(adjust_denny))
        .routes(routes!(grant_exp))
        .routes(routes!(user_denny_logs))
        .routes(routes!(user_exp_logs))
        .routes(routes!(user_sessions))
        .routes(routes!(revoke_sessions))
        .routes(routes!(put_username))
        .routes(routes!(reset_profile))
        .routes(routes!(get_dashboard))
        .routes(routes!(get_trends))
        .routes(routes!(get_search))
        .routes(routes!(reindex))
        .routes(routes!(list_audit))
}

/// 账号列表
///
/// 关键词匹配用户名或邮箱（数字时也按 id、会员号）；可按角色、状态、是否 AI 账号筛选。新注册的在前。
/// 邮箱只有管理员看到全文。
#[utoipa::path(
    get,
    path = "/v1/admin/users",
    tag = "admin",
    params(UserQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Paged<AdminUser>),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn list_users(
    State(state): State<AdminState>,
    StaffOperator(op): StaffOperator,
    Query(query): Query<UserQuery>,
) -> Result<Json<Paged<AdminUser>>, Problem> {
    Ok(Json(users::list(&state, &op, query).await?))
}

/// 账号详情（含丁尼、经验与发帖统计）
#[utoipa::path(
    get,
    path = "/v1/admin/users/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminUserDetail),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn user_detail(
    State(state): State<AdminState>,
    StaffOperator(op): StaffOperator,
    Path(id): Path<Id<User>>,
) -> Result<Json<AdminUserDetail>, Problem> {
    Ok(Json(users::detail(&state, &op, id.get()).await?))
}

/// 修改角色（只有管理员；不能改自己）
#[utoipa::path(
    put,
    path = "/v1/admin/users/{id}/role",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = RoleUpdate,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminUserDetail),
        (status = 400, description = "CANNOT_CHANGE_SELF", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_role(
    State(state): State<AdminState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id<User>>,
    Json(update): Json<RoleUpdate>,
) -> Result<Json<AdminUserDetail>, Problem> {
    Ok(Json(users::set_role(&state, &op, id.get(), update).await?))
}

/// 标记 / 取消 AI 账号（只有管理员）
#[utoipa::path(
    put,
    path = "/v1/admin/users/{id}/ai-agent",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = AiAgentUpdate,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminUserDetail),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_ai_agent(
    State(state): State<AdminState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id<User>>,
    Json(update): Json<AiAgentUpdate>,
) -> Result<Json<AdminUserDetail>, Problem> {
    Ok(Json(
        users::set_ai_agent(&state, &op, id.get(), update).await?,
    ))
}

/// 手动通过入站考试（只有管理员；不发首次通过奖励）
#[utoipa::path(
    post,
    path = "/v1/admin/users/{id}/exam-pass",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminUserDetail),
        (status = 409, description = "ACCOUNT_UNAVAILABLE", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn pass_exam(
    State(state): State<AdminState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id<User>>,
) -> Result<Json<AdminUserDetail>, Problem> {
    Ok(Json(users::pass_exam(&state, &op, id.get()).await?))
}

/// 调整丁尼（只有管理员）
///
/// 正数发放、负数扣除；余额封顶 9999，扣除时余额不足 402 `INSUFFICIENT_DENNY`。
#[utoipa::path(
    post,
    path = "/v1/admin/users/{id}/denny",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = DennyAdjustment,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = EconomyResult),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 402, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "ACCOUNT_UNAVAILABLE", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn adjust_denny(
    State(state): State<AdminState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id<User>>,
    Json(request): Json<DennyAdjustment>,
) -> Result<Json<EconomyResult>, Problem> {
    Ok(Json(
        users::adjust_denny(&state, &op, id.get(), request).await?,
    ))
}

/// 发放经验（只有管理员；不占每日上限）
#[utoipa::path(
    post,
    path = "/v1/admin/users/{id}/exp",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = ExpGrant,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = EconomyResult),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "ACCOUNT_UNAVAILABLE", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn grant_exp(
    State(state): State<AdminState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id<User>>,
    Json(request): Json<ExpGrant>,
) -> Result<Json<EconomyResult>, Problem> {
    Ok(Json(
        users::grant_exp(&state, &op, id.get(), request).await?,
    ))
}

/// 账号的丁尼流水（新的在前）
#[utoipa::path(
    get,
    path = "/v1/admin/users/{id}/denny-logs",
    tag = "admin",
    params(("id" = String, Path), LedgerQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = Paged<DennyLogView>))
)]
async fn user_denny_logs(
    State(state): State<AdminState>,
    StaffOperator(_): StaffOperator,
    Path(id): Path<Id<User>>,
    Query(query): Query<LedgerQuery>,
) -> Result<Json<Paged<DennyLogView>>, Problem> {
    Ok(Json(users::denny_logs(&state, id.get(), query).await?))
}

/// 账号的经验流水（新的在前）
#[utoipa::path(
    get,
    path = "/v1/admin/users/{id}/exp-logs",
    tag = "admin",
    params(("id" = String, Path), LedgerQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = Paged<ExpLogView>))
)]
async fn user_exp_logs(
    State(state): State<AdminState>,
    StaffOperator(_): StaffOperator,
    Path(id): Path<Id<User>>,
    Query(query): Query<LedgerQuery>,
) -> Result<Json<Paged<ExpLogView>>, Problem> {
    Ok(Json(users::exp_logs(&state, id.get(), query).await?))
}

/// 账号的登录设备（只有管理员：含 IP）
#[utoipa::path(
    get,
    path = "/v1/admin/users/{id}/sessions",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Vec<SessionView>),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn user_sessions(
    State(state): State<AdminState>,
    AdminOperator(_): AdminOperator,
    Path(id): Path<Id<User>>,
) -> Result<Json<Vec<SessionView>>, Problem> {
    Ok(Json(users::sessions(&state, id.get()).await?))
}

/// 强制下线（只有管理员）
///
/// 吊销全部会话，在线的连接随即断开。
#[utoipa::path(
    post,
    path = "/v1/admin/users/{id}/sessions/revoke",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = RevokeResult),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn revoke_sessions(
    State(state): State<AdminState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id<User>>,
) -> Result<Json<RevokeResult>, Problem> {
    Ok(Json(users::revoke_sessions(&state, &op, id.get()).await?))
}

/// 强制改名（只有管理员；不收费，封禁中的账号也能改）
#[utoipa::path(
    put,
    path = "/v1/admin/users/{id}/username",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = UsernameUpdate,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminUserDetail),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "USERNAME_TAKEN", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_username(
    State(state): State<AdminState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id<User>>,
    Json(update): Json<UsernameUpdate>,
) -> Result<Json<AdminUserDetail>, Problem> {
    Ok(Json(
        users::force_rename(&state, &op, id.get(), update).await?,
    ))
}

/// 清理违规资料：清空签名、清除自定义头像（审核）
///
/// 版主不能清理版主、管理员的资料（403 `CANNOT_SANCTION_STAFF`）。
#[utoipa::path(
    post,
    path = "/v1/admin/users/{id}/profile/reset",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = ProfileReset,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ProfileResetResult),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "CANNOT_SANCTION_STAFF", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn reset_profile(
    State(state): State<AdminState>,
    StaffOperator(op): StaffOperator,
    Path(id): Path<Id<User>>,
    Json(reset): Json<ProfileReset>,
) -> Result<Json<ProfileResetResult>, Problem> {
    Ok(Json(
        users::reset_profile(&state, &op, id.get(), reset).await?,
    ))
}

/// 仪表盘
///
/// 账号、帖子、评论（总数与今日）、待处理举报、题库与 AI 今日概况。
#[utoipa::path(
    get,
    path = "/v1/admin/dashboard",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Dashboard),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn get_dashboard(
    State(state): State<AdminState>,
    StaffOperator(op): StaffOperator,
) -> Result<Json<Dashboard>, Problem> {
    Ok(Json(dashboard::dashboard(&state, &op).await?))
}

/// 仪表盘趋势：最近 7 或 30 个业务日每天的新增账号、首次发布的帖子、评论、举报
#[utoipa::path(
    get,
    path = "/v1/admin/dashboard/trends",
    tag = "admin",
    params(TrendQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Trends),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn get_trends(
    State(state): State<AdminState>,
    StaffOperator(_): StaffOperator,
    Query(query): Query<TrendQuery>,
) -> Result<Json<Trends>, Problem> {
    Ok(Json(dashboard::trends(&state, query).await?))
}

/// 搜索索引状态（只有管理员）
///
/// 索引里的文档数与数据库行数对比（ADMINCRUD-007）。
#[utoipa::path(
    get,
    path = "/v1/admin/search",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = SearchStatus))
)]
async fn get_search(
    State(state): State<AdminState>,
    AdminOperator(_): AdminOperator,
) -> Result<Json<SearchStatus>, Problem> {
    Ok(Json(dashboard::search_status(&state).await?))
}

/// 重建搜索索引（只有管理员）
///
/// 在后台做一次全量对账：补齐缺的、删掉残留的。立即返回 202；正在重建时 409 `REINDEX_RUNNING`。
#[utoipa::path(
    post,
    path = "/v1/admin/search/reindex",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 202),
        (status = 409, description = "REINDEX_RUNNING / SEARCH_ENGINE_NOT_CONFIGURED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn reindex(
    State(state): State<AdminState>,
    AdminOperator(op): AdminOperator,
) -> Result<StatusCode, Problem> {
    dashboard::start_reindex(&state, &op).await?;
    Ok(StatusCode::ACCEPTED)
}

/// 审计日志（只有管理员）
///
/// 管理后台的全部写操作，新的在前；可按操作者、动作前缀、目标筛选。
#[utoipa::path(
    get,
    path = "/v1/admin/audit",
    tag = "admin",
    params(AuditQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = Paged<AuditEntry>))
)]
async fn list_audit(
    State(state): State<AdminState>,
    AdminOperator(_): AdminOperator,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Paged<AuditEntry>>, Problem> {
    let mut conn = state.db.acquire().await?;
    Ok(Json(ik_auth::admin::list(&mut conn, query).await?))
}
