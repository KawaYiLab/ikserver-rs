use chrono::{DateTime, Utc};
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
    pub suggested_questions: Vec<String>,
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

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelInput {
    pub key: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub badge: Option<String>,
    /// 配置里的提供方名字（`IK_AI__PROVIDERS__<名字>__…`）。
    pub provider: String,
    /// 上游的模型名。
    pub provider_model: String,
    #[serde(default = "one")]
    pub cost_weight: f64,
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
