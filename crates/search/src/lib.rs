//! 搜索：帖子与评论全文搜索、标题联想、Meilisearch 索引同步。
//!
//! 业务规则：`docs/business-rules/feed-search.md` 3.9～3.11、4 节。本 crate 在 content、comments 之上，
//! 只经它们的 interop / service 读数据（不读它们的表）。搜索引擎只找候选与排序，返回内容一律回查数据库；
//! 引擎不可用、或命中全是索引残留时走数据库兜底（见 [`service`]）。用户搜索在 profile。

pub mod dto;
mod engine;
mod indexer;
mod routes;
pub mod service;
mod state;
pub mod text;

pub use engine::PostSort;
pub use routes::router;
pub use state::SearchState;
