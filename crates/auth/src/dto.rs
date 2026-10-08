//! 请求/响应类型（对外契约，进入 OpenAPI）。

use chrono::{DateTime, Utc};
use ik_kernel::Id;
use ik_kernel::Role;
use ik_kernel::id::marker::{Session, User};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub use crate::code::CodePurpose;

/// 令牌交付方式：网页用 HttpOnly Cookie；App 在响应体里拿令牌，之后用 `Authorization: Bearer`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    #[default]
    Web,
    App,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SendCodeRequest {
    #[schema(example = "alice@example.com")]
    pub email: String,
    pub purpose: CodePurpose,
}

/// 无论邮箱是否注册都返回同样的结构（登录、重置密码用途防账号枚举）。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SendCodeResponse {
    pub sent: bool,
    pub expires_in_secs: i64,
    pub cooldown_secs: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterRequest {
    pub email: String,
    #[schema(example = "042817")]
    pub code: String,
    pub password: String,
    #[serde(default)]
    pub client: ClientKind,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PasswordLoginRequest {
    /// 邮箱或用户名。
    pub identifier: String,
    pub password: String,
    #[serde(default)]
    pub client: ClientKind,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeLoginRequest {
    pub email: String,
    pub code: String,
    #[serde(default)]
    pub client: ClientKind,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetPasswordRequest {
    pub email: String,
    pub code: String,
    pub new_password: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponse {
    pub user: MeView,
    pub session: SessionView,
    /// 仅 `client = app` 时返回；网页端令牌只在 HttpOnly Cookie 里。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// 当前登录用户的自我视图（含邮箱，只返回给本人）。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeView {
    #[schema(value_type = String, example = "7385159247626227712")]
    pub id: Id<User>,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub role: Role,
    pub exam_passed: bool,
    pub is_ai_agent: bool,
    /// 等级 0–7（0 = 未通过入站考试）。
    pub level: i16,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    #[schema(value_type = String, example = "7385159247626227713")]
    pub id: Id<Session>,
    /// `password` | `email_code` | `register` | `mihoyo` | `password_reset`
    pub login_method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// 是否为发起本次请求的会话。
    pub current: bool,
}
