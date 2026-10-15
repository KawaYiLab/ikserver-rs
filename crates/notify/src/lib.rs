//! 通知（敲敲）：写入、撤回、新帖扇出、列表、按发送者分组、已读。
//!
//! 业务规则：`docs/business-rules/messaging-notifications.md` 第 4、5 节。
//! 本 crate 位于 content、comments 之下：各域在自己的事务里调用 [`interop`] 写通知，
//! 实时推送随调用方的 `Effects` 在提交后发出。通知里存帖子标题与评论摘要的快照，
//! 不读其他域的表。

pub mod dto;
pub mod interop;
mod repo;
mod routes;
pub mod service;
mod state;

pub use routes::router;
pub use state::NotifyState;
