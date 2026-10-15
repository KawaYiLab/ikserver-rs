//! HTTP 接口：`/v1/dm/*`。会话列表与已读在统一收件箱 `/v1/inbox`（notify）。

use axum::extract::{FromRef, State};
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use ik_auth::{AuthState, CurrentUser};
use ik_kernel::cursor::Page;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::id::marker::{Conversation, Message};
use ik_kernel::{Id, Problem};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::MessagingState;
use crate::dto::{
    ConversationView, EditMessageRequest, MessageView, MessagesQuery, OpenDirectRequest,
    SendMessageRequest, UpdateConversationRequest,
};
use crate::service;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    MessagingState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(open_direct))
        .routes(routes!(conversation, update))
        .routes(routes!(leave))
        .routes(routes!(messages, send))
        .routes(routes!(edit, withdraw))
}

/// 发起私聊
///
/// 幂等：与同一个人只有一个会话，已存在时返回它（自己移除过的会恢复到列表）。
/// 需要通过入站考试；新会话在对方的列表里先不出现，收到第一条消息时才出现。
#[utoipa::path(
    post,
    path = "/v1/dm/direct",
    tag = "messaging",
    request_body = OpenDirectRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ConversationView),
        (status = 400, description = "DM_SELF / VALIDATION_FAILED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "EXAM_REQUIRED / USER_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "用户不存在、已封禁或已注销", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn open_direct(
    State(state): State<MessagingState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<OpenDirectRequest>,
) -> Result<Json<ConversationView>, Problem> {
    Ok(Json(
        service::open_direct(&state, &principal, request).await?,
    ))
}

/// 会话详情
#[utoipa::path(
    get,
    path = "/v1/dm/conversations/{id}",
    tag = "messaging",
    params(("id" = String, Path, description = "会话 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ConversationView),
        (status = 404, description = "会话不存在或不在自己的列表里", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn conversation(
    State(state): State<MessagingState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Conversation>>,
) -> Result<Json<ConversationView>, Problem> {
    Ok(Json(
        service::conversation(&state, &principal, id.get()).await?,
    ))
}

/// 修改会话设置（免打扰、置顶）
///
/// 只影响自己。每人最多置顶 50 个会话。免打扰只是客户端展示偏好，服务端不据此抑制推送或未读（D-076）。
#[utoipa::path(
    patch,
    path = "/v1/dm/conversations/{id}",
    tag = "messaging",
    params(("id" = String, Path, description = "会话 id")),
    request_body = UpdateConversationRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ConversationView),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "DM_PIN_LIMIT", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn update(
    State(state): State<MessagingState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Conversation>>,
    Json(request): Json<UpdateConversationRequest>,
) -> Result<Json<ConversationView>, Problem> {
    Ok(Json(
        service::update(&state, &principal, id.get(), request).await?,
    ))
}

/// 移除会话
///
/// 从自己的列表移除，历史保留；对方不受影响。对方再发来消息、或自己重新发起私聊时恢复。
#[utoipa::path(
    post,
    path = "/v1/dm/conversations/{id}/leave",
    tag = "messaging",
    params(("id" = String, Path, description = "会话 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已移除"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn leave(
    State(state): State<MessagingState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Conversation>>,
) -> Result<StatusCode, Problem> {
    service::leave(&state, &principal, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 会话里的消息（新的在前）
///
/// 混排了对方通知的时间线见 `GET /v1/inbox/{key}/items`。
#[utoipa::path(
    get,
    path = "/v1/dm/conversations/{id}/messages",
    tag = "messaging",
    params(("id" = String, Path, description = "会话 id"), MessagesQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Page<MessageView>),
        (status = 400, description = "CURSOR_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn messages(
    State(state): State<MessagingState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Conversation>>,
    Query(query): Query<MessagesQuery>,
) -> Result<Json<Page<MessageView>>, Problem> {
    Ok(Json(
        service::messages(&state, &principal, id.get(), query).await?,
    ))
}

/// 发消息
///
/// 需要通过入站考试。限流：每人每秒 5 条、每分钟 60 条。带 `clientId` 重试时返回第一次的消息（200）。
#[utoipa::path(
    post,
    path = "/v1/dm/conversations/{id}/messages",
    tag = "messaging",
    params(("id" = String, Path, description = "会话 id")),
    request_body = SendMessageRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = MessageView),
        (status = 200, description = "同一 clientId 的重放", body = MessageView),
        (status = 400, description = "VALIDATION_FAILED / REPLY_TARGET_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "EXAM_REQUIRED / USER_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "ALREADY_EXISTS：clientId 已用于另一个会话", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "RATE_LIMITED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn send(
    State(state): State<MessagingState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Conversation>>,
    Json(request): Json<SendMessageRequest>,
) -> Result<Response, Problem> {
    let (created, message) = service::send(&state, &principal, id.get(), request).await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(message)).into_response())
}

/// 编辑消息
///
/// 只能编辑自己发的、5 分钟内、未撤回的文本消息。需要通过入站考试。
#[utoipa::path(
    patch,
    path = "/v1/dm/messages/{id}",
    tag = "messaging",
    params(("id" = String, Path, description = "消息 id")),
    request_body = EditMessageRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = MessageView),
        (status = 400, description = "VALIDATION_FAILED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "EXAM_REQUIRED / FORBIDDEN：不是自己的消息", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "MESSAGE_TOO_OLD / MESSAGE_WITHDRAWN", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn edit(
    State(state): State<MessagingState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Message>>,
    Json(request): Json<EditMessageRequest>,
) -> Result<Json<MessageView>, Problem> {
    Ok(Json(
        service::edit(&state, &principal, id.get(), request).await?,
    ))
}

/// 撤回消息
///
/// 只能撤回自己发的、5 分钟内的消息。幂等：已撤回的直接返回。
#[utoipa::path(
    delete,
    path = "/v1/dm/messages/{id}",
    tag = "messaging",
    params(("id" = String, Path, description = "消息 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = MessageView),
        (status = 403, description = "FORBIDDEN：不是自己的消息", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "MESSAGE_TOO_OLD", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn withdraw(
    State(state): State<MessagingState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Message>>,
) -> Result<Json<MessageView>, Problem> {
    Ok(Json(service::withdraw(&state, &principal, id.get()).await?))
}
