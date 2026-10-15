//! 装扮：系统头像、论坛名片（COSMETIC-001～009）。
//!
//! 业务规则：`docs/business-rules/social-economy.md` 第 10 节。目录（`system_avatars`、`business_cards`）
//! 归本 crate；装备状态是账号上的列，经 `ik_auth::interop` 写入。系统头像的图片 id 冗余在账号上，
//! 所以其他域展示作者时不必依赖本 crate（见 `ik_auth::service::Author::avatar_media_id`）。
//! 所有头像、名片免费、对所有人开放；装备不要求通过考试（COMMON-006）。

pub mod dto;
mod repo;
mod routes;
pub mod service;
mod state;

pub use routes::router;
pub use state::CosmeticsState;
