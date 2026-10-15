//! 个人资料与作者主页：签名、隐藏主页、改名（收费）、主页精选、作者主页（资料 / 帖子 / 评论）、用户搜索。
//!
//! 业务规则：`docs/business-rules/social-economy.md` 第 11 节。本 crate 位于最上层：
//! 账号信息来自 auth，关注与拉黑来自 social，经验与扣费来自 economy，帖子与评论列表来自 content、comments。
//! 头像、名片（COSMETIC）随 media 模块加入。

pub mod dto;
mod repo;
mod routes;
pub mod service;
mod state;

pub use routes::router;
pub use state::ProfileState;
