//! HTTP 接口：`/v1/feed`、`/v1/feed/hot`、`/v1/feed/following`、`/v1/feed/unread`、`/v1/me/reads`。

use axum::extract::{FromRef, State};
use http::header;
use ik_auth::{AuthState, CurrentUser, MaybeUser};
use ik_content::dto::{FeedQuery, HomeFeedPage, MarkReadRequest, MarkReadResult, PostCard};
use ik_kernel::Problem;
use ik_kernel::cursor::Page;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Query};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::FeedState;
use crate::service::{self, HotQuery};

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    FeedState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(home))
        .routes(routes!(hot))
        .routes(routes!(following))
        .routes(routes!(unread))
        .routes(routes!(mark_read))
}

/// 个性化结果（含已读、拉黑过滤）不能被共享缓存。
const PRIVATE: (header::HeaderName, &str) = (header::CACHE_CONTROL, "private, no-store");

/// 首页信息流
///
/// 第一页额外返回置顶（没按标签筛选时）与热门位（没按分区、标签筛选时，最多 4 篇），
/// 它们不会再出现在 `items` 里，翻页时也不会。`items` 按最近活跃倒序。
#[utoipa::path(
    get,
    path = "/v1/feed",
    tag = "feed",
    params(FeedQuery),
    responses(
        (status = 200, body = HomeFeedPage),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "分区或标签不存在", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn home(
    State(state): State<FeedState>,
    MaybeUser(viewer): MaybeUser,
    Query(query): Query<FeedQuery>,
) -> Result<([(header::HeaderName, &'static str); 1], Json<HomeFeedPage>), Problem> {
    Ok((
        [PRIVATE],
        Json(service::home(&state, viewer.as_ref(), query).await?),
    ))
}

/// 热门榜
///
/// 按热度排序（每 10 分钟刷新），不含置顶，同一作者不限篇数。
#[utoipa::path(
    get,
    path = "/v1/feed/hot",
    tag = "feed",
    params(HotQuery),
    responses(
        (status = 200, body = Page<PostCard>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn hot(
    State(state): State<FeedState>,
    MaybeUser(viewer): MaybeUser,
    Query(query): Query<HotQuery>,
) -> Result<
    (
        [(header::HeaderName, &'static str); 1],
        Json<Page<PostCard>>,
    ),
    Problem,
> {
    Ok((
        [PRIVATE],
        Json(service::hot(&state, viewer.as_ref(), query).await?),
    ))
}

/// 关注流
///
/// 关注的人的非匿名帖子，按最近活跃倒序。
#[utoipa::path(
    get,
    path = "/v1/feed/following",
    tag = "feed",
    params(FeedQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Page<PostCard>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn following(
    State(state): State<FeedState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<FeedQuery>,
) -> Result<
    (
        [(header::HeaderName, &'static str); 1],
        Json<Page<PostCard>>,
    ),
    Problem,
> {
    Ok((
        [PRIVATE],
        Json(service::following(&state, &principal, query).await?),
    ))
}

/// 未看流
///
/// 最新流去掉自己读过的帖子。
#[utoipa::path(
    get,
    path = "/v1/feed/unread",
    tag = "feed",
    params(FeedQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Page<PostCard>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unread(
    State(state): State<FeedState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<FeedQuery>,
) -> Result<
    (
        [(header::HeaderName, &'static str); 1],
        Json<Page<PostCard>>,
    ),
    Problem,
> {
    Ok((
        [PRIVATE],
        Json(service::unread(&state, &principal, query).await?),
    ))
}

/// 标为已读
///
/// 一次最多 100 篇；有不存在的帖子时整批拒绝，不写入。
#[utoipa::path(
    post,
    path = "/v1/me/reads",
    tag = "feed",
    request_body = MarkReadRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = MarkReadResult),
        (status = 400, description = "VALIDATION_FAILED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "有帖子不存在", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn mark_read(
    State(state): State<FeedState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<MarkReadRequest>,
) -> Result<Json<MarkReadResult>, Problem> {
    Ok(Json(service::mark_read(&state, &principal, request).await?))
}
