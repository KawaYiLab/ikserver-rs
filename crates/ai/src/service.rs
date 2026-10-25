//! 公开读取（角色、模型、额度）与配置导入。

use std::collections::HashSet;
use std::sync::LazyLock;

use chrono::Utc;
use ik_auth::Principal;
use ik_kernel::{Id, Problem};

use crate::AiState;
use crate::catalog::{Catalog, RoleCard};
use crate::dto::{
    AssistantView, Character, CharacterInput, CharacterList, CharacterUser, ImportFile,
    ImportReport, ModelInput, ModelList, ModelView, QuotaView,
};
use crate::quota;
use crate::repo;

/// 敲敲角色列表（AI-020、AI-021）。绑定账号已不可用的角色不列出。
pub async fn characters(state: &AiState) -> Result<CharacterList, Problem> {
    let catalog = state.catalog.get().await?;
    let cards: Vec<&RoleCard> = catalog.visible_cards().collect();
    let user_ids: Vec<i64> = cards.iter().map(|c| c.bound_user_id).collect();
    let authors = ik_auth::service::authors(&state.db, &user_ids).await?;
    let user_avatars = state.media.avatar_urls(authors.values()).await?;
    let card_media: Vec<i64> = cards.iter().filter_map(|c| c.avatar_media_id).collect();
    let card_avatars = state.media.views(&card_media).await?;
    let items = cards
        .into_iter()
        .filter_map(|card| {
            let author = authors.get(&card.bound_user_id).filter(|a| !a.gone)?;
            let user_avatar = user_avatars.get(&card.bound_user_id).cloned();
            let avatar_url = card
                .avatar_media_id
                .and_then(|m| card_avatars.get(&m))
                .map(|v| v.thumb_url.clone())
                .or_else(|| user_avatar.clone());
            Some(character(&catalog, card, author, avatar_url, user_avatar))
        })
        .collect();
    Ok(CharacterList { items })
}

fn character(
    catalog: &Catalog,
    card: &RoleCard,
    author: &ik_auth::service::Author,
    avatar_url: Option<String>,
    user_avatar: Option<String>,
) -> Character {
    let default_model_key = card
        .default_model_id
        .and_then(|id| catalog.model_by_id(id))
        .filter(|m| card.allows(m))
        .map(|m| m.key.clone());
    let allowed_model_keys = (!card.allowed_model_ids.is_empty()).then(|| {
        catalog
            .allowed_models(card)
            .map(|m| m.key.clone())
            .collect()
    });
    Character {
        slug: card.slug.clone(),
        display_name: card.display_name.clone(),
        bio: (!card.bio.trim().is_empty()).then(|| card.bio.clone()),
        avatar_url,
        sort_order: card.sort_order,
        suggested_questions: card.suggested_questions.clone(),
        default_model_key,
        allowed_model_keys,
        user: CharacterUser {
            id: author.id,
            username: author.username.clone(),
            avatar_url: user_avatar,
        },
    }
}

/// 启用模型列表（AI-022）。
pub async fn models(state: &AiState) -> Result<ModelList, Problem> {
    let catalog = state.catalog.get().await?;
    let items = catalog
        .enabled_models()
        .map(|m| ModelView {
            key: m.key.clone(),
            display_name: if m.display_name.trim().is_empty() {
                m.key.clone()
            } else {
                m.display_name.clone()
            },
            description: m.description.clone(),
            badge: m.badge.clone(),
            sort_order: m.sort_order,
            is_default: m.is_default,
            vision: m.vision,
        })
        .collect();
    Ok(ModelList { items })
}

/// 今日额度（AI-023、QUOTA-031）。用量读不到时 `available = false`，不报错。
pub async fn quota(state: &AiState, principal: &Principal) -> Result<QuotaView, Problem> {
    let catalog = state.catalog.get().await?;
    let now = Utc::now();
    let limit = catalog.limits.per_user;
    let reading = async {
        let mut conn = state.db.acquire().await?;
        quota::read(&mut conn, principal.user_id.get(), now).await
    }
    .await;
    let (used, available) = match reading {
        Ok(reading) => (reading.used, true),
        Err(problem) => {
            tracing::warn!(?problem, "读取 AI 额度失败");
            (0, false)
        }
    };
    Ok(quota_view(
        catalog.limits.global_enabled,
        limit,
        used,
        available,
        now,
    ))
}

fn quota_view(
    global_enabled: bool,
    limit: i64,
    used: i64,
    available: bool,
    now: chrono::DateTime<Utc>,
) -> QuotaView {
    let unlimited = limit <= 0;
    let percent = if unlimited || !available {
        0
    } else {
        let scaled = (i128::from(used.max(0)) * 100 + i128::from(limit) / 2) / i128::from(limit);
        u8::try_from(scaled.min(100)).unwrap_or(100)
    };
    QuotaView {
        global_enabled,
        percent,
        exhausted: available && !unlimited && used >= limit,
        unlimited,
        reset_at: quota::reset_at(now),
        available,
    }
}

/// 个人助手（AI-024，二期占位）。
pub fn assistant() -> AssistantView {
    AssistantView {
        enabled: false,
        message: "个人助手功能即将上线".to_owned(),
    }
}

static MODEL_KEY: LazyLock<Option<regex::Regex>> =
    LazyLock::new(|| regex::Regex::new(r"^[a-z0-9][a-z0-9._-]{0,63}$").ok());
static SLUG: LazyLock<Option<regex::Regex>> =
    LazyLock::new(|| regex::Regex::new(r"^[a-z0-9][a-z0-9-]{0,63}$").ok());

fn matches(re: &LazyLock<Option<regex::Regex>>, text: &str) -> bool {
    re.as_ref().is_some_and(|re| re.is_match(text))
}

fn len_between(text: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&text.chars().count())
}

fn validate_model(index: usize, m: &ModelInput) -> Result<(), Problem> {
    let field = |name: &str, detail: &str| {
        Problem::field(&format!("models[{index}].{name}"), "invalid", detail)
    };
    if !matches(&MODEL_KEY, &m.key) {
        return Err(field(
            "key",
            "小写字母、数字、点、下划线、连字符，1–64 字符",
        ));
    }
    if !len_between(m.display_name.trim(), 1, 64) {
        return Err(field("displayName", "1–64 字"));
    }
    if m.badge.as_deref().is_some_and(|b| b.chars().count() > 16) {
        return Err(field("badge", "最多 16 字"));
    }
    if m.provider.trim().is_empty() || m.provider_model.trim().is_empty() {
        return Err(field("provider", "提供方与上游模型名不能为空"));
    }
    if !(m.cost_weight.is_finite() && m.cost_weight > 0.0 && m.cost_weight <= 1000.0) {
        return Err(field("costWeight", "须大于 0、不超过 1000"));
    }
    if m.is_default && !m.enabled {
        return Err(field("isDefault", "默认模型必须启用"));
    }
    Ok(())
}

fn validate_character(index: usize, c: &CharacterInput) -> Result<(), Problem> {
    let field = |name: &str, detail: &str| {
        Problem::field(&format!("characters[{index}].{name}"), "invalid", detail)
    };
    if !matches(&SLUG, &c.slug) {
        return Err(field("slug", "小写字母、数字、连字符，1–64 字符"));
    }
    if !len_between(c.display_name.trim(), 1, 64) {
        return Err(field("displayName", "1–64 字"));
    }
    if c.bio.chars().count() > 500 {
        return Err(field("bio", "最多 500 字"));
    }
    if c.system_prompt.chars().count() > 20_000 {
        return Err(field("systemPrompt", "最多 20000 字"));
    }
    if c.suggested_questions.len() > 10 {
        return Err(field("suggestedQuestions", "最多 10 条"));
    }
    Ok(())
}

/// 导入 AI 配置（CLI）。整份一个事务，有一项不合格就都不写入；写完让缓存失效。
pub async fn import(state: &AiState, file: ImportFile) -> Result<ImportReport, Problem> {
    let mut tx = state.db.begin().await?;
    let report = apply(state, &mut tx, &file).await?;
    tx.commit().await?;
    state.catalog.invalidate().await;
    Ok(report)
}

/// 校验并写入一份配置（导入与管理后台共用），在调用方的事务里。调用方提交后让缓存失效。
pub(crate) async fn apply(
    state: &AiState,
    tx: &mut sqlx::PgConnection,
    file: &ImportFile,
) -> Result<ImportReport, Problem> {
    for (index, model) in file.models.iter().enumerate() {
        validate_model(index, model)?;
    }
    if file.models.iter().filter(|m| m.is_default).count() > 1 {
        return Err(Problem::field("models", "invalid", "最多一个默认模型"));
    }
    for (index, character) in file.characters.iter().enumerate() {
        validate_character(index, character)?;
    }
    if let Some(settings) = &file.settings
        && (settings.daily_tokens_per_user < 0 || settings.daily_tokens < 0)
    {
        return Err(Problem::field("settings", "invalid", "上限不能为负数"));
    }
    let configured = repo::providers(tx).await?;
    for model in &file.models {
        let in_admin = configured
            .iter()
            .any(|p| p.enabled && p.name == model.provider);
        if !in_admin && !state.providers.contains(&model.provider) {
            tracing::warn!(
                key = %model.key,
                provider = %model.provider,
                "模型指向的提供方没有配置，调用时会退回默认模型"
            );
        }
    }

    let now = Utc::now();
    if let Some(settings) = &file.settings {
        repo::upsert_settings(
            tx,
            settings.global_enabled,
            settings.daily_tokens_per_user,
            settings.daily_tokens,
            now,
        )
        .await?;
    }
    if file.models.iter().any(|m| m.is_default) {
        repo::clear_default_model(tx).await?;
    }
    for model in &file.models {
        repo::upsert_model(
            tx,
            &repo::NewModel {
                id: state.ids.next_raw(),
                key: &model.key,
                display_name: model.display_name.trim(),
                description: &model.description,
                badge: model.badge.as_deref(),
                provider: &model.provider,
                provider_model: &model.provider_model,
                cost_weight: model.cost_weight,
                vision: model.vision,
                enabled: model.enabled,
                is_default: model.is_default,
                sort_order: model.sort_order,
                now,
            },
        )
        .await?;
    }
    let models = repo::models(tx).await?;
    let model_id = |index: usize, key: &str| {
        models
            .iter()
            .find(|m| m.key == key)
            .map(|m| m.id)
            .ok_or_else(|| {
                Problem::field(
                    &format!("characters[{index}]"),
                    "unknown_model",
                    format!("模型 {key} 不存在"),
                )
            })
    };
    let mcp_servers = if file.characters.iter().any(|c| !c.mcp_servers.is_empty()) {
        repo::mcp_servers(tx).await?
    } else {
        Vec::new()
    };
    let mut seen = HashSet::new();
    for (index, c) in file.characters.iter().enumerate() {
        if !seen.insert(c.slug.as_str()) {
            return Err(Problem::field(
                &format!("characters[{index}].slug"),
                "duplicate",
                "slug 重复",
            ));
        }
        let default_model_id = c
            .default_model_key
            .as_deref()
            .map(|key| model_id(index, key))
            .transpose()?;
        let allowed_model_ids = c
            .allowed_model_keys
            .iter()
            .map(|key| model_id(index, key))
            .collect::<Result<Vec<_>, _>>()?;
        let mut mcp_server_ids = Vec::with_capacity(c.mcp_servers.len());
        for name in &c.mcp_servers {
            let id = mcp_servers
                .iter()
                .find(|s| &s.name == name)
                .map(|s| s.id)
                .ok_or_else(|| {
                    Problem::field(
                        &format!("characters[{index}].mcpServers"),
                        "unknown_mcp_server",
                        format!("MCP 服务 {name} 不存在"),
                    )
                })?;
            if !mcp_server_ids.contains(&id) {
                mcp_server_ids.push(id);
            }
        }
        let avatar_media_id = match c.avatar_id.as_deref() {
            None => None,
            Some(raw) => {
                let id: Id = raw.parse().map_err(|_| {
                    Problem::field(
                        &format!("characters[{index}].avatarId"),
                        "invalid",
                        "不是合法的 id",
                    )
                })?;
                if state.media.views(&[id.get()]).await?.is_empty() {
                    return Err(Problem::field(
                        &format!("characters[{index}].avatarId"),
                        "not_found",
                        "图片不存在",
                    ));
                }
                Some(id.get())
            }
        };
        let Some(user_id) = ik_auth::interop::mark_ai_agent(tx, &c.username, now).await? else {
            return Err(Problem::field(
                &format!("characters[{index}].username"),
                "not_found",
                "账号不存在或不可用",
            ));
        };
        let examples = serde_json::to_value(&c.examples).map_err(|e| Problem::internal_from(&e))?;
        let questions: Vec<String> = c
            .suggested_questions
            .iter()
            .map(|q| q.trim().to_owned())
            .filter(|q| !q.is_empty())
            .collect();
        repo::upsert_card(
            tx,
            &repo::NewCard {
                id: state.ids.next_raw(),
                slug: &c.slug,
                display_name: c.display_name.trim(),
                bio: &c.bio,
                avatar_media_id,
                bound_user_id: user_id,
                enabled: c.enabled,
                knock_visible: c.knock_visible,
                dm_enabled: c.dm_enabled,
                comment_enabled: c.comment_enabled,
                sort_order: c.sort_order,
                output_mode: c.output_mode.as_str(),
                system_prompt: &c.system_prompt,
                examples: &examples,
                default_model_id,
                allowed_model_ids: &allowed_model_ids,
                suggested_questions: &questions,
                forum_content_tools: c.forum_content_tools,
                forum_user_tools: c.forum_user_tools,
                forum_browse_tools: c.forum_browse_tools,
                story_tools: c.story_tools,
                mcp_server_ids: &mcp_server_ids,
                now,
            },
        )
        .await?;
    }
    if !models.iter().any(|m| m.is_default && m.enabled) && !models.is_empty() {
        tracing::warn!("没有默认模型：角色卡白名单内都不可用时 AI 不会回复");
    }
    Ok(ImportReport {
        models: file.models.len(),
        characters: file.characters.len(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use chrono::TimeZone;

    use super::*;

    #[test]
    fn quota_view_follows_ai_023() {
        let now = Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
        let v = quota_view(true, 1000, 125, true, now);
        assert_eq!((v.percent, v.exhausted, v.unlimited), (13, false, false));
        let v = quota_view(true, 1000, 1000, true, now);
        assert_eq!((v.percent, v.exhausted), (100, true));
        let v = quota_view(true, 1000, 5000, true, now);
        assert_eq!(v.percent, 100);
        let v = quota_view(true, 0, 5000, true, now);
        assert_eq!((v.percent, v.exhausted, v.unlimited), (0, false, true));
        let v = quota_view(true, 1000, 0, false, now);
        assert_eq!((v.percent, v.exhausted, v.available), (0, false, false));
    }
}
