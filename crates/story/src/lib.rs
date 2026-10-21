//! 绝区零剧情库：把 zzz-story-extractor 提取的主线剧情、过场字幕/漫画与世界观设定做成可检索、可顺读的
//! 只读知识库，以 MCP（Model Context Protocol，Streamable HTTP 无状态模式）挂在 `POST /v1/story/mcp`，
//! 给剧情总结 AI 用。
//!
//! 语料以「语料包」（[`corpus::Bundle`]，gzip 压缩的 JSON）存在 `story_corpora` 表里，管理员用
//! `PUT /v1/admin/story/corpus` 上传（zzz-story-extractor 的 `python -m zzz_story upload` 一键完成），
//! 各实例在内存里构建索引：启动时加载生效版本（[`StoryState::load`]），上传后立即切换，
//! 其他实例一分钟内跟上（[`StoryState::spawn_watcher`]）。
//!
//! 分层：`corpus`（数据模型、语料包、构建）→ `index`（字符二元组倒排）→ `engine`（六个工具的逻辑，
//! 产出类型化结果与兼容文本）→ `mcp`（JSON-RPC 分发）→ `routes` / `admin`（HTTP）；`repo` 读写表。

pub mod admin;
pub mod corpus;
pub mod de;
pub mod dto;
pub mod engine;
pub mod index;
pub mod mcp;
mod repo;
mod routes;
mod state;
pub mod text;

pub use routes::{mcp_router, router};
pub use state::{Loaded, StoryState};

use std::path::Path;

/// 本地语料：zzz-story-extractor 的输出目录，或 `python -m zzz_story upload --save` 存下的语料包文件。
/// 生产环境的语料在数据库里（[`StoryState::load`]），这条路径给本地开发与 `story call` 用。
pub fn load_source(path: &Path) -> anyhow::Result<corpus::Corpus> {
    if path.is_dir() {
        corpus::build_from_dir(path)
    } else {
        let bytes = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("读取 {} 失败：{e}", path.display()))?;
        corpus::build_from_bundle(&corpus::Bundle::decode(&bytes, MAX_BUNDLE_BYTES)?)
    }
}

/// 语料包解压后的大小上限（当前全量数据约 45 MB）。
pub const MAX_BUNDLE_BYTES: usize = 512 * 1024 * 1024;
