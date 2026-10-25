//! 评论：扁平的楼层（每条评论一层，可以回复任意一条，ADR 0006）、置顶、采纳、@提及。
//!
//! 业务规则：`docs/business-rules/comments.md`。帖子计数、顶帖与采纳投影通过
//! `ik_content::interop` 在同一事务里更新。发表评论时经 [`CommentHooks`] 通知 ik-ai（AI 评论触发）；
//! AI 的回复经 [`interop::insert_ai_reply`] 写入。

pub mod admin;
pub mod dto;
pub mod interop;
mod repo;
mod routes;
pub mod service;
mod state;
pub mod text;

pub use routes::router;
pub use state::{CommentHooks, CommentsState, CreatedComment, ReplyToRef};
