//! HTTP 接口：`/v1/notifications*`。

use axum::extract::{FromRef, State};
use ik_auth::{AuthState, CurrentUser};
use ik_kernel::cursor::Page;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::id::marker::Notification;
use ik_kernel::{Id, Problem};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::NotifyState;
use crate::dto::{BucketView, ListQuery, NotificationView, PageQuery, ReadResult, UnreadCount};
use crate::service;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    NotifyState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(unread_count))
        .routes(routes!(mark_read))
        .routes(routes!(mark_all_read))
        .routes(routes!(buckets))
        .routes(routes!(bucket_items))
        .routes(routes!(mark_bucket_read))
}

/// 我的通知（新的在前）
#[utoipa::path(
    get,
    path = "/v1/notifications",
    tag = "notify",
    params(ListQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Page<NotificationView>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn list(
    State(state): State<NotifyState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<NotificationView>>, Problem> {
    Ok(Json(service::list(&state, &principal, query).await?))
}

/// 未读通知数
#[utoipa::path(
    get,
    path = "/v1/notifications/unread-count",
    tag = "notify",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = UnreadCount),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unread_count(
    State(state): State<NotifyState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<UnreadCount>, Problem> {
    Ok(Json(service::unread_count(&state, &principal).await?))
}

/// 标记一条为已读
///
/// 幂等。
#[utoipa::path(
    put,
    path = "/v1/notifications/{id}/read",
    tag = "notify",
    params(("id" = String, Path, description = "通知 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ReadResult),
        (status = 404, description = "通知不存在或不是自己的", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn mark_read(
    State(state): State<NotifyState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Notification>>,
) -> Result<Json<ReadResult>, Problem> {
    Ok(Json(
        service::mark_read(&state, &principal, id.get()).await?,
    ))
}

/// 全部标为已读
#[utoipa::path(
    post,
    path = "/v1/notifications/read-all",
    tag = "notify",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ReadResult),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn mark_all_read(
    State(state): State<NotifyState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<ReadResult>, Problem> {
    Ok(Json(service::mark_all_read(&state, &principal).await?))
}

/// 敲敲会话列表
///
/// 同一个人发来的通知归为一组，系统通知（对端名为 Fairy）一组，按最后一条的时间倒序。
#[utoipa::path(
    get,
    path = "/v1/notifications/buckets",
    tag = "notify",
    params(PageQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Page<BucketView>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn buckets(
    State(state): State<NotifyState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<BucketView>>, Problem> {
    Ok(Json(service::buckets(&state, &principal, query).await?))
}

/// 某个敲敲会话里的通知（新的在前）
#[utoipa::path(
    get,
    path = "/v1/notifications/buckets/{key}",
    tag = "notify",
    params(("key" = String, Path, description = "`user-<id>` 或 `system`"), PageQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Page<NotificationView>),
        (status = 404, description = "会话键格式不对", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn bucket_items(
    State(state): State<NotifyState>,
    CurrentUser(principal): CurrentUser,
    Path(key): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<NotificationView>>, Problem> {
    Ok(Json(
        service::bucket_items(&state, &principal, &key, query).await?,
    ))
}

/// 把某个敲敲会话标为已读
#[utoipa::path(
    post,
    path = "/v1/notifications/buckets/{key}/read",
    tag = "notify",
    params(("key" = String, Path, description = "`user-<id>` 或 `system`")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ReadResult),
        (status = 404, description = "会话键格式不对", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn mark_bucket_read(
    State(state): State<NotifyState>,
    CurrentUser(principal): CurrentUser,
    Path(key): Path<String>,
) -> Result<Json<ReadResult>, Problem> {
    Ok(Json(
        service::mark_bucket_read(&state, &principal, &key).await?,
    ))
}
