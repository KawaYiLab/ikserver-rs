//! 经济：经验与等级、丁尼钱包与流水、签到、投币记账、等级权益。
//!
//! 业务规则：`docs/business-rules/social-economy.md` 第 6–9 节。等级结果写回 `users.level`
//! （通过 auth 的 interop），供鉴权上下文读取。

pub mod benefits;
pub mod dto;
pub mod level;
mod repo;
mod routes;
pub mod service;
mod state;

pub use routes::router;
pub use state::EconomyState;
