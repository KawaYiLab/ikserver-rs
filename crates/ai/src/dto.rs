use chrono::{DateTime, NaiveDate, Utc};
use ik_kernel::Id;
use ik_kernel::id::marker::User;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::catalog::{Example, OutputMode};

/// 角色绑定的 AI 账号。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CharacterUser {
    #[schema(value_type = String)]
    pub id: Id<User>,
    pub username: String,
    /// 账号的展示头像（缩略图）。
    pub avatar_url: Option<String>,
}

/// 敲敲里可以对话的 AI 角色（AI-021）。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Character {
    pub slug: String,
    pub display_name: String,
    pub bio: Option<String>,
    /// 角色卡头像，没有时回落到绑定账号的头像。
    pub avatar_url: Option<String>,
    pub sort_order: i32,
    /// 角色卡里配的示例问题。
    pub suggested_questions: Vec<String>,
    /// 「大家都在问」：最近 30 天至少 3 个人问过的开场问题，问的人多的在前，最多 6 条（AI-028）。
    /// 已去掉和示例问题重复的；数据不够时为空或不足 6 条，客户端用示例问题补齐。
    pub popular_questions: Vec<String>,
    /// 角色卡默认模型（已停用或不在白名单内时为 null）。
    pub default_model_key: Option<String>,
    /// 允许选择的模型；null 表示不限制（全部启用模型）。
    pub allowed_model_keys: Option<Vec<String>>,
    pub user: CharacterUser,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CharacterList {
    pub items: Vec<Character>,
}

/// 可选模型（AI-022）。不含上游标识与倍率。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelView {
    pub key: String,
    pub display_name: String,
    pub description: String,
    pub badge: Option<String>,
    pub sort_order: i32,
    /// 全站默认模型。
    pub is_default: bool,
    /// 支持识图（私信里发的图片它能看到）。
    pub vision: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ModelList {
    pub items: Vec<ModelView>,
}

/// 今日额度（AI-023）。只给百分比，不给 token 数与上限。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuotaView {
    pub global_enabled: bool,
    /// 0–100。
    pub percent: u8,
    pub exhausted: bool,
    /// 没有个人上限。
    pub unlimited: bool,
    /// 下次重置（北京时间 04:00）。
    pub reset_at: DateTime<Utc>,
    /// false 表示用量读不到，客户端不要把 0 当真。
    pub available: bool,
}

/// 个人助手（AI-024，二期占位）。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AssistantView {
    pub enabled: bool,
    pub message: String,
}

// ---------- 导入（CLI；后台上线前用它维护配置） ----------

/// 一份完整的 AI 配置。模型按 key、角色卡按 slug 新增或更新；没列出的保持不动。
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportFile {
    #[serde(default)]
    pub settings: Option<SettingsInput>,
    #[serde(default)]
    pub models: Vec<ModelInput>,
    #[serde(default)]
    pub characters: Vec<CharacterInput>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsInput {
    pub global_enabled: bool,
    /// 每人每日加权 token，0 不限制。
    pub daily_tokens_per_user: i64,
    /// 全站每日加权 token，0 不限制。
    pub daily_tokens: i64,
}

/// 管理后台维护的上游提供方（OpenAI 兼容接口）。按 `name` 新增或更新。
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderInput {
    /// 模型的 `provider` 按它引用。小写字母、数字、下划线、连字符，1–64 字符；创建后不可改。
    pub name: String,
    pub display_name: String,
    /// 接口地址，例如 `https://api.deepseek.com/v1`（不含 `/chat/completions`）。
    pub base_url: String,
    /// 缺省或 null 保留原有密钥；空串表示不带密钥。
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub sort_order: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelInput {
    pub key: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub badge: Option<String>,
    /// 提供方名字：管理后台「提供方」里的，或部署配置里的（`IK_AI__PROVIDERS__<名字>__…`）。
    pub provider: String,
    /// 上游的模型名。
    pub provider_model: String,
    #[serde(default = "one")]
    pub cost_weight: f64,
    /// 支持识图（上游模型接受 `image_url` 内容块）。不支持的模型收到图片会报错，所以默认关。
    #[serde(default)]
    pub vision: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub sort_order: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CharacterInput {
    pub slug: String,
    pub display_name: String,
    #[serde(default)]
    pub bio: String,
    /// 头像的媒体 id（已上传的图片）。
    #[serde(default)]
    pub avatar_id: Option<String>,
    /// 绑定的账号（用户名）；导入时会被标为 AI 账号。
    pub username: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub knock_visible: bool,
    #[serde(default = "yes")]
    pub dm_enabled: bool,
    #[serde(default = "yes")]
    pub comment_enabled: bool,
    /// 允许在 QQ 渠道使用此角色，默认关闭（ADR 0008）。
    #[serde(default)]
    pub qq_enabled: bool,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default)]
    pub output_mode: OutputMode,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub examples: Vec<Example>,
    #[serde(default)]
    pub default_model_key: Option<String>,
    #[serde(default)]
    pub allowed_model_keys: Vec<String>,
    #[serde(default)]
    pub suggested_questions: Vec<String>,
    /// 提供剧情库工具（ADR 0004）。
    #[serde(default)]
    pub story_tools: bool,
    /// 启用的站外 MCP 服务（服务名）。
    #[serde(default)]
    pub mcp_servers: Vec<String>,
}

fn one() -> f64 {
    1.0
}

fn yes() -> bool {
    true
}

/// 导入结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportReport {
    pub models: usize,
    pub characters: usize,
}

// ---------- AI 会话 ----------

/// 新建 AI 会话（TRIG-024）。
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateSessionRequest {
    /// 角色（`/v1/ai/characters` 的 slug）。
    pub character: String,
    /// 标题，最多 64 字（超出截断）。
    #[serde(default)]
    pub title: Option<String>,
}

/// AI 会话的设置。OpenAPI 里叫 `AiSessionView`，与登录会话的 `SessionView` 区分。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = AiSessionView)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    #[schema(value_type = String)]
    pub conversation_id: Id<ik_kernel::id::marker::Conversation>,
    /// 角色已下线时为 null。
    pub character: Option<String>,
    pub title: Option<String>,
    /// 会话级模型选择；null 表示跟随角色卡默认。
    pub model_key: Option<String>,
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// 改 AI 会话（CTX-032、TRIG-025）。字段缺省表示不改；null 或空串表示清除。
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSessionRequest {
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>)]
    pub title: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>)]
    pub model_key: Option<Option<String>>,
}

/// 重新生成（GEN-013～015）。
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegenerateRequest {
    /// 只对这一次生效的模型；不改会话级选择。
    #[serde(default)]
    pub model_key: Option<String>,
}

/// 重新生成已受理：旧回复已撤回，新回复随后以 `dm.message.created`（`ai.state = streaming`）到达。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegenerateAccepted {
    #[schema(value_type = String)]
    pub replaced_message_id: Id<ik_kernel::id::marker::Message>,
}

// ---------- 记忆（ADR 0007） ----------

/// 记忆的种类：`profile` 是用户档案（所有角色共享），`relation` 是与某个角色之间的关系记忆，
/// `event` 是与某个角色聊过的具体经历（带日期，按话题检索，可以有待跟进日期）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    Profile,
    Relation,
    Event,
}

/// 敏感度：`public` 的可以在公开的评论区里用，`private` 的只在私信里用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MemorySensitivity {
    Private,
    Public,
}

/// 记忆的来源：后台从对话里提炼 / 对话里让 AI 记下 / 人工写入（目前没有入口，保留）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MemoryOrigin {
    Extracted,
    Told,
    Manual,
}

/// 一条记忆。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = AiMemoryView)]
#[serde(rename_all = "camelCase")]
pub struct MemoryView {
    #[schema(value_type = String)]
    pub id: Id<ik_kernel::id::marker::AiMemory>,
    pub kind: MemoryKind,
    /// 关系记忆所属的角色（slug）；档案、或角色卡已删除时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    /// 关系记忆所属角色的 AI 账号；档案时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub ai_user_id: Option<Id<User>>,
    pub content: String,
    /// 置顶的不会被自动整理改写或淘汰。
    pub pinned: bool,
    pub origin: MemoryOrigin,
    pub sensitivity: MemorySensitivity,
    /// 重要度 1–5（检索与淘汰用）。
    pub importance: i16,
    /// 事件发生的日期（北京时间）。只有事件有。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub happened_on: Option<NaiveDate>,
    /// 待跟进：到了这天，AI 会在下次聊天开场时问起。只有事件有。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow_up_on: Option<NaiveDate>,
    /// 已经问起过的时刻。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub followed_up_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 管理后台：某个用户的全部记忆（置顶的在前，其余最近更新的在前）。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = AdminUserMemories)]
#[serde(rename_all = "camelCase")]
pub struct UserMemories {
    #[schema(value_type = String)]
    pub user_id: Id<User>,
    pub username: String,
    pub items: Vec<MemoryView>,
}

/// 管理后台：清空某个用户的记忆。
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[schema(as = AdminClearMemoriesRequest)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClearMemoriesRequest {
    #[schema(value_type = String)]
    pub user_id: Id<User>,
    /// 指定角色：只清这个用户与它的关系与事件记忆；不指定：清空这个用户的全部记忆（含档案）。
    #[serde(default)]
    pub character: Option<String>,
}

/// 管理后台：AI 记忆的全站设置（MEM-030）。
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[schema(as = AdminMemorySettingsInput)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemorySettingsInput {
    /// 总开关：关掉后不读、不写、不整理，行为与没有记忆时一样；已有记忆保留。
    pub enabled: bool,
    /// 摘要、提炼、整理用的模型 key；null 用默认模型。
    #[serde(default)]
    pub model_key: Option<String>,
    /// 对话停下来多久后提炼长期记忆，0–86400 秒。
    pub extract_delay_secs: i32,
    /// 新会话带上同一角色其他会话里多久以内的最近几轮，0–720 小时，0 关闭。
    pub bridge_hours: i32,
    /// 事件向量的提供方名字（需支持 OpenAI 兼容的 `/embeddings`）；与 `embeddingModel` 同时为 null 时只按关键词检索。
    #[serde(default)]
    pub embedding_provider: Option<String>,
    /// 上游的向量模型名，如 `BAAI/bge-m3`。
    #[serde(default)]
    pub embedding_model: Option<String>,
}

/// 管理后台：AI 记忆的全站设置与统计。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = AdminMemorySettings)]
#[serde(rename_all = "camelCase")]
pub struct MemorySettingsView {
    pub enabled: bool,
    pub model_key: Option<String>,
    pub extract_delay_secs: i32,
    pub bridge_hours: i32,
    pub embedding_provider: Option<String>,
    pub embedding_model: Option<String>,
    /// 从没在后台改过（用的是默认值）时为 null。
    pub updated_at: Option<DateTime<Utc>>,
    pub stats: MemoryStats,
}

/// 记忆的全站统计。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = AdminMemoryStats)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStats {
    /// 有记忆的用户数。
    pub users: i64,
    pub profile: i64,
    pub relation: i64,
    pub event: i64,
    /// 还没问起的待跟进。
    pub pending_follow_ups: i64,
    /// 已经算了向量的记忆。
    pub with_embedding: i64,
}
