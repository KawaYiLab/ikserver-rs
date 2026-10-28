//! AI 记忆的管理后台（ADR 0007，MEM-030～033）：全站设置与统计、按用户查看 / 删除 / 清空记忆。
//!
//! 用户端没有任何记忆设置；这里是唯一的入口。全部只给管理员，写操作与审计同一事务，
//! 设置改完让本进程的配置缓存失效（其他实例最长 60 秒后生效，AI-002）。

use axum::extract::{FromRef, State};
use chrono::Utc;
use http::StatusCode;
use ik_auth::admin::{self, Audit};
use ik_auth::{AdminOperator, AuthState, Operator};
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::id::marker::AiMemory;
use ik_kernel::{Id, Problem};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AiState;
use crate::dto::{
    ClearMemoriesRequest, MemorySettingsInput, MemorySettingsView, MemoryStats, UserMemories,
};
use crate::memory;
use crate::repo::{self, NewMemorySettings};

/// 全站设置与统计。
pub async fn settings(state: &AiState) -> Result<MemorySettingsView, Problem> {
    let mut conn = state.db.acquire().await?;
    let row = repo::memory_settings(&mut conn).await?;
    let stats = repo::memory_stats(&mut conn).await?;
    let defaults = crate::catalog::MemoryConfig::default();
    Ok(MemorySettingsView {
        enabled: row.as_ref().map_or(defaults.enabled, |r| r.enabled),
        model_key: row.as_ref().and_then(|r| r.model_key.clone()),
        extract_delay_secs: row.as_ref().map_or_else(
            || i32::try_from(defaults.extract_delay_secs).unwrap_or(180),
            |r| r.extract_delay_secs,
        ),
        bridge_hours: row.as_ref().map_or_else(
            || i32::try_from(defaults.bridge_hours).unwrap_or(48),
            |r| r.bridge_hours,
        ),
        embedding_provider: row.as_ref().and_then(|r| r.embedding_provider.clone()),
        embedding_model: row.as_ref().and_then(|r| r.embedding_model.clone()),
        updated_at: row.map(|r| r.updated_at),
        stats: MemoryStats {
            users: stats.users,
            profile: stats.profile,
            relation: stats.relation,
            event: stats.event,
            pending_follow_ups: stats.pending_follow_ups,
            with_embedding: stats.with_embedding,
        },
    })
}

/// 去掉首尾空白；空串当作没填。
fn optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// 改全站设置（MEM-030）。模型要存在；向量的提供方与模型名要么都填、要么都不填，提供方要可用。
pub async fn update_settings(
    state: &AiState,
    op: &Operator,
    input: MemorySettingsInput,
) -> Result<MemorySettingsView, Problem> {
    if !(0..=86_400).contains(&input.extract_delay_secs) {
        return Err(Problem::field(
            "extractDelaySecs",
            "OUT_OF_RANGE",
            "提炼延迟在 0–86400 秒之间",
        ));
    }
    if !(0..=720).contains(&input.bridge_hours) {
        return Err(Problem::field(
            "bridgeHours",
            "OUT_OF_RANGE",
            "衔接时长在 0–720 小时之间",
        ));
    }
    let model_key = optional(input.model_key.as_deref());
    let embedding_provider = optional(input.embedding_provider.as_deref());
    let embedding_model = optional(input.embedding_model.as_deref());
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    if let Some(key) = &model_key
        && !repo::models(&mut tx).await?.iter().any(|m| &m.key == key)
    {
        return Err(Problem::field("modelKey", "NOT_FOUND", "没有这个模型"));
    }
    match (&embedding_provider, &embedding_model) {
        (None, None) => {}
        (Some(provider), Some(model)) => {
            let usable = state.providers.contains(provider)
                || repo::providers(&mut tx)
                    .await?
                    .iter()
                    .any(|p| p.enabled && &p.name == provider);
            if !usable {
                return Err(Problem::field(
                    "embeddingProvider",
                    "NOT_FOUND",
                    "没有这个提供方，或它没有启用",
                ));
            }
            if model.chars().count() > 128 {
                return Err(Problem::field(
                    "embeddingModel",
                    "TOO_LONG",
                    "向量模型名最多 128 字符",
                ));
            }
        }
        (None, Some(_)) => {
            return Err(Problem::field(
                "embeddingProvider",
                "REQUIRED",
                "填了向量模型就要选提供方",
            ));
        }
        (Some(_), None) => {
            return Err(Problem::field(
                "embeddingModel",
                "REQUIRED",
                "选了提供方就要填向量模型名",
            ));
        }
    }
    repo::upsert_memory_settings(
        &mut tx,
        &NewMemorySettings {
            enabled: input.enabled,
            model_key: model_key.as_deref(),
            extract_delay_secs: input.extract_delay_secs,
            bridge_hours: input.bridge_hours,
            embedding_provider: embedding_provider.as_deref(),
            embedding_model: embedding_model.as_deref(),
            now,
        },
    )
    .await?;
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "ai.memory.settings.update",
            target_type: "ai_memory_settings",
            target_id: None,
            details: serde_json::json!({
                "enabled": input.enabled,
                "modelKey": model_key,
                "extractDelaySecs": input.extract_delay_secs,
                "bridgeHours": input.bridge_hours,
                "embeddingProvider": embedding_provider,
                "embeddingModel": embedding_model,
            }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    state.catalog.invalidate().await;
    settings(state).await
}

/// 按 id 或用户名找用户（用户名不区分大小写、完全匹配）。
pub async fn find_user(state: &AiState, query: &str) -> Result<(i64, String), Problem> {
    let query = query.trim().trim_start_matches('@');
    let mut conn = state.db.acquire().await?;
    if let Ok(id) = query.parse::<i64>()
        && let Some(author) = ik_auth::service::authors(&mut conn, &[id])
            .await?
            .remove(&id)
    {
        return Ok((id, author.username));
    }
    ik_auth::interop::search_users(&mut conn, query, 20)
        .await?
        .into_iter()
        .find(|u| u.username.eq_ignore_ascii_case(query))
        .map(|u| (u.id, u.username))
        .ok_or_else(Problem::not_found)
}

/// 一个用户的全部记忆（MEM-031）。
pub async fn user_memories(state: &AiState, query: &str) -> Result<UserMemories, Problem> {
    let (user_id, username) = find_user(state, query).await?;
    Ok(UserMemories {
        user_id: Id::new(user_id),
        username,
        items: memory::user_memories(state, user_id).await?,
    })
}

/// 删一条记忆（MEM-032），记审计。
pub async fn delete(state: &AiState, op: &Operator, id: i64) -> Result<(), Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let row = repo::lock_memory(&mut tx, id)
        .await?
        .ok_or_else(Problem::not_found)?;
    repo::delete_memories(&mut tx, row.user_id, &[id]).await?;
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "ai.memory.delete",
            target_type: "ai_memory",
            target_id: Some(id.to_string()),
            details: serde_json::json!({
                "userId": row.user_id.to_string(),
                "kind": row.kind,
                "content": row.content,
            }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 清空一个用户的记忆（MEM-032）：指定角色只清与它的关系与事件记忆，否则清空全部。记审计。
pub async fn clear(
    state: &AiState,
    op: &Operator,
    request: ClearMemoriesRequest,
) -> Result<u64, Problem> {
    let ai_user_id = match request.character.as_deref() {
        Some(slug) => Some(memory::character_user(state, slug).await?),
        None => None,
    };
    let user_id = request.user_id.get();
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let deleted = repo::clear_memories(&mut tx, user_id, ai_user_id).await?;
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "ai.memory.clear",
            target_type: "user",
            target_id: Some(user_id.to_string()),
            details: serde_json::json!({
                "character": request.character,
                "deleted": deleted,
            }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(deleted)
}

// ---------- HTTP ----------

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    AiState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(get_memory_settings, put_memory_settings))
        .routes(routes!(list_user_memories))
        .routes(routes!(delete_memory))
        .routes(routes!(clear_user_memories))
}

/// AI 记忆设置与统计
#[utoipa::path(
    get,
    path = "/v1/admin/ai/memory",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = MemorySettingsView),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn get_memory_settings(
    State(state): State<AiState>,
    AdminOperator(_): AdminOperator,
) -> Result<Json<MemorySettingsView>, Problem> {
    Ok(Json(settings(&state).await?))
}

/// 修改 AI 记忆设置
///
/// 总开关、摘要与提炼用的模型、提炼延迟、跨会话衔接时长、事件向量的提供方与模型。
/// 本实例立即生效，其他实例最长 60 秒后生效。
#[utoipa::path(
    put,
    path = "/v1/admin/ai/memory",
    tag = "admin",
    request_body = MemorySettingsInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = MemorySettingsView),
        (status = 400, description = "VALIDATION_FAILED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_memory_settings(
    State(state): State<AiState>,
    AdminOperator(op): AdminOperator,
    Json(input): Json<MemorySettingsInput>,
) -> Result<Json<MemorySettingsView>, Problem> {
    Ok(Json(update_settings(&state, &op, input).await?))
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct UserQuery {
    /// 用户 id 或用户名（不区分大小写、完全匹配，可带 @）。
    pub user: String,
}

/// 某个用户的 AI 记忆
///
/// 置顶的在前，其余最近更新的在前。
#[utoipa::path(
    get,
    path = "/v1/admin/ai/memories",
    tag = "admin",
    params(UserQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = UserMemories),
        (status = 404, description = "没有这个用户", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn list_user_memories(
    State(state): State<AiState>,
    AdminOperator(_): AdminOperator,
    Query(query): Query<UserQuery>,
) -> Result<Json<UserMemories>, Problem> {
    Ok(Json(user_memories(&state, &query.user).await?))
}

/// 删除一条 AI 记忆
#[utoipa::path(
    delete,
    path = "/v1/admin/ai/memories/{id}",
    tag = "admin",
    params(("id" = String, Path, description = "记忆 id")),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已删除"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn delete_memory(
    State(state): State<AiState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id<AiMemory>>,
) -> Result<StatusCode, Problem> {
    delete(&state, &op, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 清空某个用户的 AI 记忆
///
/// 指定角色只清这个用户与它的关系与事件记忆；不指定清空这个用户的全部记忆（含档案）。不可恢复。
#[utoipa::path(
    post,
    path = "/v1/admin/ai/memories/clear",
    tag = "admin",
    request_body = ClearMemoriesRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204, description = "已清空"),
        (status = 404, description = "AI_CHARACTER_NOT_FOUND", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn clear_user_memories(
    State(state): State<AiState>,
    AdminOperator(op): AdminOperator,
    Json(request): Json<ClearMemoriesRequest>,
) -> Result<StatusCode, Problem> {
    clear(&state, &op, request).await?;
    Ok(StatusCode::NO_CONTENT)
}
