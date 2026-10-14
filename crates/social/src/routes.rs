//! HTTP 接口：`/v1/users/{id}/follow`、`/v1/users/{id}/block`、`/v1/me/following`、
//! `/v1/me/blocks`、`/v1/me/relations`。

use axum::extract::{FromRef, State};
use ik_auth::{AuthState, CurrentUser};
use ik_kernel::Id;
use ik_kernel::Problem;
use ik_kernel::cursor::Page;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::id::marker::User;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::SocialState;
use crate::dto::{BlockResult, FollowResult, PageQuery, RelationView, RelationsQuery, UserCard};
use crate::service;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    SocialState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(follow, unfollow))
        .routes(routes!(block, unblock))
        .routes(routes!(following))
        .routes(routes!(blocks))
        .routes(routes!(relations))
}

/// 关注
///
/// 幂等。需要通过入站考试；与对方存在拉黑关系时不能关注。
#[utoipa::path(
    put,
    path = "/v1/users/{id}/follow",
    tag = "social",
    params(("id" = String, Path, description = "用户 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = FollowResult),
        (status = 400, description = "FOLLOW_SELF", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "EXAM_REQUIRED / USER_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "用户不存在或已注销", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn follow(
    State(state): State<SocialState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<User>>,
) -> Result<Json<FollowResult>, Problem> {
    Ok(Json(service::follow(&state, &principal, id.get()).await?))
}

/// 取消关注
///
/// 幂等。
#[utoipa::path(
    delete,
    path = "/v1/users/{id}/follow",
    tag = "social",
    params(("id" = String, Path, description = "用户 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = FollowResult),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unfollow(
    State(state): State<SocialState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<User>>,
) -> Result<Json<FollowResult>, Problem> {
    Ok(Json(service::unfollow(&state, &principal, id.get()).await?))
}

/// 拉黑
///
/// 幂等。不要求通过考试。同时双向取消关注（取消拉黑不会恢复）。
/// 拉黑与取消拉黑合计每分钟 10 次、每天 100 次。
#[utoipa::path(
    put,
    path = "/v1/users/{id}/block",
    tag = "social",
    params(("id" = String, Path, description = "用户 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = BlockResult),
        (status = 400, description = "BLOCK_SELF / BLOCK_SYSTEM_ACCOUNT", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "用户不存在或已注销", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn block(
    State(state): State<SocialState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<User>>,
) -> Result<Json<BlockResult>, Problem> {
    Ok(Json(service::block(&state, &principal, id.get()).await?))
}

/// 取消拉黑
///
/// 幂等。
#[utoipa::path(
    delete,
    path = "/v1/users/{id}/block",
    tag = "social",
    params(("id" = String, Path, description = "用户 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = BlockResult),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unblock(
    State(state): State<SocialState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<User>>,
) -> Result<Json<BlockResult>, Problem> {
    Ok(Json(service::unblock(&state, &principal, id.get()).await?))
}

/// 我关注的人（新关注的在前）
#[utoipa::path(
    get,
    path = "/v1/me/following",
    tag = "social",
    params(PageQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Page<UserCard>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn following(
    State(state): State<SocialState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<UserCard>>, Problem> {
    Ok(Json(service::following(&state, &principal, query).await?))
}

/// 我拉黑的人（新拉黑的在前）
#[utoipa::path(
    get,
    path = "/v1/me/blocks",
    tag = "social",
    params(PageQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Page<UserCard>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn blocks(
    State(state): State<SocialState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<UserCard>>, Problem> {
    Ok(Json(service::blocks(&state, &principal, query).await?))
}

/// 我与一批用户的关系
///
/// 返回是否关注、是否拉黑了对方。不反映「对方是否拉黑了我」。
#[utoipa::path(
    get,
    path = "/v1/me/relations",
    tag = "social",
    params(RelationsQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Vec<RelationView>),
        (status = 400, description = "VALIDATION_FAILED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn relations(
    State(state): State<SocialState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<RelationsQuery>,
) -> Result<Json<Vec<RelationView>>, Problem> {
    Ok(Json(
        service::relations(&state, &principal, &query.user_ids).await?,
    ))
}
