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
use crate::dto::{CommentView, CommentsPage, CreateCommentRequest, PageQuery};
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
}

/// 帖子的评论
///
/// 一级评论新的在前，每条带前 3 条回复。第一页额外返回采纳与置顶。
#[utoipa::path(
    get,
    path = "/v1/posts/{id}/comments",
    tag = "comments",
    params(("id" = String, Path, description = "帖子 id"), PageQuery),
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
    Query(query): Query<PageQuery>,
) -> Result<Json<CommentsPage>, Problem> {
    Ok(Json(
        service::list(&state, viewer.as_ref(), id.get(), query).await?,
    ))
}

/// 发表评论或回复
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
        (status = 403, description = "EXAM_REQUIRED", body = ProblemBody, content_type = "application/problem+json"),
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

/// 删除自己的评论（一级评论连同回复一起删除）
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

/// 一级评论的回复（旧的在前）
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
        (status = 400, description = "COMMENT_NOT_TOP_LEVEL", body = ProblemBody, content_type = "application/problem+json"),
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
        (status = 400, description = "COMMENT_NOT_TOP_LEVEL", body = ProblemBody, content_type = "application/problem+json"),
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
