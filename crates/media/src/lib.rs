//! 媒体：直传上传（签名、完成校验与发布）、媒体文件、我的图库、CDN 缩略图地址。
//!
//! 业务规则：`docs/business-rules/media-ops.md` 第 1～5 节。本 crate 只依赖 auth；引用媒体的业务域
//! （个人资料的头像，以后的帖子封面、评论图片）经 [`interop`] 校验归属、经 [`MediaState::views`] 取展示数据。

pub mod cdn;
pub mod dto;
pub mod gate;
pub mod interop;
pub mod jobs;
mod repo;
mod routes;
pub mod service;
pub mod sniff;
mod state;
pub mod storage;
pub mod thumb;
pub mod token;

pub use routes::router;
pub use state::MediaState;
