//! HTTP 接口：`/v1/posts/{id}/comments`、`/v1/comments/*`。

use axum::extract::{FromRef, State};
use http::StatusCode;
use ik_auth::{AuthState, CurrentUser, MaybeUser};
use ik_kernel::cursor::Page;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::id::marker::{Comment, Post};
use ik_kernel::{Id, Problem};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::CommentsState;
use crate::dto::{
    CommentLikeResult, CommentView, CommentsPage, CommentsQuery, CreateCommentRequest, PageQuery,
};
use crate::service;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    CommentsState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(list_comments, create_comment))
        .routes(routes!(delete_comment))
        .routes(routes!(list_replies))
        .routes(routes!(pin, unpin))
        .routes(routes!(accept, unaccept))
        .routes(routes!(like, unlike))
        .merge(crate::admin::router())
}

/// 帖子的评论
///
/// 所有评论按楼层从旧到新排成一条线（回复也是普通的一层，带上被回复评论的摘要）。
/// 第一页额外返回采纳与置顶；`anchor` 从某条评论所在的楼层开始。
#[utoipa::path(
    get,
    path = "/v1/posts/{id}/comments",
    tag = "comments",
    params(("id" = String, Path, description = "帖子 id"), CommentsQuery),
    responses(
        (status = 200, body = CommentsPage),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "帖子不存在或不可见", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn list_comments(
    State(state): State<CommentsState>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<Id<Post>>,
    Query(query): Query<CommentsQuery>,
) -> Result<Json<CommentsPage>, Problem> {
    Ok(Json(
        service::list(&state, viewer.as_ref(), id.get(), query).await?,
    ))
}

/// 发表评论（可以回复本帖下任意一条评论）
#[utoipa::path(
    post,
    path = "/v1/posts/{id}/comments",
    tag = "comments",
    params(("id" = String, Path, description = "帖子 id")),
    request_body = CreateCommentRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = CommentView),
        (status = 400, description = "VALIDATION_FAILED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "EXAM_REQUIRED / USER_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "帖子不存在或不公开", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn create_comment(
    State(state): State<CommentsState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Post>>,
    Json(request): Json<CreateCommentRequest>,
) -> Result<(StatusCode, Json<CommentView>), Problem> {
    let view = service::create(&state, &principal, id.get(), request).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// 删除自己的评论（回复它的评论保留，显示「原评论已删除」）
#[utoipa::path(
    delete,
    path = "/v1/comments/{id}",
    tag = "comments",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已删除"),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn delete_comment(
    State(state): State<CommentsState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Comment>>,
) -> Result<StatusCode, Problem> {
    service::delete(&state, &principal, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 回复某条评论的评论（旧的在前）
#[utoipa::path(
    get,
    path = "/v1/comments/{id}/replies",
    tag = "comments",
    params(("id" = String, Path), PageQuery),
    responses(
        (status = 200, body = Page<CommentView>),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn list_replies(
    State(state): State<CommentsState>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<Id<Comment>>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<CommentView>>, Problem> {
    Ok(Json(
        service::replies(&state, viewer.as_ref(), id.get(), query).await?,
    ))
}

/// 置顶评论（帖主或管理员）
#[utoipa::path(
    put,
    path = "/v1/comments/{id}/pin",
    tag = "comments",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已置顶（会替换本帖原来的置顶）"),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn pin(
    State(state): State<CommentsState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Comment>>,
) -> Result<StatusCode, Problem> {
    service::pin(&state, &principal, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 取消置顶（帖主或管理员）
#[utoipa::path(
    delete,
    path = "/v1/comments/{id}/pin",
    tag = "comments",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已取消"),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unpin(
    State(state): State<CommentsState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Comment>>,
) -> Result<StatusCode, Problem> {
    service::unpin(&state, &principal, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 采纳为最佳答案（帖主或管理员，分区需开启采纳）
#[utoipa::path(
    put,
    path = "/v1/comments/{id}/accept",
    tag = "comments",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已采纳（会替换本帖原来的采纳）"),
        (status = 403, description = "FORBIDDEN / ACCEPT_DISABLED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn accept(
    State(state): State<CommentsState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Comment>>,
) -> Result<StatusCode, Problem> {
    service::accept(&state, &principal, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 取消采纳（帖主或管理员）
#[utoipa::path(
    delete,
    path = "/v1/comments/{id}/accept",
    tag = "comments",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已取消"),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unaccept(
    State(state): State<CommentsState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Comment>>,
) -> Result<StatusCode, Problem> {
    service::unaccept(&state, &principal, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 给评论点赞
///
/// 幂等。需要通过入站考试。
#[utoipa::path(
    put,
    path = "/v1/comments/{id}/like",
    tag = "comments",
    params(("id" = String, Path, description = "评论 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = CommentLikeResult),
        (status = 403, description = "EXAM_REQUIRED / USER_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "评论不存在或不可见", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn like(
    State(state): State<CommentsState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Comment>>,
) -> Result<Json<CommentLikeResult>, Problem> {
    Ok(Json(service::like(&state, &principal, id.get()).await?))
}

/// 取消评论点赞
///
/// 幂等。评论已不可见时也能取消。
#[utoipa::path(
    delete,
    path = "/v1/comments/{id}/like",
    tag = "comments",
    params(("id" = String, Path, description = "评论 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = CommentLikeResult),
        (status = 404, description = "评论不存在", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unlike(
    State(state): State<CommentsState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Comment>>,
) -> Result<Json<CommentLikeResult>, Problem> {
    Ok(Json(service::unlike(&state, &principal, id.get()).await?))
}
