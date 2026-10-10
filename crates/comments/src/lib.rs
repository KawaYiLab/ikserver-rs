//! 评论：楼层、楼中楼（一层）、置顶、采纳、@提及。
//!
//! 业务规则：`docs/business-rules/comments.md`。帖子计数、顶帖与采纳投影通过
//! `ik_content::interop` 在同一事务里更新。

pub mod dto;
mod repo;
mod routes;
pub mod service;
mod state;
pub mod text;

pub use routes::router;
pub use state::CommentsState;
