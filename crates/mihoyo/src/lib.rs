//! 米游社扫码登录、绑定与解绑（MHY-001～020）。
//!
//! 业务规则：`docs/business-rules/auth-account.md` 第 7 节。本 crate 拥有 `mihoyo_bindings`（米游社身份）
//! 与 `mihoyo_qr_logins`（扫码会话），建号与签发会话经 `ik_auth`。上游协议见 [`upstream`]。
//! 扫码注销核验（delete 模式，MHY-016～018）随账号注销功能一起做。

pub mod device;
pub mod dto;
pub mod interop;
pub mod jobs;
mod repo;
mod routes;
pub mod service;
mod state;
pub mod upstream;

pub use routes::router;
pub use state::MihoyoState;
