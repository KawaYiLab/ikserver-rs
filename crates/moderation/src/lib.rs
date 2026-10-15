//! 举报与审核（RPT、RPTADM、SANCTION）。
//!
//! 业务规则：`docs/business-rules/moderation-exam.md` 第 1、2、6 节。本 crate 拥有 `reports` 与
//! `moderation_log`；隐藏、删除内容经 content / comments 的 interop，封禁经 auth，通知经 notify，
//! 都在审核自己的事务里完成。审核接口在 `/v1/admin/reports*`，需要版主或管理员角色
//! （论坛账号的角色，见 ADR 0001；不再有独立的后台账号）。

pub mod admin;
pub mod dto;
mod repo;
mod routes;
pub mod service;
mod state;

pub use routes::router;
pub use state::ModerationState;
