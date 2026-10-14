//! HTTP 接口：`/v1/categories`、`/v1/tags/*`、`/v1/posts*`、`/v1/me/posts`。

use axum::extract::{FromRef, State};
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use ik_auth::{AuthState, ClientMeta, CurrentUser, MaybeUser};
use ik_kernel::Id;
use ik_kernel::Problem;
use ik_kernel::cursor::Page;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::id::marker::Post;
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::ContentState;
use crate::dto::{
    CategoryView, CreatePostQuery, FeedPage, FeedQuery, GiftRequest, GiftResult, MyPostItem,
    MyPostsQuery, PostDetail, PostEditView, PostInput, TagRef, TagSuggestQuery, ViewResult,
};
use crate::service;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    ContentState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(list_categories))
        .routes(routes!(suggest_tags))
        .routes(routes!(feed, create_post))
        .routes(routes!(post_detail, delete_post))
        .routes(routes!(edit_view))
        .routes(routes!(save_draft, discard_pending))
        .routes(routes!(publish))
        .routes(routes!(unpublish))
        .routes(routes!(record_view))
        .routes(routes!(gift_denny))
        .routes(routes!(pin, unpin))
        .routes(routes!(my_posts))
}

/// `ETag: "rev:<n>"`
fn with_etag(view: PostEditView, status: StatusCode) -> Response {
    let etag = HeaderValue::from_str(&format!("\"rev:{}\"", view.revision)).ok();
    let mut response = (status, Json(view)).into_response();
    if let Some(etag) = etag {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}

/// 解析 `If-Match: "rev:<n>"`。没带或为 `*` 时不校验；格式不对按版本冲突处理。
fn expected_revision(headers: &HeaderMap) -> Result<Option<i32>, Problem> {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    let raw = value.to_str().unwrap_or_default().trim();
    if raw == "*" {
        return Ok(None);
    }
    raw.trim_start_matches("W/")
        .trim_matches('"')
        .strip_prefix("rev:")
        .and_then(|n| n.parse::<i32>().ok())
        .map(Some)
        .ok_or_else(|| {
            Problem::new(
                StatusCode::PRECONDITION_FAILED,
                "REVISION_CONFLICT",
                "内容已在别处被修改，请刷新后重试",
            )
        })
}

/// 启用中的分区（按排序）
#[utoipa::path(
    get,
    path = "/v1/categories",
    tag = "content",
    responses((status = 200, body = Vec<CategoryView>))
)]
async fn list_categories(
    State(state): State<ContentState>,
) -> Result<Json<Vec<CategoryView>>, Problem> {
    Ok(Json(service::categories(&state).await?))
}

/// 标签联想
#[utoipa::path(
    get,
    path = "/v1/tags/suggest",
    tag = "content",
    params(TagSuggestQuery),
    responses(
        (status = 200, body = Vec<TagRef>),
        (status = 400, description = "关键词过长", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn suggest_tags(
    State(state): State<ContentState>,
    Query(query): Query<TagSuggestQuery>,
) -> Result<Json<Vec<TagRef>>, Problem> {
    Ok(Json(
        service::suggest_tags(&state, &query.q, query.limit).await?,
    ))
}

/// 信息流（最新）
///
/// 按最近活跃倒序（发布或有新评论）。第一页且未按标签筛选时，置顶帖单独放在 `pinned`。
#[utoipa::path(
    get,
    path = "/v1/posts",
    tag = "content",
    params(FeedQuery),
    responses(
        (status = 200, body = FeedPage),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "分区或标签不存在", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn feed(
    State(state): State<ContentState>,
    MaybeUser(viewer): MaybeUser,
    Query(query): Query<FeedQuery>,
) -> Result<Json<FeedPage>, Problem> {
    Ok(Json(service::feed(&state, viewer.as_ref(), query).await?))
}

/// 新建帖子（默认存为草稿，`?publish=true` 直接发布）
#[utoipa::path(
    post,
    path = "/v1/posts",
    tag = "content",
    params(CreatePostQuery),
    request_body = PostInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = PostEditView, headers(("ETag" = String, description = "\"rev:<n>\""))),
        (status = 400, description = "VALIDATION_FAILED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "UNAUTHORIZED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "EXAM_REQUIRED / CATEGORY_ADMIN_ONLY", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn create_post(
    State(state): State<ContentState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<CreatePostQuery>,
    Json(input): Json<PostInput>,
) -> Result<Response, Problem> {
    let view = service::create(&state, &principal, input, query.publish).await?;
    Ok(with_etag(view, StatusCode::CREATED))
}

/// 帖子详情
#[utoipa::path(
    get,
    path = "/v1/posts/{id}",
    tag = "content",
    params(("id" = String, Path, description = "帖子 id")),
    responses(
        (status = 200, body = PostDetail),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn post_detail(
    State(state): State<ContentState>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<Id<Post>>,
) -> Result<Json<PostDetail>, Problem> {
    Ok(Json(
        service::detail(&state, viewer.as_ref(), id.get()).await?,
    ))
}

/// 删除自己的帖子
///
/// 不需要通过入站考试，不扣丁尼。已发布的匿名帖不能删除。
#[utoipa::path(
    delete,
    path = "/v1/posts/{id}",
    tag = "content",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已删除"),
        (status = 403, description = "FORBIDDEN / ANONYMOUS_POST_LOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn delete_post(
    State(state): State<ContentState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Post>>,
) -> Result<StatusCode, Problem> {
    service::delete(&state, &principal, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 作者编辑视图（含未发布的修改与编辑器状态）
#[utoipa::path(
    get,
    path = "/v1/posts/{id}/edit",
    tag = "content",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = PostEditView, headers(("ETag" = String))),
        (status = 403, description = "FORBIDDEN / ANONYMOUS_POST_LOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn edit_view(
    State(state): State<ContentState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Post>>,
) -> Result<Response, Problem> {
    let view = service::edit_view(&state, &principal, id.get()).await?;
    Ok(with_etag(view, StatusCode::OK))
}

/// 保存工作副本
///
/// 草稿直接保存；已发布的帖子保存为“待发布修改”，线上内容不变，直到调用发布。
/// 带 `If-Match: "rev:<n>"` 可防止多端编辑互相覆盖。
#[utoipa::path(
    put,
    path = "/v1/posts/{id}/draft",
    tag = "content",
    params(("id" = String, Path), ("If-Match" = Option<String>, Header, description = "\"rev:<n>\"")),
    request_body = PostInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = PostEditView, headers(("ETag" = String))),
        (status = 400, description = "VALIDATION_FAILED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "FORBIDDEN / EXAM_REQUIRED / ANONYMOUS_POST_LOCKED / CATEGORY_ADMIN_ONLY", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 412, description = "REVISION_CONFLICT", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn save_draft(
    State(state): State<ContentState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Post>>,
    headers: HeaderMap,
    Json(input): Json<PostInput>,
) -> Result<Response, Problem> {
    let expected = expected_revision(&headers)?;
    let view = service::save_draft(&state, &principal, id.get(), input, expected).await?;
    Ok(with_etag(view, StatusCode::OK))
}

/// 丢弃未发布的修改（回到线上版本）
#[utoipa::path(
    delete,
    path = "/v1/posts/{id}/draft",
    tag = "content",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = PostEditView, headers(("ETag" = String))),
        (status = 409, description = "NOT_PUBLISHED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn discard_pending(
    State(state): State<ContentState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Post>>,
) -> Result<Response, Problem> {
    let view = service::discard_pending(&state, &principal, id.get()).await?;
    Ok(with_etag(view, StatusCode::OK))
}

/// 发布
///
/// 草稿 → 公开（进入信息流顶部）；已发布的帖子 → 把未发布的修改上线（不顶帖）。
#[utoipa::path(
    post,
    path = "/v1/posts/{id}/publish",
    tag = "content",
    params(("id" = String, Path), ("If-Match" = Option<String>, Header, description = "\"rev:<n>\"")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = PostEditView, headers(("ETag" = String))),
        (status = 400, description = "正文超过当前等级上限", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "FORBIDDEN / EXAM_REQUIRED / ANONYMOUS_POST_LOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 412, description = "REVISION_CONFLICT", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn publish(
    State(state): State<ContentState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Post>>,
    headers: HeaderMap,
) -> Result<Response, Problem> {
    let expected = expected_revision(&headers)?;
    let view = service::publish(&state, &principal, id.get(), None, expected).await?;
    Ok(with_etag(view, StatusCode::OK))
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
struct UnpublishQuery {
    /// 有未发布的修改时：`true` 丢弃，默认并入草稿保留。
    #[serde(default)]
    discard_pending: bool,
}

/// 下线（回到草稿）
#[utoipa::path(
    post,
    path = "/v1/posts/{id}/unpublish",
    tag = "content",
    params(("id" = String, Path), UnpublishQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = PostEditView, headers(("ETag" = String))),
        (status = 403, description = "FORBIDDEN / EXAM_REQUIRED / ANONYMOUS_POST_LOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "NOT_PUBLISHED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unpublish(
    State(state): State<ContentState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Post>>,
    Query(query): Query<UnpublishQuery>,
) -> Result<Response, Problem> {
    let view = service::unpublish(&state, &principal, id.get(), query.discard_pending).await?;
    Ok(with_etag(view, StatusCode::OK))
}

/// 上报一次浏览
///
/// 同一人（登录用户按账号，游客按 IP）5 分钟内重复浏览同一帖子只计一次；每人每分钟最多计 30 次。
#[utoipa::path(
    post,
    path = "/v1/posts/{id}/views",
    tag = "content",
    params(("id" = String, Path)),
    responses(
        (status = 200, body = ViewResult),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn record_view(
    State(state): State<ContentState>,
    MaybeUser(viewer): MaybeUser,
    ClientMeta(meta): ClientMeta,
    Path(id): Path<Id<Post>>,
) -> Result<Json<ViewResult>, Problem> {
    let key = match viewer {
        Some(p) => format!("u:{}", p.user_id),
        None => format!("ip:{}", meta.ip),
    };
    Ok(Json(service::record_view(&state, key, id.get()).await?))
}

/// 投币（给帖子投 1 丁尼）
///
/// 每人每帖终身一次。不能投给自己的帖子和匿名帖子。作者同时获得 1 丁尼与 1 点经验。
#[utoipa::path(
    post,
    path = "/v1/posts/{id}/denny",
    tag = "content",
    params(("id" = String, Path)),
    request_body = GiftRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = GiftResult),
        (status = 402, description = "INSUFFICIENT_DENNY", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "EXAM_REQUIRED / DENNY_SELF_GIVE / DENNY_ANONYMOUS_POST", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "帖子不存在或不公开", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "DENNY_ALREADY_GIVEN", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn gift_denny(
    State(state): State<ContentState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Post>>,
    Json(request): Json<GiftRequest>,
) -> Result<Json<GiftResult>, Problem> {
    Ok(Json(
        service::gift_denny(&state, &principal, id.get(), request).await?,
    ))
}

/// 置顶（管理员）
#[utoipa::path(
    put,
    path = "/v1/posts/{id}/pin",
    tag = "content",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已置顶"),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "帖子不存在或不公开", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn pin(
    State(state): State<ContentState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Post>>,
) -> Result<StatusCode, Problem> {
    service::pin(&state, &principal, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 取消置顶（管理员）
#[utoipa::path(
    delete,
    path = "/v1/posts/{id}/pin",
    tag = "content",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已取消"),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unpin(
    State(state): State<ContentState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Post>>,
) -> Result<StatusCode, Problem> {
    service::unpin(&state, &principal, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 我的帖子（草稿与已发布）
#[utoipa::path(
    get,
    path = "/v1/me/posts",
    tag = "me",
    params(MyPostsQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Page<MyPostItem>),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn my_posts(
    State(state): State<ContentState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<MyPostsQuery>,
) -> Result<Json<Page<MyPostItem>>, Problem> {
    Ok(Json(service::my_posts(&state, &principal, query).await?))
}
