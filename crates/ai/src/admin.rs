//! AI 配置的管理后台（media-ops.md ADMINCRUD：ai-model、ai-role-card、ai-agent-config；ADR 0001
//! 「人格 / 模型 / 额度全部进 /v1/admin」）与任务审计（AI-027 死信查看）。
//!
//! 全部只给管理员。校验与写入复用配置导入（[`crate::service::apply`]），与审计同一事务；
//! 提交后让本进程的配置缓存失效（其他实例最长 60 秒后生效，AI-002）。

use axum::extract::{FromRef, State};
use chrono::{DateTime, Utc};
use http::StatusCode;
use ik_auth::admin::{self, Audit};
use ik_auth::{AdminOperator, AuthState, Operator};
use ik_kernel::cursor::{Paged, page_window};
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::{Id, Problem};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AiState;
use crate::catalog::{Example, OutputMode};
use crate::dto::{CharacterInput, ImportFile, ModelInput, SettingsInput};
use crate::repo::{self, JobFilter};
use crate::service::apply;

// ---------- 视图 ----------

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminSettings {
    pub global_enabled: bool,
    /// 每人每日加权 token，0 不限制。
    pub daily_tokens_per_user: i64,
    /// 全站每日加权 token，0 不限制。
    pub daily_tokens: i64,
    /// 部署配置里已有的提供方名字（模型的 `provider` 只能指向这些，否则调用时退回默认模型）。
    pub providers: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminModel {
    pub key: String,
    pub display_name: String,
    pub description: String,
    pub badge: Option<String>,
    pub provider: String,
    pub provider_model: String,
    pub cost_weight: f64,
    pub enabled: bool,
    pub is_default: bool,
    pub sort_order: i32,
    /// `provider` 在部署配置里存在。
    pub provider_configured: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminCharacter {
    pub slug: String,
    pub display_name: String,
    pub bio: String,
    #[schema(value_type = Option<String>)]
    pub avatar_id: Option<Id>,
    /// 绑定账号的用户名（账号已不存在时为空）。
    pub username: Option<String>,
    #[schema(value_type = String)]
    pub user_id: Id,
    pub enabled: bool,
    pub knock_visible: bool,
    pub dm_enabled: bool,
    pub comment_enabled: bool,
    pub sort_order: i32,
    pub output_mode: OutputMode,
    pub system_prompt: String,
    pub examples: Vec<Example>,
    pub default_model_key: Option<String>,
    /// 空 = 全部启用模型。
    pub allowed_model_keys: Vec<String>,
    pub suggested_questions: Vec<String>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct JobQuery {
    /// `queued` | `processing` | `completed` | `failed` | `skipped`；`failed` 即死信。
    pub status: Option<String>,
    /// `dm` | `comment`。
    pub kind: Option<String>,
    pub page: Option<u32>,
    /// 默认 50，最大 100。
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminJob {
    #[schema(value_type = String)]
    pub id: Id,
    pub kind: String,
    pub status: String,
    /// 失败、跳过的原因（TRIG-036）。
    pub reason: Option<String>,
    #[schema(value_type = String)]
    pub user_id: Id,
    #[schema(value_type = String)]
    pub ai_user_id: Id,
    #[schema(value_type = Option<String>)]
    pub conversation_id: Option<Id>,
    #[schema(value_type = Option<String>)]
    pub post_id: Option<Id>,
    pub attempts: i16,
    pub model_key: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------- 服务 ----------

pub async fn settings(state: &AiState) -> Result<AdminSettings, Problem> {
    let mut conn = state.db.acquire().await?;
    let row = repo::settings(&mut conn).await?;
    Ok(AdminSettings {
        global_enabled: row.as_ref().is_none_or(|r| r.global_enabled),
        daily_tokens_per_user: row.as_ref().map_or(1_000_000, |r| r.daily_tokens_per_user),
        daily_tokens: row.as_ref().map_or(0, |r| r.daily_tokens),
        providers: state.providers.names(),
    })
}

pub async fn models(state: &AiState) -> Result<Vec<AdminModel>, Problem> {
    let mut conn = state.db.acquire().await?;
    Ok(repo::models(&mut conn)
        .await?
        .into_iter()
        .map(|m| AdminModel {
            provider_configured: state.providers.contains(&m.provider),
            key: m.key,
            display_name: m.display_name,
            description: m.description,
            badge: m.badge,
            provider: m.provider,
            provider_model: m.provider_model,
            cost_weight: m.cost_weight,
            enabled: m.enabled,
            is_default: m.is_default,
            sort_order: m.sort_order,
        })
        .collect())
}

pub async fn characters(state: &AiState) -> Result<Vec<AdminCharacter>, Problem> {
    let mut conn = state.db.acquire().await?;
    let cards = repo::cards(&mut conn).await?;
    let models = repo::models(&mut conn).await?;
    drop(conn);
    let key_of = |id: i64| models.iter().find(|m| m.id == id).map(|m| m.key.clone());
    let user_ids: Vec<i64> = cards.iter().map(|c| c.bound_user_id).collect();
    let authors = ik_auth::service::authors(&state.db, &user_ids).await?;
    Ok(cards
        .into_iter()
        .map(|c| AdminCharacter {
            username: authors.get(&c.bound_user_id).map(|a| a.username.clone()),
            user_id: Id::new(c.bound_user_id),
            slug: c.slug,
            display_name: c.display_name,
            bio: c.bio,
            avatar_id: c.avatar_media_id.map(Id::new),
            enabled: c.enabled,
            knock_visible: c.knock_visible,
            dm_enabled: c.dm_enabled,
            comment_enabled: c.comment_enabled,
            sort_order: c.sort_order,
            output_mode: if c.output_mode == "markdown" {
                OutputMode::Markdown
            } else {
                OutputMode::Plain
            },
            system_prompt: c.system_prompt,
            examples: serde_json::from_value(c.examples).unwrap_or_default(),
            default_model_key: c.default_model_id.and_then(key_of),
            allowed_model_keys: c.allowed_model_ids.into_iter().filter_map(key_of).collect(),
            suggested_questions: c.suggested_questions,
        })
        .collect())
}

/// 校验 + 写入 + 审计，一个事务；提交后让缓存失效。
async fn save(
    state: &AiState,
    op: &Operator,
    file: ImportFile,
    audit: Audit<'_>,
) -> Result<(), Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    apply(state, &mut tx, &file).await?;
    admin::record(&mut tx, &state.ids, op, audit, now).await?;
    tx.commit().await?;
    state.catalog.invalidate().await;
    Ok(())
}

pub async fn update_settings(
    state: &AiState,
    op: &Operator,
    input: SettingsInput,
) -> Result<AdminSettings, Problem> {
    let details = serde_json::json!({
        "globalEnabled": input.global_enabled,
        "dailyTokensPerUser": input.daily_tokens_per_user,
        "dailyTokens": input.daily_tokens,
    });
    save(
        state,
        op,
        ImportFile {
            settings: Some(input),
            models: Vec::new(),
            characters: Vec::new(),
        },
        Audit {
            action: "ai.settings.update",
            target_type: "ai_settings",
            target_id: None,
            details,
        },
    )
    .await?;
    settings(state).await
}

fn key_mismatch(field: &str) -> Problem {
    Problem::field(field, "MISMATCH", "请求体里的标识与路径不一致")
}

/// 新建或修改模型（按 key）。设为默认会取消原来的默认模型。
pub async fn save_model(
    state: &AiState,
    op: &Operator,
    key: &str,
    input: ModelInput,
) -> Result<AdminModel, Problem> {
    if input.key != key {
        return Err(key_mismatch("key"));
    }
    let details = serde_json::json!({
        "displayName": input.display_name,
        "provider": input.provider,
        "providerModel": input.provider_model,
        "costWeight": input.cost_weight,
        "enabled": input.enabled,
        "isDefault": input.is_default,
    });
    save(
        state,
        op,
        ImportFile {
            settings: None,
            models: vec![input],
            characters: Vec::new(),
        },
        Audit {
            action: "ai.model.save",
            target_type: "ai_model",
            target_id: Some(key.to_owned()),
            details,
        },
    )
    .await?;
    models(state)
        .await?
        .into_iter()
        .find(|m| m.key == key)
        .ok_or_else(Problem::internal)
}

/// 删除模型。默认模型不能删（先把别的模型设为默认）；引用它的角色卡默认模型置空、白名单去掉它。
pub async fn delete_model(state: &AiState, op: &Operator, key: &str) -> Result<(), Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let Some((id, is_default)) = repo::model_by_key(&mut tx, key).await? else {
        return Err(Problem::not_found());
    };
    if is_default {
        return Err(Problem::conflict(
            "AI_MODEL_IS_DEFAULT",
            "默认模型不能删除，请先把其他模型设为默认",
        ));
    }
    repo::delete_model(&mut tx, id).await?;
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "ai.model.delete",
            target_type: "ai_model",
            target_id: Some(key.to_owned()),
            details: serde_json::json!({}),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    state.catalog.invalidate().await;
    Ok(())
}

/// 新建或修改角色卡（按 slug）。绑定的账号会被标为 AI 账号。
pub async fn save_character(
    state: &AiState,
    op: &Operator,
    slug: &str,
    input: CharacterInput,
) -> Result<AdminCharacter, Problem> {
    if input.slug != slug {
        return Err(key_mismatch("slug"));
    }
    let details = serde_json::json!({
        "displayName": input.display_name,
        "username": input.username,
        "enabled": input.enabled,
        "dmEnabled": input.dm_enabled,
        "commentEnabled": input.comment_enabled,
        "defaultModelKey": input.default_model_key,
        "allowedModelKeys": input.allowed_model_keys,
        "systemPromptChars": input.system_prompt.chars().count(),
    });
    save(
        state,
        op,
        ImportFile {
            settings: None,
            models: Vec::new(),
            characters: vec![input],
        },
        Audit {
            action: "ai.character.save",
            target_type: "ai_character",
            target_id: Some(slug.to_owned()),
            details,
        },
    )
    .await?;
    characters(state)
        .await?
        .into_iter()
        .find(|c| c.slug == slug)
        .ok_or_else(Problem::internal)
}

/// 删除角色卡：这个 AI 账号不再响应任何触发；账号本身与历史会话保留。
pub async fn delete_character(state: &AiState, op: &Operator, slug: &str) -> Result<(), Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    if !repo::delete_card(&mut tx, slug).await? {
        return Err(Problem::not_found());
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "ai.character.delete",
            target_type: "ai_character",
            target_id: Some(slug.to_owned()),
            details: serde_json::json!({}),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    state.catalog.invalidate().await;
    Ok(())
}

pub async fn jobs(state: &AiState, query: JobQuery) -> Result<Paged<AdminJob>, Problem> {
    let (page, limit, offset) = page_window(query.page, query.limit, 50);
    let clean = |v: Option<String>| v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let filter = JobFilter {
        status: clean(query.status),
        kind: clean(query.kind),
    };
    let mut conn = state.db.acquire().await?;
    let total = repo::count_jobs(&mut conn, &filter).await?;
    let rows = repo::list_jobs(&mut conn, &filter, i64::from(limit), offset).await?;
    Ok(Paged {
        items: rows
            .into_iter()
            .map(|r| AdminJob {
                id: Id::new(r.id),
                kind: r.kind,
                status: r.status,
                reason: r.reason,
                user_id: Id::new(r.quota_user_id),
                ai_user_id: Id::new(r.ai_user_id),
                conversation_id: r.conversation_id.map(Id::new),
                post_id: r.post_id.map(Id::new),
                attempts: r.attempts,
                model_key: r.model_key,
                prompt_tokens: r.prompt_tokens,
                completion_tokens: r.completion_tokens,
                created_at: r.created_at,
                updated_at: r.updated_at,
            })
            .collect(),
        total,
        page,
        limit,
    })
}

/// AI 今日概况（管理后台仪表盘）。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AiOverview {
    pub global_enabled: bool,
    /// 全站今日已用加权 token（含在飞预占）。
    pub site_used_today: i64,
    /// 全站每日上限，0 不限制。
    pub site_limit: i64,
    /// 最近 24 小时最终失败的任务数。
    pub failed_last_24h: i64,
}

pub async fn overview(state: &AiState) -> Result<AiOverview, Problem> {
    let now = Utc::now();
    let current = settings(state).await?;
    let mut conn = state.db.acquire().await?;
    let used = crate::quota::read(&mut conn, crate::quota::SITE, now)
        .await?
        .used;
    let failed = repo::failed_jobs_since(&mut conn, now - chrono::Duration::hours(24)).await?;
    Ok(AiOverview {
        global_enabled: current.global_enabled,
        site_used_today: used,
        site_limit: current.daily_tokens,
        failed_last_24h: failed,
    })
}

// ---------- HTTP ----------

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    AiState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(get_settings, put_settings))
        .routes(routes!(list_models))
        .routes(routes!(put_model, remove_model))
        .routes(routes!(list_characters))
        .routes(routes!(put_character, remove_character))
        .routes(routes!(list_jobs))
}

/// AI 全局设置
#[utoipa::path(
    get,
    path = "/v1/admin/ai/settings",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminSettings),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn get_settings(
    State(state): State<AiState>,
    AdminOperator(_): AdminOperator,
) -> Result<Json<AdminSettings>, Problem> {
    Ok(Json(settings(&state).await?))
}

/// 修改 AI 全局设置（全局开关、每人与全站每日额度）
#[utoipa::path(
    put,
    path = "/v1/admin/ai/settings",
    tag = "admin",
    request_body = SettingsInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminSettings),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_settings(
    State(state): State<AiState>,
    AdminOperator(op): AdminOperator,
    Json(input): Json<SettingsInput>,
) -> Result<Json<AdminSettings>, Problem> {
    Ok(Json(update_settings(&state, &op, input).await?))
}

/// AI 模型（含停用的，含上游路由与计费倍率）
#[utoipa::path(
    get,
    path = "/v1/admin/ai/models",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = Vec<AdminModel>))
)]
async fn list_models(
    State(state): State<AiState>,
    AdminOperator(_): AdminOperator,
) -> Result<Json<Vec<AdminModel>>, Problem> {
    Ok(Json(models(&state).await?))
}

/// 新建或修改模型
#[utoipa::path(
    put,
    path = "/v1/admin/ai/models/{key}",
    tag = "admin",
    params(("key" = String, Path)),
    request_body = ModelInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminModel),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_model(
    State(state): State<AiState>,
    AdminOperator(op): AdminOperator,
    Path(key): Path<String>,
    Json(input): Json<ModelInput>,
) -> Result<Json<AdminModel>, Problem> {
    Ok(Json(save_model(&state, &op, &key, input).await?))
}

/// 删除模型
///
/// 默认模型不能删除（409 `AI_MODEL_IS_DEFAULT`）。
#[utoipa::path(
    delete,
    path = "/v1/admin/ai/models/{key}",
    tag = "admin",
    params(("key" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn remove_model(
    State(state): State<AiState>,
    AdminOperator(op): AdminOperator,
    Path(key): Path<String>,
) -> Result<StatusCode, Problem> {
    delete_model(&state, &op, &key).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// AI 角色卡（含人格、示例对话、模型白名单）
#[utoipa::path(
    get,
    path = "/v1/admin/ai/characters",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = Vec<AdminCharacter>))
)]
async fn list_characters(
    State(state): State<AiState>,
    AdminOperator(_): AdminOperator,
) -> Result<Json<Vec<AdminCharacter>>, Problem> {
    Ok(Json(characters(&state).await?))
}

/// 新建或修改角色卡
#[utoipa::path(
    put,
    path = "/v1/admin/ai/characters/{slug}",
    tag = "admin",
    params(("slug" = String, Path)),
    request_body = CharacterInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminCharacter),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_character(
    State(state): State<AiState>,
    AdminOperator(op): AdminOperator,
    Path(slug): Path<String>,
    Json(input): Json<CharacterInput>,
) -> Result<Json<AdminCharacter>, Problem> {
    Ok(Json(save_character(&state, &op, &slug, input).await?))
}

/// 删除角色卡
#[utoipa::path(
    delete,
    path = "/v1/admin/ai/characters/{slug}",
    tag = "admin",
    params(("slug" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn remove_character(
    State(state): State<AiState>,
    AdminOperator(op): AdminOperator,
    Path(slug): Path<String>,
) -> Result<StatusCode, Problem> {
    delete_character(&state, &op, &slug).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// AI 任务审计（`status=failed` 即死信，AI-027）
#[utoipa::path(
    get,
    path = "/v1/admin/ai/jobs",
    tag = "admin",
    params(JobQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = Paged<AdminJob>))
)]
async fn list_jobs(
    State(state): State<AiState>,
    AdminOperator(_): AdminOperator,
    Query(query): Query<JobQuery>,
) -> Result<Json<Paged<AdminJob>>, Problem> {
    Ok(Json(jobs(&state, query).await?))
}
