//! 全局配置、提供方、模型、角色卡的进程内快照（AI-002、AI-004：缓存 60 秒，写入时主动失效）。
//!
//! 多实例部署时别的实例最多晚 60 秒看到后台的改动。

use std::sync::Arc;
use std::time::Duration;

use ik_kernel::Problem;
use moka::future::Cache;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::llm::{ChatProvider, GenParams, OpenAiCompatible, Providers};
use crate::quota::{self, Limits};
use crate::repo;

pub const CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct Model {
    pub id: i64,
    pub key: String,
    pub display_name: String,
    pub description: String,
    pub badge: Option<String>,
    pub provider: String,
    pub provider_model: String,
    pub cost_weight: f64,
    /// 支持识图（LLM-015）：用户消息里的图片原样发给它；不支持的只看到「[图片]」占位。
    pub vision: bool,
    pub enabled: bool,
    pub is_default: bool,
    pub sort_order: i32,
    /// 生成参数（LLM-016）。
    pub params: ModelParams,
}

impl Model {
    pub fn weight(&self) -> f64 {
        quota::sane_weight(self.cost_weight)
    }

    /// 一次用量的计费额度：命中缓存的输入按折扣算，再乘模型倍率（QUOTA-001、LLM-017）。
    pub fn charge(&self, usage: crate::llm::Usage) -> i64 {
        quota::weighted(
            quota::billable_tokens(usage, self.cached_weight()),
            self.weight(),
        )
    }

    /// 上下文窗口（token，LLM-018）。
    pub fn context_tokens(&self) -> i64 {
        i64::from(self.params.context_tokens.unwrap_or(DEFAULT_CONTEXT_TOKENS))
    }

    /// 命中缓存的输入按原价的几成计入额度（LLM-017）。
    pub fn cached_weight(&self) -> f64 {
        self.params
            .cached_weight
            .filter(|w| w.is_finite() && (0.0..=1.0).contains(w))
            .unwrap_or(DEFAULT_CACHED_WEIGHT)
    }

    /// 发给上游的采样参数：角色卡填了的覆盖模型的（LLM-016）。
    pub fn gen_params(&self, card: Option<&CardParams>) -> GenParams {
        GenParams {
            temperature: card.and_then(|c| c.temperature).or(self.params.temperature),
            top_p: card.and_then(|c| c.top_p).or(self.params.top_p),
            max_tokens: self.params.max_output_tokens,
            extra: self.params.extra_body.clone(),
        }
    }
}

/// 没填上下文窗口时按这么多 token 算（主流对话模型都不小于它）。
pub const DEFAULT_CONTEXT_TOKENS: u32 = 65_536;
/// 没填时命中缓存的输入按原价的两成计入额度（各家缓存价大多是原价的一到两成多，取偏高的）。
pub const DEFAULT_CACHED_WEIGHT: f64 = 0.2;

/// 模型的生成参数（LLM-016，`ai_models.params`）。不填的项用上游默认值或内置默认值。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ModelParams {
    /// 0～2。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// (0, 1]。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// 单次请求的输出上限（`max_tokens`）。不填由上游决定。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// 上下文窗口（token），用来裁剪历史与工具结果（LLM-018）。不填按 65536。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u32>,
    /// 命中缓存的输入按原价的几成计入额度，0～1（LLM-017）。不填按 0.2。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_weight: Option<f64>,
    /// 原样合进请求体的上游扩展字段，例如思考开关 `{"enable_thinking": false}`。
    #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
    #[schema(value_type = Object)]
    pub extra_body: serde_json::Map<String, serde_json::Value>,
}

/// 角色卡覆盖模型的采样参数（`ai_role_cards.params`）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct CardParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
}

/// 输出模式（AI-007）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    #[default]
    Plain,
    Markdown,
}

impl OutputMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Markdown => "markdown",
        }
    }

    fn parse(raw: &str) -> Self {
        if raw == "markdown" {
            Self::Markdown
        } else {
            Self::Plain
        }
    }
}

/// 人格的示例对话（GEN-071）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Example {
    pub user: String,
    pub assistant: String,
}

#[derive(Debug, Clone)]
pub struct RoleCard {
    pub id: i64,
    pub slug: String,
    pub display_name: String,
    pub bio: String,
    pub avatar_media_id: Option<i64>,
    pub bound_user_id: i64,
    pub enabled: bool,
    pub knock_visible: bool,
    pub dm_enabled: bool,
    pub comment_enabled: bool,
    pub qq_enabled: bool,
    pub sort_order: i32,
    pub output_mode: OutputMode,
    pub system_prompt: String,
    pub examples: Vec<Example>,
    pub default_model_id: Option<i64>,
    /// 空 = 全部启用模型。
    pub allowed_model_ids: Vec<i64>,
    pub suggested_questions: Vec<String>,
    /// 提供剧情库工具（ADR 0004）。
    pub story_tools: bool,
    /// 启用的站外 MCP 服务。
    pub mcp_server_ids: Vec<i64>,
    /// 覆盖模型的采样参数。
    pub params: CardParams,
}

impl RoleCard {
    pub fn allows(&self, model: &Model) -> bool {
        model.enabled
            && (self.allowed_model_ids.is_empty() || self.allowed_model_ids.contains(&model.id))
    }
}

/// 站外 MCP 服务（启用的）与它生效的工具（ADR 0004）。
#[derive(Debug, Clone)]
pub struct McpServer {
    pub id: i64,
    pub name: String,
    pub display_name: String,
    pub url: String,
    pub auth_header: String,
    pub timeout: Duration,
    pub tools: Vec<McpTool>,
}

/// 管理员确认过的工具定义（运行时用它，不用远端实时返回的）。
#[derive(Debug, Clone)]
pub struct McpTool {
    pub exposed_name: String,
    pub remote_name: String,
    /// 时间线文案：后台填的，否则 MCP 的 title。
    pub label: Option<String>,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// 一份快照。模型、角色卡都按 `sort_order, id` 排好。
#[derive(Debug, Clone)]
pub struct Catalog {
    pub limits: Limits,
    /// 可用的上游：部署配置里的，加上后台启用的（同名以部署配置为准）。
    pub providers: Providers,
    pub models: Vec<Model>,
    pub cards: Vec<RoleCard>,
    /// 启用的站外 MCP 服务（含生效的工具）。
    pub mcp_servers: Vec<Arc<McpServer>>,
    /// AI 记忆的全站设置（ADR 0007，管理后台「AI 记忆」页）。
    pub memory: MemoryConfig,
    /// 保存调用记录（OBS-002）。
    pub trace_enabled: bool,
}

/// AI 记忆的全站设置（MEM-030）。没有配置行时按默认值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConfig {
    /// 总开关：关掉后不读、不写、不整理。
    pub enabled: bool,
    /// 摘要、提炼、整理用的模型 key；None 或不可用时用默认模型。
    pub model_key: Option<String>,
    /// 对话停下来多久后提炼（秒，MEM-020）。
    pub extract_delay_secs: i64,
    /// 跨会话衔接的时长（小时，0 关闭，MEM-005）。
    pub bridge_hours: i64,
    /// 事件向量：（提供方名字, 上游的向量模型名），None 时只按关键词检索（MEM-052）。
    pub embedding: Option<(String, String)>,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model_key: None,
            extract_delay_secs: 180,
            bridge_hours: 48,
            embedding: None,
        }
    }
}

impl Catalog {
    pub fn enabled_models(&self) -> impl Iterator<Item = &Model> {
        self.models.iter().filter(|m| m.enabled)
    }

    /// 启用的模型（按 key）。
    pub fn model(&self, key: &str) -> Option<&Model> {
        self.enabled_models().find(|m| m.key == key)
    }

    pub fn model_by_id(&self, id: i64) -> Option<&Model> {
        self.models.iter().find(|m| m.id == id)
    }

    /// 默认模型（D-005 方案 A：必须是一条启用的模型记录）。
    pub fn default_model(&self) -> Option<&Model> {
        self.enabled_models().find(|m| m.is_default)
    }

    /// 预占时不知道模型时用的倍率：启用模型里最大的（QUOTA-008）。
    pub fn max_weight(&self) -> f64 {
        self.enabled_models()
            .map(Model::weight)
            .fold(None, |acc: Option<f64>, w| {
                Some(acc.map_or(w, |a| a.max(w)))
            })
            .unwrap_or(1.0)
    }

    /// 某个 AI 账号的角色卡：启用的、按排序取第一张（AI-006）。
    pub fn card_for_user(&self, user_id: i64) -> Option<&RoleCard> {
        self.cards
            .iter()
            .find(|c| c.enabled && c.bound_user_id == user_id)
    }

    pub fn card_by_slug(&self, slug: &str) -> Option<&RoleCard> {
        self.cards.iter().find(|c| c.slug == slug)
    }

    /// 敲敲角色列表（AI-020）。
    pub fn visible_cards(&self) -> impl Iterator<Item = &RoleCard> {
        self.cards.iter().filter(|c| c.enabled && c.knock_visible)
    }

    /// 角色卡白名单内的启用模型（白名单空 = 全部启用模型）。
    pub fn allowed_models<'a>(&'a self, card: &'a RoleCard) -> impl Iterator<Item = &'a Model> {
        self.enabled_models().filter(move |m| card.allows(m))
    }

    /// 单次回复的模型（CTX-030）：单次覆盖 → 会话级选择 → 角色卡默认 → 白名单内第一个启用模型 →
    /// 全站默认模型。任何一级不合法都静默跳过。评论回复不传会话级选择（CTX-031）。
    pub fn resolve_model<'a>(
        &'a self,
        card: &'a RoleCard,
        override_key: Option<&str>,
        session_key: Option<&str>,
    ) -> Option<&'a Model> {
        let pick = |key: Option<&str>| key.and_then(|k| self.model(k)).filter(|m| card.allows(m));
        pick(override_key)
            .or_else(|| pick(session_key))
            .or_else(|| {
                card.default_model_id
                    .and_then(|id| self.model_by_id(id))
                    .filter(|m| card.allows(m))
            })
            .or_else(|| self.allowed_models(card).next())
            .or_else(|| self.default_model())
    }
}

/// 进程内缓存。
#[derive(Clone)]
pub struct CatalogCache {
    db: PgPool,
    /// 部署配置里的提供方（启动时建好，不随后台改动）。
    fixed: Providers,
    cache: Cache<(), Arc<Catalog>>,
}

impl CatalogCache {
    pub fn new(db: PgPool, fixed: Providers) -> Self {
        Self {
            db,
            fixed,
            cache: Cache::builder()
                .max_capacity(1)
                .time_to_live(CACHE_TTL)
                .build(),
        }
    }

    pub async fn get(&self) -> Result<Arc<Catalog>, Problem> {
        let db = self.db.clone();
        let fixed = self.fixed.clone();
        self.cache
            .try_get_with((), async move { load(&db, fixed).await.map(Arc::new) })
            .await
            .map_err(|e| (*e).clone())
    }

    /// 后台改了配置、提供方、模型、角色卡之后调用。
    pub async fn invalidate(&self) {
        self.cache.invalidate(&()).await;
    }
}

async fn load(db: &PgPool, fixed: Providers) -> Result<Catalog, Problem> {
    let mut conn = db.acquire().await?;
    // 配置行不存在时按默认值（AI-002）。
    let settings = repo::settings(&mut conn).await?;
    let trace_enabled = settings.as_ref().is_some_and(|row| row.trace_enabled);
    let limits = settings.map_or(
        Limits {
            global_enabled: true,
            per_user: 1_000_000,
            site: 0,
        },
        |row| Limits {
            global_enabled: row.global_enabled,
            per_user: row.daily_tokens_per_user,
            site: row.daily_tokens,
        },
    );
    let mut providers = fixed;
    for row in repo::providers(&mut conn).await? {
        if !row.enabled {
            continue;
        }
        match OpenAiCompatible::connect(&row.base_url, &row.api_key) {
            Ok(client) => {
                let client: Arc<dyn ChatProvider> = Arc::new(client);
                providers = providers.or_with(row.name, client);
            }
            // 建不出客户端（TLS 初始化失败）时这一家不可用，模型退回默认（LLM-011）。
            Err(error) => tracing::error!(provider = %row.name, %error, "提供方客户端创建失败"),
        }
    }
    let models = repo::models(&mut conn)
        .await?
        .into_iter()
        .map(|row| Model {
            id: row.id,
            key: row.key,
            display_name: row.display_name,
            description: row.description,
            badge: row.badge,
            provider: row.provider,
            provider_model: row.provider_model,
            cost_weight: row.cost_weight,
            vision: row.vision,
            enabled: row.enabled,
            is_default: row.is_default,
            sort_order: row.sort_order,
            // 坏数据按没填处理，不阻塞生成。
            params: serde_json::from_value(row.params).unwrap_or_default(),
        })
        .collect();
    let cards = repo::cards(&mut conn)
        .await?
        .into_iter()
        .map(|row| RoleCard {
            id: row.id,
            slug: row.slug,
            display_name: row.display_name,
            bio: row.bio,
            avatar_media_id: row.avatar_media_id,
            bound_user_id: row.bound_user_id,
            enabled: row.enabled,
            knock_visible: row.knock_visible,
            dm_enabled: row.dm_enabled,
            comment_enabled: row.comment_enabled,
            qq_enabled: row.qq_enabled,
            sort_order: row.sort_order,
            output_mode: OutputMode::parse(&row.output_mode),
            system_prompt: row.system_prompt,
            // 坏数据按没有示例处理，不阻塞生成（GEN-072）。
            examples: serde_json::from_value(row.examples).unwrap_or_default(),
            default_model_id: row.default_model_id,
            allowed_model_ids: row.allowed_model_ids,
            suggested_questions: row
                .suggested_questions
                .into_iter()
                .filter(|q| !q.trim().is_empty())
                .collect(),
            story_tools: row.story_tools,
            mcp_server_ids: row.mcp_server_ids,
            params: serde_json::from_value(row.params).unwrap_or_default(),
        })
        .collect();
    let mcp_servers = load_mcp(&mut conn).await?;
    let memory =
        repo::memory_settings(&mut conn)
            .await?
            .map_or_else(MemoryConfig::default, |row| MemoryConfig {
                enabled: row.enabled,
                model_key: row.model_key,
                extract_delay_secs: i64::from(row.extract_delay_secs),
                bridge_hours: i64::from(row.bridge_hours),
                embedding: row.embedding_provider.zip(row.embedding_model),
            });
    Ok(Catalog {
        limits,
        providers,
        models,
        cards,
        mcp_servers,
        memory,
        trace_enabled,
    })
}

async fn load_mcp(conn: &mut sqlx::PgConnection) -> Result<Vec<Arc<McpServer>>, Problem> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Definition {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        description: String,
        #[serde(default)]
        input_schema: serde_json::Value,
    }
    let mut tools = repo::active_mcp_tools(conn).await?;
    let mut out = Vec::new();
    for row in repo::mcp_servers(conn).await? {
        if !row.enabled {
            continue;
        }
        let server_tools = tools
            .extract_if(.., |t| t.server_id == row.id)
            .filter_map(|t| {
                // 坏数据跳过这个工具，不阻塞生成。
                let def: Definition = serde_json::from_value(t.approved).ok()?;
                let input_schema = if def.input_schema.is_object() {
                    def.input_schema
                } else {
                    serde_json::json!({ "type": "object", "properties": {} })
                };
                Some(McpTool {
                    exposed_name: t.exposed_name,
                    remote_name: t.remote_name,
                    label: t
                        .timeline_text
                        .filter(|l| !l.trim().is_empty())
                        .or(def.title),
                    description: def.description,
                    input_schema,
                })
            })
            .collect();
        out.push(Arc::new(McpServer {
            id: row.id,
            name: row.name,
            display_name: row.display_name,
            url: row.url,
            auth_header: row.auth_header,
            timeout: Duration::from_millis(u64::try_from(row.timeout_ms).unwrap_or(15_000)),
            tools: server_tools,
        }));
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn model(id: i64, key: &str, enabled: bool, is_default: bool, weight: f64) -> Model {
        Model {
            id,
            key: key.into(),
            display_name: key.into(),
            description: String::new(),
            badge: None,
            provider: "p".into(),
            provider_model: key.into(),
            cost_weight: weight,
            vision: false,
            enabled,
            is_default,
            sort_order: 0,
            params: ModelParams::default(),
        }
    }

    pub(crate) fn card(default_model_id: Option<i64>, allowed: Vec<i64>) -> RoleCard {
        RoleCard {
            id: 1,
            slug: "fairy".into(),
            display_name: "Fairy".into(),
            bio: String::new(),
            avatar_media_id: None,
            bound_user_id: 7,
            enabled: true,
            knock_visible: true,
            dm_enabled: true,
            comment_enabled: true,
            qq_enabled: false,
            sort_order: 0,
            output_mode: OutputMode::Plain,
            system_prompt: String::new(),
            examples: Vec::new(),
            default_model_id,
            allowed_model_ids: allowed,
            suggested_questions: Vec::new(),
            story_tools: false,
            mcp_server_ids: Vec::new(),
            params: CardParams::default(),
        }
    }

    fn catalog(models: Vec<Model>) -> Catalog {
        Catalog {
            limits: Limits {
                global_enabled: true,
                per_user: 0,
                site: 0,
            },
            providers: Providers::default(),
            models,
            cards: Vec::new(),
            mcp_servers: Vec::new(),
            memory: MemoryConfig::default(),
            trace_enabled: false,
        }
    }

    #[test]
    fn model_resolution_follows_ctx_030() {
        let c = catalog(vec![
            model(1, "base", true, true, 1.0),
            model(2, "pro", true, false, 3.0),
            model(3, "off", false, false, 9.0),
            model(4, "lite", true, false, 0.5),
        ]);
        let open = card(Some(2), vec![]);
        let key = |m: Option<&Model>| m.map(|m| m.key.clone());
        // 单次覆盖优先。
        assert_eq!(
            key(c.resolve_model(&open, Some("lite"), Some("base"))),
            Some("lite".into())
        );
        // 停用的覆盖与会话选择静默跳过，落到角色卡默认。
        assert_eq!(
            key(c.resolve_model(&open, Some("off"), Some("nope"))),
            Some("pro".into())
        );
        assert_eq!(
            key(c.resolve_model(&open, None, Some("base"))),
            Some("base".into())
        );
        // 白名单：默认模型出白名单被忽略，取白名单内第一个启用模型。
        let limited = card(Some(2), vec![4, 3]);
        assert_eq!(
            key(c.resolve_model(&limited, Some("pro"), None)),
            Some("lite".into())
        );
        // 白名单里都停用了：落到全站默认模型。
        let dead = card(None, vec![3]);
        assert_eq!(key(c.resolve_model(&dead, None, None)), Some("base".into()));
        assert_eq!(c.max_weight(), 3.0);
        assert_eq!(catalog(vec![]).max_weight(), 1.0);
        assert!(catalog(vec![]).resolve_model(&open, None, None).is_none());
    }
}
