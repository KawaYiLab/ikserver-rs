//! 内容：分区、标签、帖子（草稿、发布、编辑、下线、删除、置顶、浏览量、最新信息流），
//! 以及帖子上的点赞、收藏、投币、一键三连。
//!
//! 业务规则：`docs/business-rules/posts.md`、`docs/business-rules/social-economy.md` 第 1、2、5 节。
//! 评论在 comments 模块，关注与拉黑在 social 模块，通知在 notify 模块。

pub mod admin;
pub mod anonymous;
pub mod bilibili;
pub mod dto;
pub mod interop;
pub mod jobs;
mod repo;
mod routes;
pub mod service;
mod state;
mod tags;
mod video;

pub use routes::router;
pub use state::ContentState;
pub use video::ExternalVideo;

/// 里区的 slug（POST-122）：这个分区的帖子只在选中它时出现，不进首页、热门、关注流、全站搜索等。
pub const INNER_CATEGORY_SLUG: &str = "inner";
