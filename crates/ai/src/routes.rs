//! HTTP 接口：`/v1/ai/characters`、`/v1/ai/models`、`/v1/ai/quota`、`/v1/me/ai-assistant`。

use axum::extract::{FromRef, State};
use ik_auth::{AuthState, CurrentUser};
use ik_kernel::Problem;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::Json;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AiState;
use crate::dto::{AssistantView, CharacterList, ModelList, QuotaView};
use crate::service;

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
