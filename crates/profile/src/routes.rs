//! HTTP 接口：`/v1/users/{id}`（作者主页及其帖子、评论）、`/v1/users/search`、
//! `/v1/users/mention-candidates`、`/v1/me/profile*`、`/v1/me/username`。

use axum::extract::{FromRef, State};
use ik_auth::{AuthState, ClientMeta, CurrentUser, MaybeUser};
use ik_comments::dto::AuthorCommentView;
use ik_content::dto::{PinCandidate, PinCandidatesQuery};
use ik_kernel::cursor::Page;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::id::marker::User;
use ik_kernel::{Id, Problem};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::ProfileState;
use crate::dto::{
    AvatarResult, MentionCandidate, MyProfileView, PageQuery, PinnedPostsRequest, ProfilePostsPage,
    ProfilePostsQuery, ProfileView, RenameRequest, RenameResult, SearchQuery, SetAvatarRequest,
    UpdateProfileRequest, UserSearchItem,
};
use crate::service;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    ProfileState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(profile))
        .routes(routes!(profile_posts))
        .routes(routes!(profile_comments))
        .routes(routes!(search))
        .routes(routes!(mention_candidates))
        .routes(routes!(my_profile, update_profile))
        .routes(routes!(rename))
        .routes(routes!(set_avatar, clear_avatar))
        .routes(routes!(set_pinned))
        .routes(routes!(pin_candidates))
}

/// 作者主页
///
/// 双方存在拉黑或对方隐藏了主页时返回精简视图（见 `ProfileView` 说明）。已注销的账号 404。
#[utoipa::path(
    get,
    path = "/v1/users/{id}",
    tag = "profile",
    params(("id" = String, Path, description = "用户 id")),
    responses(
        (status = 200, body = ProfileView),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn profile(
    State(state): State<ProfileState>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<Id<User>>,
) -> Result<Json<ProfileView>, Problem> {
    Ok(Json(
        service::profile(&state, viewer.as_ref(), id.get()).await?,
    ))
}

/// 作者主页的帖子
///
/// 配置了主页精选时默认只返回精选（`pinned: true`，没有下一页），`scope=all` 返回全部。
/// 匿名帖永远不出现；双方存在拉黑或对方隐藏了主页时返回空页。
#[utoipa::path(
    get,
    path = "/v1/users/{id}/posts",
    tag = "profile",
    params(("id" = String, Path, description = "用户 id"), ProfilePostsQuery),
    responses(
        (status = 200, body = ProfilePostsPage),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn profile_posts(
    State(state): State<ProfileState>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<Id<User>>,
    Query(query): Query<ProfilePostsQuery>,
) -> Result<Json<ProfilePostsPage>, Problem> {
    Ok(Json(
        service::posts(&state, viewer.as_ref(), id.get(), query).await?,
    ))
}

/// 作者主页的评论
///
/// 只含非匿名、未隐藏、所属帖子公开的评论；不带父评论信息。一页可能不满，`nextCursor` 为空才是到底。
#[utoipa::path(
    get,
    path = "/v1/users/{id}/comments",
    tag = "profile",
    params(("id" = String, Path, description = "用户 id"), PageQuery),
    responses(
        (status = 200, body = Page<AuthorCommentView>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn profile_comments(
    State(state): State<ProfileState>,
    MaybeUser(viewer): MaybeUser,
    Path(id): Path<Id<User>>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<AuthorCommentView>>, Problem> {
    Ok(Json(
        service::comments(&state, viewer.as_ref(), id.get(), query).await?,
    ))
}

/// 搜索用户
///
/// 按用户名子串匹配（不区分大小写），完全匹配和前缀匹配排在前面；不含隐藏主页的人和有拉黑关系的人。
/// 关键词为空或超过 32 字时返回空列表。限流：登录用户每秒 10 次，游客按 IP 每秒 3 次。
#[utoipa::path(
    get,
    path = "/v1/users/search",
    tag = "profile",
    params(SearchQuery),
    responses(
        (status = 200, body = Vec<UserSearchItem>),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn search(
    State(state): State<ProfileState>,
    MaybeUser(viewer): MaybeUser,
    ClientMeta(meta): ClientMeta,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<UserSearchItem>>, Problem> {
    Ok(Json(
        service::search(&state, viewer.as_ref(), meta.ip, query).await?,
    ))
}

/// @ 提及候选
#[utoipa::path(
    get,
    path = "/v1/users/mention-candidates",
    tag = "profile",
    params(SearchQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Vec<MentionCandidate>),
        (status = 400, description = "VALIDATION_FAILED：关键词为空或过长", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn mention_candidates(
    State(state): State<ProfileState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<MentionCandidate>>, Problem> {
    Ok(Json(
        service::mention_candidates(&state, &principal, query).await?,
    ))
}

/// 我的资料设置
#[utoipa::path(
    get,
    path = "/v1/me/profile",
    tag = "profile",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = MyProfileView),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn my_profile(
    State(state): State<ProfileState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<MyProfileView>, Problem> {
    Ok(Json(service::my_profile(&state, &principal).await?))
}

/// 修改签名、隐藏主页
///
/// 只改传了的字段。不收费，不要求通过考试。
#[utoipa::path(
    patch,
    path = "/v1/me/profile",
    tag = "profile",
    request_body = UpdateProfileRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = MyProfileView),
        (status = 400, description = "VALIDATION_FAILED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn update_profile(
    State(state): State<ProfileState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<UpdateProfileRequest>,
) -> Result<Json<MyProfileView>, Problem> {
    Ok(Json(
        service::update_profile(&state, &principal, request).await?,
    ))
}

/// 改用户名
///
/// 每次 10 丁尼（包括第一次）；与原名相同时直接成功、不扣费。名字被占用时不扣费。
#[utoipa::path(
    put,
    path = "/v1/me/username",
    tag = "profile",
    request_body = RenameRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = RenameResult),
        (status = 400, description = "VALIDATION_FAILED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 402, description = "INSUFFICIENT_DENNY", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "USERNAME_TAKEN", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn rename(
    State(state): State<ProfileState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<RenameRequest>,
) -> Result<Json<RenameResult>, Problem> {
    Ok(Json(service::rename(&state, &principal, request).await?))
}

/// 设置主页精选
#[utoipa::path(
    put,
    path = "/v1/me/profile/pinned-posts",
    tag = "profile",
    request_body = PinnedPostsRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = MyProfileView),
        (status = 400, description = "VALIDATION_FAILED：超过 6 篇", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn set_pinned(
    State(state): State<ProfileState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<PinnedPostsRequest>,
) -> Result<Json<MyProfileView>, Problem> {
    Ok(Json(
        service::set_pinned(&state, &principal, request).await?,
    ))
}

/// 主页精选的候选帖子
///
/// 自己已发布的非匿名帖，新的在前，可按标题搜索。
#[utoipa::path(
    get,
    path = "/v1/me/profile/pin-candidates",
    tag = "profile",
    params(PinCandidatesQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Page<PinCandidate>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn pin_candidates(
    State(state): State<ProfileState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<PinCandidatesQuery>,
) -> Result<Json<Page<PinCandidate>>, Problem> {
    Ok(Json(
        ik_content::service::pin_candidates(&state.content, &principal, query).await?,
    ))
}

/// 设置自定义头像
///
/// 用自己图库里的图片（先走直传上传）。每次 10 丁尼，换成当前同一张图不扣费；不要求通过考试。
#[utoipa::path(
    put,
    path = "/v1/me/avatar",
    tag = "profile",
    request_body = SetAvatarRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AvatarResult),
        (status = 402, description = "INSUFFICIENT_DENNY", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "图片不存在或不在自己的图库里", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn set_avatar(
    State(state): State<ProfileState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<SetAvatarRequest>,
) -> Result<Json<AvatarResult>, Problem> {
    Ok(Json(
        service::set_avatar(&state, &principal, request).await?,
    ))
}

/// 清除自定义头像
///
/// 恢复默认头像，不收费。
#[utoipa::path(
    delete,
    path = "/v1/me/avatar",
    tag = "profile",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AvatarResult),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn clear_avatar(
    State(state): State<ProfileState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<AvatarResult>, Problem> {
    Ok(Json(service::clear_avatar(&state, &principal).await?))
}
