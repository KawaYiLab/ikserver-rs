//! 信息流：首页（置顶 + 热门位 + 最新流）、热门榜、关注流、未看流、阅读记录。
//!
//! 业务规则：`docs/business-rules/feed-search.md`。热榜需要点赞（content）与评论（comments）两类活动，
//! 所以放在两者之上；各种流的 SQL 仍在 content（帖子表归它所有），本 crate 只负责热度计算与组合。

pub mod hot;
mod routes;
pub mod service;
mod state;

pub use routes::router;
pub use state::FeedState;
