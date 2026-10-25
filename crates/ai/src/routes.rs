//! HTTP 接口：`/v1/ai/characters`、`/v1/ai/models`、`/v1/ai/quota`、`/v1/me/ai-assistant`，
//! 以及 AI 会话 `/v1/ai/sessions*`、AI 回复的停止与重新生成 `/v1/ai/messages/{id}/*`、
//! 记忆面板 `/v1/ai/memories*`、`/v1/ai/memory-settings`（ADR 0007）。

use axum::extract::{FromRef, State};
use http::StatusCode;
use ik_auth::{AuthState, CurrentUser};
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path};
use ik_kernel::id::marker::{AiMemory, Conversation, Message};
use ik_kernel::{Id, Problem};
use ik_messaging::dto::{ConversationView, MessageView};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AiState;
use crate::dto::{
    AssistantView, CharacterList, ClearMemoriesRequest, CreateMemoryRequest, CreateSessionRequest,
    MemoryList, MemorySettings, MemoryView, ModelList, QuotaView, RegenerateAccepted,
    RegenerateRequest, SessionView, UpdateMemoryRequest, UpdateSessionRequest,
};
use crate::{memory, service, sessions};

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    AiState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(characters))
        .routes(routes!(models))
        .routes(routes!(quota))
        .routes(routes!(assistant))
        .routes(routes!(create_session))
        .routes(routes!(session, update_session))
        .routes(routes!(reset_session))
        .routes(routes!(stop))
        .routes(routes!(regenerate))
        .routes(routes!(memories, create_memory))
        .routes(routes!(update_memory, delete_memory))
        .routes(routes!(clear_memories))
        .routes(routes!(memory_settings))
        .merge(crate::admin::router())
}

/// AI 角色
///
/// 敲敲里可以对话的 AI 角色（启用且对敲敲可见），按排序。匿名可读。
#[utoipa::path(
    get,
    path = "/v1/ai/characters",
    tag = "ai",
    responses((status = 200, body = CharacterList))
)]
async fn characters(State(state): State<AiState>) -> Result<Json<CharacterList>, Problem> {
    Ok(Json(service::characters(&state).await?))
}

/// AI 模型
///
/// 全部启用的模型，按排序。匿名可读。
#[utoipa::path(
    get,
    path = "/v1/ai/models",
    tag = "ai",
    responses((status = 200, body = ModelList))
)]
async fn models(State(state): State<AiState>) -> Result<Json<ModelList>, Problem> {
    Ok(Json(service::models(&state).await?))
}

/// 今日 AI 额度
///
/// 只给百分比。读数含进行中的回复（回复中先跳高、结算后回落）；`available = false` 时不要展示。
#[utoipa::path(
    get,
    path = "/v1/ai/quota",
    tag = "ai",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = QuotaView),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn quota(
    State(state): State<AiState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<QuotaView>, Problem> {
    Ok(Json(service::quota(&state, &principal).await?))
}

/// 个人助手
///
/// 二期功能，目前固定返回未开放。
#[utoipa::path(
    get,
    path = "/v1/me/ai-assistant",
    tag = "ai",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AssistantView),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn assistant(CurrentUser(_principal): CurrentUser) -> Json<AssistantView> {
    Json(service::assistant())
}

/// 新建 AI 会话
///
/// 每次新建一条（同一个角色可以有多条）。需要通过入站考试；角色须启用且开放私信。
/// 返回的会话 `kind = ai`；之后在 `/v1/dm/conversations/{id}/messages` 发消息即触发 AI 回复。
#[utoipa::path(
    post,
    path = "/v1/ai/sessions",
    tag = "ai",
    request_body = CreateSessionRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = ConversationView),
        (status = 400, description = "DM_SELF / VALIDATION_FAILED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "EXAM_REQUIRED / USER_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "AI_CHARACTER_NOT_FOUND", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn create_session(
    State(state): State<AiState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<ConversationView>), Problem> {
    Ok((
        StatusCode::CREATED,
        Json(sessions::create(&state, &principal, request).await?),
    ))
}

/// AI 会话设置
#[utoipa::path(
    get,
    path = "/v1/ai/sessions/{id}",
    tag = "ai",
    params(("id" = String, Path, description = "会话 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = SessionView),
        (status = 400, description = "NOT_AI_SESSION", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "会话不存在或不在自己的列表里", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn session(
    State(state): State<AiState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Conversation>>,
) -> Result<Json<SessionView>, Problem> {
    Ok(Json(sessions::get(&state, &principal, id.get()).await?))
}

/// 修改 AI 会话
///
/// 改标题、会话级模型、无痕。字段缺省不改；null 或空串清除（模型清除后跟随角色卡默认）。
/// 无痕会话不进长期记忆、不给别的会话做衔接；关掉无痕后，无痕期间的对话也不会补提炼。
/// 模型须启用且在角色的白名单内。变更推 `dm.conversation.updated`。
#[utoipa::path(
    patch,
    path = "/v1/ai/sessions/{id}",
    tag = "ai",
    request_body = UpdateSessionRequest,
    params(("id" = String, Path, description = "会话 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = SessionView),
        (status = 400, description = "NOT_AI_SESSION / AI_MODEL_UNAVAILABLE / AI_MODEL_NOT_ALLOWED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "会话不存在、不在自己的列表里，或角色已下线（AI_CHARACTER_NOT_FOUND）", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn update_session(
    State(state): State<AiState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Conversation>>,
    Json(request): Json<UpdateSessionRequest>,
) -> Result<Json<SessionView>, Problem> {
    Ok(Json(
        sessions::update(&state, &principal, id.get(), request).await?,
    ))
}

/// 重置 AI 记忆
///
/// 写一条 `[对话已重置]` 分界消息（不删聊天记录），AI 之后只读分界之后的消息；
/// 进行中的旧回复会被停止。
#[utoipa::path(
    post,
    path = "/v1/ai/sessions/{id}/reset",
    tag = "ai",
    params(("id" = String, Path, description = "会话 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, description = "分界消息", body = MessageView),
        (status = 400, description = "NOT_AI_SESSION", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn reset_session(
    State(state): State<AiState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Conversation>>,
) -> Result<(StatusCode, Json<MessageView>), Problem> {
    Ok((
        StatusCode::CREATED,
        Json(sessions::reset(&state, &principal, id.get()).await?),
    ))
}

/// 停止生成
///
/// 停止一条正在生成的 AI 回复：已生成的正文保留并加「（已停止）」。已经结束的回复直接返回 204。
#[utoipa::path(
    post,
    path = "/v1/ai/messages/{id}/stop",
    tag = "ai",
    params(("id" = String, Path, description = "AI 回复的消息 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已发出停止信号"),
        (status = 400, description = "NOT_AI_REPLY", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn stop(
    State(state): State<AiState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Message>>,
) -> Result<StatusCode, Problem> {
    sessions::stop(&state, &principal, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 重新生成
///
/// 只能是会话里最后一条 AI 回复。旧回复被撤回（`dm.message.withdrawn`），新回复随后到达；
/// 重复提交只生成一次。`modelKey` 只对这一次生效。
#[utoipa::path(
    post,
    path = "/v1/ai/messages/{id}/regenerate",
    tag = "ai",
    request_body = RegenerateRequest,
    params(("id" = String, Path, description = "AI 回复的消息 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 202, body = RegenerateAccepted),
        (status = 400, description = "NOT_AI_REPLY / AI_MODEL_UNAVAILABLE / AI_MODEL_NOT_ALLOWED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "EXAM_REQUIRED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "AI_NOT_LATEST_REPLY / AI_TRIGGER_WITHDRAWN", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn regenerate(
    State(state): State<AiState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<Message>>,
    Json(request): Json<RegenerateRequest>,
) -> Result<(StatusCode, Json<RegenerateAccepted>), Problem> {
    Ok((
        StatusCode::ACCEPTED,
        Json(sessions::regenerate(&state, &principal, id.get(), request).await?),
    ))
}

/// 我的 AI 记忆
///
/// 记忆面板：开关与全部记忆。`profile` 是用户档案（所有角色共享），`relation` 是与某个角色之间的
/// 关系记忆、`event` 是与某个角色聊过的具体经历（都带 `character`）。置顶的在前，其余最近更新的在前；
/// 档案、每个角色的关系记忆各最多 40 条，事件最多 300 条。`sensitivity = public` 的才会用在公开的评论区。
#[utoipa::path(
    get,
    path = "/v1/ai/memories",
    tag = "ai",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = MemoryList),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn memories(
    State(state): State<AiState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<MemoryList>, Problem> {
    Ok(Json(memory::list(&state, &principal).await?))
}

/// 添加记忆
///
/// 手动写一条（来源 `manual`）。同一类里已经有一样的就返回那条。事件可以带发生日期与待跟进日期。
#[utoipa::path(
    post,
    path = "/v1/ai/memories",
    tag = "ai",
    request_body = CreateMemoryRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = MemoryView),
        (status = 400, description = "VALIDATION_FAILED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "AI_CHARACTER_NOT_FOUND", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "AI_MEMORY_FULL", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn create_memory(
    State(state): State<AiState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<CreateMemoryRequest>,
) -> Result<(StatusCode, Json<MemoryView>), Problem> {
    Ok((
        StatusCode::CREATED,
        Json(memory::create(&state, &principal, request).await?),
    ))
}

/// 修改记忆
///
/// 改正文（来源变为 `manual`）、置顶、敏感度；事件还能改日期与待跟进（null 清除）。
/// 置顶的不会被自动整理改写或淘汰。
#[utoipa::path(
    patch,
    path = "/v1/ai/memories/{id}",
    tag = "ai",
    request_body = UpdateMemoryRequest,
    params(("id" = String, Path, description = "记忆 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = MemoryView),
        (status = 400, description = "VALIDATION_FAILED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn update_memory(
    State(state): State<AiState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<AiMemory>>,
    Json(request): Json<UpdateMemoryRequest>,
) -> Result<Json<MemoryView>, Problem> {
    Ok(Json(
        memory::update(&state, &principal, id.get(), request).await?,
    ))
}

/// 删除记忆
#[utoipa::path(
    delete,
    path = "/v1/ai/memories/{id}",
    tag = "ai",
    params(("id" = String, Path, description = "记忆 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已删除"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn delete_memory(
    State(state): State<AiState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<AiMemory>>,
) -> Result<StatusCode, Problem> {
    memory::delete(&state, &principal, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 清空记忆
///
/// 指定角色：只清与它的关系与事件记忆（「让它忘了我」）；不指定：清空全部，含档案。不可恢复。
#[utoipa::path(
    post,
    path = "/v1/ai/memories/clear",
    tag = "ai",
    request_body = ClearMemoriesRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已清空"),
        (status = 404, description = "AI_CHARACTER_NOT_FOUND", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn clear_memories(
    State(state): State<AiState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<ClearMemoriesRequest>,
) -> Result<StatusCode, Problem> {
    memory::clear(&state, &principal, request).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 记忆开关
///
/// 关掉后 AI 不再读取、不再记下新的记忆，也不再跨会话衔接；已有记忆保留，可以在面板里清空。
#[utoipa::path(
    put,
    path = "/v1/ai/memory-settings",
    tag = "ai",
    request_body = MemorySettings,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = MemorySettings),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn memory_settings(
    State(state): State<AiState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<MemorySettings>,
) -> Result<Json<MemorySettings>, Problem> {
    Ok(Json(
        memory::set_enabled(&state, &principal, request).await?,
    ))
}
