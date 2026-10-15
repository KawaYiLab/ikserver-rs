//! 私信：一对一会话、消息、编辑与撤回、输入中提示。
//!
//! 业务规则：`docs/business-rules/messaging-notifications.md` 第 1～3 节。本 crate 只依赖 auth、social，
//! 位于 notify 之下：对方发来的通知要在通知的事务里推进私聊会话的排序（[`interop::touch_direct`]），
//! 统一收件箱（私聊会话与通知分组混排、已读）由 notify 组合本 crate 的 [`interop`] 实现。
//! 群聊（D-073）与图片消息（D-021）首版不实现。

pub mod dto;
pub mod interop;
mod repo;
mod routes;
pub mod service;
mod state;

pub use routes::router;
pub use state::MessagingState;
