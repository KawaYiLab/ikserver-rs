//! 社交关系：关注、拉黑。
//!
//! 业务规则：`docs/business-rules/social-economy.md` 第 3、4 节。本 crate 只依赖 auth，
//! 位于 content、comments 之下，这样发帖、评论、投币、点赞都能在自己的事务里做拉黑判断
//! （见 [`interop`]）。点赞、收藏、三连是帖子与评论上的数据，分别归 content、comments 所有。

pub mod dto;
pub mod interop;
mod repo;
mod routes;
pub mod service;
mod state;

pub use routes::router;
pub use state::SocialState;
