//! AI 配置的管理后台（media-ops.md ADMINCRUD：ai-model、ai-role-card、ai-agent-config；ADR 0001
//! 「人格 / 模型 / 额度全部进 /v1/admin」）、上游提供方与任务审计（AI-027 死信查看）。
//!
//! 全部只给管理员。校验与写入复用配置导入（[`crate::service::apply`]），与审计同一事务；
//! 提交后让本进程的配置缓存失效（其他实例最长 60 秒后生效，AI-002）。

use std::collections::BTreeSet;
use std::sync::LazyLock;
use std::time::Instant;

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
use crate::dto::{CharacterInput, ImportFile, ModelInput, ProviderInput, SettingsInput};
use crate::mcp::{
    self, AdminMcpServer, AdminMcpTool, McpRefreshResult, McpServerInput, McpToolInput,
};
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
    /// 可用的提供方名字：部署配置里的，加上后台启用的（模型的 `provider` 只能指向这些，
    /// 否则调用时退回默认模型）。
    pub providers: Vec<String>,
    /// 剧情库已加载语料：角色卡开了剧情库工具时才真正提供（ADR 0004）。
    pub story_available: bool,
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
    /// 支持识图（LLM-015）。
    pub vision: bool,
    pub enabled: bool,
    pub is_default: bool,
    pub sort_order: i32,
    /// `provider` 可用（部署配置里有，或后台里有且启用）。
    pub provider_configured: bool,
}

/// 提供方从哪来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSource {
    /// 管理后台维护，可改可删。
    Admin,
    /// 部署配置（`IK_AI__PROVIDERS__…`），后台只读。
    Config,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminProvider {
    pub name: String,
    pub display_name: String,
    pub base_url: String,
    /// 密钥末尾几位（如 `…a1b2`），没有密钥时为 null。密钥本身不回传。
    pub api_key_hint: Option<String>,
    pub enabled: bool,
    pub sort_order: i32,
    pub source: ProviderSource,
    /// 后台的这条与部署配置重名，实际生效的是部署配置里的。
    pub shadowed: bool,
    /// 引用它的模型 key。
    pub model_keys: Vec<String>,
}

/// 测试连接的结果：上游的 `GET /models`。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTestResult {
    pub ok: bool,
    /// 上游列出的模型名（按字母排序）。
    pub models: Vec<String>,
    /// 失败原因（上游的状态码与报错摘要）。
    pub error: Option<String>,
    pub elapsed_ms: u64,
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
    pub qq_enabled: bool,
    pub sort_order: i32,
    pub output_mode: OutputMode,
    pub system_prompt: String,
    pub examples: Vec<Example>,
    pub default_model_key: Option<String>,
    /// 空 = 全部启用模型。
    pub allowed_model_keys: Vec<String>,
    pub suggested_questions: Vec<String>,
    /// 提供剧情库工具（ADR 0004）。
    pub story_tools: bool,
    /// 启用的站外 MCP 服务（服务名）。
    pub mcp_servers: Vec<String>,
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

/// 可用的提供方名字：部署配置里的，加上后台启用的（与配置快照的合并规则一致）。
async fn usable_providers(
    state: &AiState,
    conn: &mut sqlx::PgConnection,
) -> Result<BTreeSet<String>, Problem> {
    let mut names: BTreeSet<String> = state.providers.names().into_iter().collect();
    names.extend(
        repo::providers(conn)
            .await?
            .into_iter()
            .filter(|p| p.enabled)
            .map(|p| p.name),
    );
    Ok(names)
}

pub async fn settings(state: &AiState) -> Result<AdminSettings, Problem> {
    let mut conn = state.db.acquire().await?;
    let row = repo::settings(&mut conn).await?;
    Ok(AdminSettings {
        global_enabled: row.as_ref().is_none_or(|r| r.global_enabled),
        daily_tokens_per_user: row.as_ref().map_or(1_000_000, |r| r.daily_tokens_per_user),
        daily_tokens: row.as_ref().map_or(0, |r| r.daily_tokens),
        providers: usable_providers(state, &mut conn)
            .await?
            .into_iter()
            .collect(),
        story_available: state
            .story
            .as_ref()
            .is_some_and(|story| story.current().is_some()),
    })
}

pub async fn models(state: &AiState) -> Result<Vec<AdminModel>, Problem> {
    let mut conn = state.db.acquire().await?;
    let usable = usable_providers(state, &mut conn).await?;
    Ok(repo::models(&mut conn)
        .await?
        .into_iter()
        .map(|m| AdminModel {
            provider_configured: usable.contains(&m.provider),
            key: m.key,
            display_name: m.display_name,
            description: m.description,
            badge: m.badge,
            provider: m.provider,
            provider_model: m.provider_model,
            cost_weight: m.cost_weight,
            vision: m.vision,
            enabled: m.enabled,
            is_default: m.is_default,
            sort_order: m.sort_order,
        })
        .collect())
}

/// 密钥只回末尾几位；太短的整个遮住。
fn key_hint(key: &str) -> Option<String> {
    let chars: Vec<char> = key.chars().collect();
    match chars.len() {
        0 => None,
        n if n < 12 => Some("…".to_owned()),
        n => Some(format!("…{}", chars[n - 4..].iter().collect::<String>())),
    }
}

/// 部署配置里的提供方 + 后台的提供方。部署配置里的排在前面。
pub async fn providers(state: &AiState) -> Result<Vec<AdminProvider>, Problem> {
    let mut conn = state.db.acquire().await?;
    let rows = repo::providers(&mut conn).await?;
    let models = repo::models(&mut conn).await?;
    drop(conn);
    let model_keys = |name: &str| -> Vec<String> {
        models
            .iter()
            .filter(|m| m.provider == name)
            .map(|m| m.key.clone())
            .collect()
    };
    let fixed = state.providers.names();
    let mut out: Vec<AdminProvider> = fixed
        .iter()
        .map(|name| {
            let config = state.config.providers.get(name);
            AdminProvider {
                name: name.clone(),
                display_name: name.clone(),
                base_url: config.map(|c| c.base_url.clone()).unwrap_or_default(),
                api_key_hint: config.and_then(|c| key_hint(c.api_key.expose())),
                enabled: true,
                sort_order: 0,
                source: ProviderSource::Config,
                shadowed: false,
                model_keys: model_keys(name),
            }
        })
        .collect();
    out.extend(rows.into_iter().map(|row| AdminProvider {
        shadowed: fixed.contains(&row.name),
        model_keys: model_keys(&row.name),
        api_key_hint: key_hint(&row.api_key),
        name: row.name,
        display_name: row.display_name,
        base_url: row.base_url,
        enabled: row.enabled,
        sort_order: row.sort_order,
        source: ProviderSource::Admin,
    }));
    Ok(out)
}

static PROVIDER_NAME: LazyLock<Option<regex::Regex>> =
    LazyLock::new(|| regex::Regex::new(r"^[a-z0-9][a-z0-9_-]{0,63}$").ok());

/// 校验并规整（去掉首尾空白与地址末尾的 `/`）。
fn validate_provider(state: &AiState, input: &mut ProviderInput) -> Result<(), Problem> {
    let field = |name: &str, detail: &str| Problem::field(name, "invalid", detail);
    if !PROVIDER_NAME
        .as_ref()
        .is_some_and(|re| re.is_match(&input.name))
    {
        return Err(field("name", "小写字母、数字、下划线、连字符，1–64 字符"));
    }
    if state.providers.contains(&input.name) {
        return Err(field("name", "与部署配置里的提供方重名"));
    }
    input.display_name = input.display_name.trim().to_owned();
    if !(1..=64).contains(&input.display_name.chars().count()) {
        return Err(field("displayName", "1–64 字"));
    }
    input.base_url = input.base_url.trim().trim_end_matches('/').to_owned();
    let has_host = input
        .base_url
        .strip_prefix("https://")
        .or_else(|| input.base_url.strip_prefix("http://"))
        .is_some_and(|rest| !rest.is_empty());
    if !has_host
        || input.base_url.chars().any(char::is_whitespace)
        || input.base_url.chars().count() > 512
    {
        return Err(field(
            "baseUrl",
            "以 http:// 或 https:// 开头的地址，最多 512 字符",
        ));
    }
    if let Some(key) = input.api_key.as_mut() {
        *key = key.trim().to_owned();
        if key.chars().count() > 4096 {
            return Err(field("apiKey", "最多 4096 字符"));
        }
    }
    Ok(())
}

/// 新建或修改后台的提供方（按名字）。不传密钥时保留原有的。
pub async fn save_provider(
    state: &AiState,
    op: &Operator,
    name: &str,
    mut input: ProviderInput,
) -> Result<AdminProvider, Problem> {
    if input.name != name {
        return Err(key_mismatch("name"));
    }
    validate_provider(state, &mut input)?;
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    repo::upsert_provider(
        &mut tx,
        &repo::NewProvider {
            id: state.ids.next_raw(),
            name: &input.name,
            display_name: &input.display_name,
            base_url: &input.base_url,
            api_key: input.api_key.as_deref(),
            enabled: input.enabled,
            sort_order: input.sort_order,
            now,
        },
    )
    .await?;
    // 密钥不进审计，只记有没有换。
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "ai.provider.save",
            target_type: "ai_provider",
            target_id: Some(name.to_owned()),
            details: serde_json::json!({
                "displayName": input.display_name,
                "baseUrl": input.base_url,
                "enabled": input.enabled,
                "apiKeyChanged": input.api_key.is_some(),
            }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    state.catalog.invalidate().await;
    providers(state)
        .await?
        .into_iter()
        .find(|p| p.source == ProviderSource::Admin && p.name == name)
        .ok_or_else(Problem::internal)
}

/// 删除后台的提供方。还有模型指向它时拒绝（与部署配置重名的除外：删了模型照样走部署配置）。
pub async fn delete_provider(state: &AiState, op: &Operator, name: &str) -> Result<(), Problem> {
    let shadowed = state.providers.contains(name);
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    // 只是防误删的提示：并发新建的模型若指向已删的提供方，调用时退回默认模型（LLM-011）。
    if !shadowed && !repo::models_using_provider(&mut tx, name).await?.is_empty() {
        return Err(Problem::conflict(
            "AI_PROVIDER_IN_USE",
            "还有模型在用这个提供方，请先把这些模型改到别的提供方或删除",
        ));
    }
    if !repo::delete_provider(&mut tx, name).await? {
        return Err(if shadowed {
            Problem::conflict(
                "AI_PROVIDER_FROM_CONFIG",
                "部署配置里的提供方只能在部署配置里修改",
            )
        } else {
            Problem::not_found()
        });
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "ai.provider.delete",
            target_type: "ai_provider",
            target_id: Some(name.to_owned()),
            details: serde_json::json!({}),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    state.catalog.invalidate().await;
    Ok(())
}

/// 用保存好的地址与密钥请求上游的 `GET /models`（同名时用部署配置里的，与调用一致）。
/// 停用的后台提供方也能测，方便启用前先确认。
pub async fn test_provider(state: &AiState, name: &str) -> Result<ProviderTestResult, Problem> {
    let (base_url, api_key) = if let Some(config) = state.config.providers.get(name) {
        (config.base_url.clone(), config.api_key.expose().to_owned())
    } else {
        let mut conn = state.db.acquire().await?;
        let row = repo::providers(&mut conn)
            .await?
            .into_iter()
            .find(|p| p.name == name)
            .ok_or_else(Problem::not_found)?;
        (row.base_url, row.api_key)
    };
    let started = Instant::now();
    let listed = crate::llm::list_models(&base_url, &api_key).await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(match listed {
        Ok(models) => ProviderTestResult {
            ok: true,
            models,
            error: None,
            elapsed_ms,
        },
        Err(error) => ProviderTestResult {
            ok: false,
            models: Vec::new(),
            error: Some(error.to_string()),
            elapsed_ms,
        },
    })
}

pub async fn characters(state: &AiState) -> Result<Vec<AdminCharacter>, Problem> {
    let mut conn = state.db.acquire().await?;
    let cards = repo::cards(&mut conn).await?;
    let models = repo::models(&mut conn).await?;
    let servers = repo::mcp_servers(&mut conn).await?;
    drop(conn);
    let key_of = |id: i64| models.iter().find(|m| m.id == id).map(|m| m.key.clone());
    let server_of = |id: i64| servers.iter().find(|s| s.id == id).map(|s| s.name.clone());
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
            qq_enabled: c.qq_enabled,
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
            story_tools: c.story_tools,
            mcp_servers: c.mcp_server_ids.into_iter().filter_map(server_of).collect(),
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
        "vision": input.vision,
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
        "qqEnabled": input.qq_enabled,
        "defaultModelKey": input.default_model_key,
        "allowedModelKeys": input.allowed_model_keys,
        "storyTools": input.story_tools,
        "mcpServers": input.mcp_servers,
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
        .routes(routes!(list_providers))
        .routes(routes!(put_provider, remove_provider))
        .routes(routes!(post_provider_test))
        .routes(routes!(list_models))
        .routes(routes!(put_model, remove_model))
        .routes(routes!(list_characters))
        .routes(routes!(put_character, remove_character))
        .routes(routes!(list_jobs))
        .routes(routes!(list_mcp_servers))
        .routes(routes!(put_mcp_server, remove_mcp_server))
        .routes(routes!(post_mcp_refresh))
        .routes(routes!(put_mcp_tool))
        .merge(crate::memory_admin::router())
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

/// AI 上游提供方（部署配置里的只读，后台维护的可改；不回传密钥）
#[utoipa::path(
    get,
    path = "/v1/admin/ai/providers",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = Vec<AdminProvider>))
)]
async fn list_providers(
    State(state): State<AiState>,
    AdminOperator(_): AdminOperator,
) -> Result<Json<Vec<AdminProvider>>, Problem> {
    Ok(Json(providers(&state).await?))
}

/// 新建或修改提供方
///
/// 不传 `apiKey`（或传 null）保留原有密钥。不能与部署配置里的提供方重名。
#[utoipa::path(
    put,
    path = "/v1/admin/ai/providers/{name}",
    tag = "admin",
    params(("name" = String, Path)),
    request_body = ProviderInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminProvider),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_provider(
    State(state): State<AiState>,
    AdminOperator(op): AdminOperator,
    Path(name): Path<String>,
    Json(input): Json<ProviderInput>,
) -> Result<Json<AdminProvider>, Problem> {
    Ok(Json(save_provider(&state, &op, &name, input).await?))
}

/// 删除提供方
///
/// 还有模型指向它时拒绝（409 `AI_PROVIDER_IN_USE`）；部署配置里的不能删（409 `AI_PROVIDER_FROM_CONFIG`）。
#[utoipa::path(
    delete,
    path = "/v1/admin/ai/providers/{name}",
    tag = "admin",
    params(("name" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn remove_provider(
    State(state): State<AiState>,
    AdminOperator(op): AdminOperator,
    Path(name): Path<String>,
) -> Result<StatusCode, Problem> {
    delete_provider(&state, &op, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 测试提供方连接并列出上游模型
///
/// 用已保存的地址与密钥请求上游的 `GET /models`；连不上时 `ok = false`，原因在 `error`。
#[utoipa::path(
    post,
    path = "/v1/admin/ai/providers/{name}/test",
    tag = "admin",
    params(("name" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ProviderTestResult),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn post_provider_test(
    State(state): State<AiState>,
    AdminOperator(_): AdminOperator,
    Path(name): Path<String>,
) -> Result<Json<ProviderTestResult>, Problem> {
    Ok(Json(test_provider(&state, &name).await?))
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

/// 站外 MCP 服务（含工具与审批状态；鉴权头只回提示）
#[utoipa::path(
    get,
    path = "/v1/admin/ai/mcp-servers",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = Vec<AdminMcpServer>))
)]
async fn list_mcp_servers(
    State(state): State<AiState>,
    AdminOperator(_): AdminOperator,
) -> Result<Json<Vec<AdminMcpServer>>, Problem> {
    Ok(Json(mcp::servers(&state).await?))
}

/// 新建或修改站外 MCP 服务
///
/// 不传 `authHeader`（或传 null）保留原有的。保存后用「拉取工具」取得工具清单，工具要逐个确认后才会提供给模型。
#[utoipa::path(
    put,
    path = "/v1/admin/ai/mcp-servers/{name}",
    tag = "admin",
    params(("name" = String, Path)),
    request_body = McpServerInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminMcpServer),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_mcp_server(
    State(state): State<AiState>,
    AdminOperator(op): AdminOperator,
    Path(name): Path<String>,
    Json(input): Json<McpServerInput>,
) -> Result<Json<AdminMcpServer>, Problem> {
    Ok(Json(mcp::save_server(&state, &op, &name, input).await?))
}

/// 删除站外 MCP 服务（工具一起删除，角色卡里去掉它）
#[utoipa::path(
    delete,
    path = "/v1/admin/ai/mcp-servers/{name}",
    tag = "admin",
    params(("name" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn remove_mcp_server(
    State(state): State<AiState>,
    AdminOperator(op): AdminOperator,
    Path(name): Path<String>,
) -> Result<StatusCode, Problem> {
    mcp::delete_server(&state, &op, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 拉取站外 MCP 服务的工具清单
///
/// 新工具默认不启用；定义变了的工具停止提供，等重新确认。连不上时 `ok = false`，原因在 `error`。
#[utoipa::path(
    post,
    path = "/v1/admin/ai/mcp-servers/{name}/refresh",
    tag = "admin",
    params(("name" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = McpRefreshResult),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn post_mcp_refresh(
    State(state): State<AiState>,
    AdminOperator(_): AdminOperator,
    Path(name): Path<String>,
) -> Result<Json<McpRefreshResult>, Problem> {
    Ok(Json(mcp::refresh_server(&state, &name).await?))
}

/// 启用、停用或重新确认一个 MCP 工具
///
/// 启用时带上后台看到的 `digest`：远端定义已经变了时拒绝（409 `AI_MCP_TOOL_CHANGED`）；
/// 只能启用只读工具（409 `AI_MCP_TOOL_NOT_READ_ONLY`）。
#[utoipa::path(
    put,
    path = "/v1/admin/ai/mcp-servers/{name}/tools/{tool}",
    tag = "admin",
    params(("name" = String, Path), ("tool" = String, Path, description = "远端工具名")),
    request_body = McpToolInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminMcpTool),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_mcp_tool(
    State(state): State<AiState>,
    AdminOperator(op): AdminOperator,
    Path((name, tool)): Path<(String, String)>,
    Json(input): Json<McpToolInput>,
) -> Result<Json<AdminMcpTool>, Problem> {
    Ok(Json(
        mcp::update_tool(&state, &op, &name, &tool, input).await?,
    ))
}
