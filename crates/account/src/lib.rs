//! 账号安全：安全概览、注销账号（DEL-001～010）。
//!
//! 业务规则：`docs/business-rules/auth-account.md` 第 9 节。注销要在一个事务里同时改 auth（脱敏、吊销会话）、
//! mihoyo（删除绑定、消费扫码凭证）、profile（清空资料），所以放在它们之上的这个 crate 里编排。
//! 只改 auth 自己的表的功能（绑定邮箱、修改密码）在 `ik_auth`。

pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
