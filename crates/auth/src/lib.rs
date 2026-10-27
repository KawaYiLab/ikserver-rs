//! 账号与认证：邮箱验证码、注册、密码/验证码登录、重置密码、会话（不透明令牌）。
//!
//! 业务规则：`docs/business-rules/auth-account.md`。

pub mod admin;
mod code;
pub mod dto;
mod extract;
pub mod guard;
pub mod interop;
pub mod lifecycle;
mod password;
mod repo;
mod routes;
pub mod service;
mod state;
mod token;
pub mod validate;

pub use admin::{AdminOperator, Operator, StaffOperator};
pub use extract::{
    AuthRateLimit, ClientMeta, CurrentUser, MaybeUser, Principal, UserIfPresent, credential_from,
};
pub use routes::{deliver, forget_session, router};
pub use state::{AuthSettings, AuthState};
