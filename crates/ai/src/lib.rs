//! AI 助手：全局配置、模型、角色卡、每日额度、大模型调用（ai-assistant.md）。
//!
//! 业务规则：`docs/business-rules/ai-assistant.md`。直连 OpenAI 兼容的上游（[`llm`]），工具调用循环
//! 在进程内完成；额度账本在 PostgreSQL（[`quota`]）。配置（模型、角色卡、上限）在数据库里，
//! 进程内缓存 60 秒（[`catalog`]）；上游地址与密钥在管理后台（`ai_providers`）或部署配置里
//! （`ik_kernel::config::Ai`，同名时以部署配置为准）。

pub mod admin;
pub mod catalog;
pub mod comment;
pub mod dm;
pub mod dto;
mod generate;
pub mod immersion;
pub mod jobs;
pub mod llm;
pub mod mcp;
pub mod mcp_client;
pub mod memory;
pub mod memory_jobs;
pub mod quota;
mod repo;
mod routes;
pub mod sanitize;
pub mod service;
pub mod sessions;
mod state;
pub mod toolbox;
pub mod tools;
pub mod workflow;

pub use routes::router;
pub use state::{AiState, Forum};
