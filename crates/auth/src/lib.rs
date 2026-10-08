//! 账号与认证：邮箱验证码、注册、密码/验证码登录、重置密码、会话（不透明令牌）。
//!
//! 业务规则：`docs/business-rules/auth-account.md`。

mod code;
pub mod dto;
mod extract;
mod password;
mod repo;
mod routes;
pub mod service;
mod state;
mod token;
mod validate;

pub use extract::{AuthRateLimit, ClientMeta, CurrentUser, MaybeUser, Principal};
pub use routes::router;
pub use state::{AuthSettings, AuthState};
