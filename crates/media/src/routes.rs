//! HTTP 接口：`/v1/uploads/*`、`/v1/me/uploads`。

use axum::extract::{FromRef, State};
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use ik_auth::{AuthState, CurrentUser};
use ik_kernel::cursor::Page;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::id::marker::Media;
use ik_kernel::{Id, Problem};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::MediaState;
use crate::dto::{CompleteRequest, GalleryQuery, MediaView, SignRequest, SignResult};
use crate::service;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    MediaState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(sign))
        .routes(routes!(complete))
        .routes(routes!(gallery))
        .routes(routes!(remove))
}

/// 申请直传上传
///
/// 需要通过入站考试。返回预签名的 PUT 地址；客户端把原文件按 `method`、`headers` 传上去后调用完成接口。
/// 声明了 `contentHash` 且自己上传过相同内容时直接返回已有文件（`existing = true`）。限流：每人每分钟 60 次。
#[utoipa::path(
    post,
    path = "/v1/uploads/sign",
    tag = "media",
    request_body = SignRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = SignResult),
        (status = 400, description = "VALIDATION_FAILED：文件名、类型或大小不合法", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "EXAM_REQUIRED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "SERVICE_UNAVAILABLE：没有配置或连不上对象存储", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn sign(
    State(state): State<MediaState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<SignRequest>,
) -> Result<Json<SignResult>, Problem> {
    Ok(Json(service::sign(&state, &principal, request).await?))
}

/// 完成直传上传
///
/// 服务端读回上传的字节，校验大小、哈希与真实图片类型，发布到新地址后返回文件（201）。
/// 同一个 `uploadId` 重试返回同一个文件（200）；内容与已有文件相同时复用已有文件（200）。
#[utoipa::path(
    post,
    path = "/v1/uploads/{uploadId}/complete",
    tag = "media",
    params(("uploadId" = String, Path, description = "签名接口返回的 uploadId")),
    request_body = CompleteRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = MediaView),
        (status = 200, description = "重试或内容去重，返回已有文件", body = MediaView),
        (status = 400, description = "UPLOAD_TOKEN_INVALID / UPLOAD_TOKEN_EXPIRED / UPLOAD_NOT_FOUND / UPLOAD_SIZE_MISMATCH / UPLOAD_CHECKSUM_MISMATCH / UPLOAD_NOT_IMAGE", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "EXAM_REQUIRED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "UPLOAD_VERIFICATION_BUSY / UPLOAD_VERIFICATION_TIMEOUT（带 Retry-After）", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "SERVICE_UNAVAILABLE", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn complete(
    State(state): State<MediaState>,
    CurrentUser(principal): CurrentUser,
    Path(upload_id): Path<String>,
    Json(request): Json<CompleteRequest>,
) -> Result<Response, Problem> {
    let (created, media) = service::complete(&state, &principal, &upload_id, request).await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(media)).into_response())
}

/// 我的图库
///
/// 我上传过的图片，按加入时间倒序。
#[utoipa::path(
    get,
    path = "/v1/me/uploads",
    tag = "media",
    params(GalleryQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Page<MediaView>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn gallery(
    State(state): State<MediaState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<GalleryQuery>,
) -> Result<Json<Page<MediaView>>, Problem> {
    Ok(Json(service::gallery(&state, &principal, query).await?))
}

/// 从图库移除
///
/// 只是从自己的图库里拿掉，文件本身不删除，已经用到它的头像、帖子不受影响。
#[utoipa::path(
    delete,
    path = "/v1/me/uploads/{id}",
    tag = "media",
    params(("id" = String, Path, description = "媒体 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已移除"),
        (status = 404, description = "不在自己的图库里", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn remove(
    State(state): State<MediaState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Media>>,
) -> Result<StatusCode, Problem> {
    service::remove(&state, &principal, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}
