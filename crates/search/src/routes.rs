//! HTTP 接口：`/v1/search/*`。用户搜索在 profile（`/v1/users/search`）。

use axum::extract::{FromRef, State};
use ik_auth::{AuthState, ClientMeta, MaybeUser};
use ik_comments::dto::CommentSearchView;
use ik_content::dto::PostCard;
use ik_kernel::Problem;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Query};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::SearchState;
use crate::dto::{CommentSearchQuery, PostSearchQuery, SearchPage, SuggestItem, SuggestQuery};
use crate::service;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    SearchState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(search_posts))
        .routes(routes!(suggest))
        .routes(routes!(search_comments))
}

/// 搜索帖子
///
/// 结果以数据库为准（可见性、匿名、作者名、拉黑过滤）。搜索引擎不可用或命中全是残留时改用数据库匹配
/// 标题与正文（查询词至少 2 个字，最多 1000 条）。限流：登录用户每秒 10 次，游客每 IP 每秒 5 次。
#[utoipa::path(
    get,
    path = "/v1/search/posts",
    tag = "search",
    params(PostSearchQuery),
    responses(
        (status = 200, body = SearchPage<PostCard>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn search_posts(
    State(state): State<SearchState>,
    MaybeUser(viewer): MaybeUser,
    ClientMeta(meta): ClientMeta,
    Query(query): Query<PostSearchQuery>,
) -> Result<Json<SearchPage<PostCard>>, Problem> {
    service::check_rate(&state, viewer.as_ref(), meta.ip)?;
    Ok(Json(
        service::search_posts(&state, viewer.as_ref(), query).await?,
    ))
}

/// 搜索联想（帖子标题）
///
/// `titleHighlighted` 已转义，命中词包在 `<mark>` 里。
#[utoipa::path(
    get,
    path = "/v1/search/suggest",
    tag = "search",
    params(SuggestQuery),
    responses(
        (status = 200, body = Vec<SuggestItem>),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn suggest(
    State(state): State<SearchState>,
    MaybeUser(viewer): MaybeUser,
    ClientMeta(meta): ClientMeta,
    Query(query): Query<SuggestQuery>,
) -> Result<Json<Vec<SuggestItem>>, Problem> {
    service::check_rate(&state, viewer.as_ref(), meta.ip)?;
    Ok(Json(
        service::suggest(&state, viewer.as_ref(), query).await?,
    ))
}

/// 搜索评论
///
/// 只返回所属帖子公开、自身与父评论未被隐藏的评论；不带父评论信息与楼层号。
#[utoipa::path(
    get,
    path = "/v1/search/comments",
    tag = "search",
    params(CommentSearchQuery),
    responses(
        (status = 200, body = SearchPage<CommentSearchView>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn search_comments(
    State(state): State<SearchState>,
    MaybeUser(viewer): MaybeUser,
    ClientMeta(meta): ClientMeta,
    Query(query): Query<CommentSearchQuery>,
) -> Result<Json<SearchPage<CommentSearchView>>, Problem> {
    service::check_rate(&state, viewer.as_ref(), meta.ip)?;
    Ok(Json(
        service::search_comments(&state, viewer.as_ref(), query).await?,
    ))
}
