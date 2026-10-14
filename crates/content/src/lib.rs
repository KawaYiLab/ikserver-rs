//! 内容：分区、标签、帖子（草稿、发布、编辑、下线、删除、置顶、浏览量、最新信息流）。
//!
//! 业务规则：`docs/business-rules/posts.md`。评论在后续的 comments 模块。

pub mod anonymous;
pub mod dto;
pub mod interop;
mod repo;
mod routes;
pub mod service;
mod state;
mod tags;
mod video;

pub use routes::router;
pub use state::ContentState;
pub use video::ExternalVideo;
