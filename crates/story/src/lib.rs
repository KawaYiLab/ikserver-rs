//! 绝区零剧情库：把 zzz-story-extractor 提取的主线剧情、过场字幕/漫画与世界观设定做成可检索、可顺读的
//! 只读知识库，以 MCP（Model Context Protocol，Streamable HTTP 无状态模式）挂在 `POST /v1/story/mcp`，
//! 给剧情总结 AI 用。
//!
//! 不读写数据库，只依赖 kernel。分层：`corpus`（数据模型、构建、快照）→ `index`（字符二元组倒排）
//! → `engine`（五个工具的逻辑，产出给模型读的紧凑文本）→ `mcp`（JSON-RPC 分发）→ `routes`（HTTP）。
//! 语料在 `serve` 启动时加载（[`StoryState::load`]），SIGHUP 时重新加载并原子替换。

pub mod corpus;
pub mod de;
pub mod dto;
pub mod engine;
pub mod index;
pub mod mcp;
mod routes;
mod state;
pub mod text;

pub use routes::router;
pub use state::StoryState;

use std::path::Path;

/// 加载语料：文件视为快照，目录视为 zzz-story-extractor 的输出目录（现场构建）。
pub fn load_source(path: &Path) -> anyhow::Result<corpus::Corpus> {
    if path.is_dir() {
        corpus::build_from_dir(path)
    } else {
        corpus::load_snapshot(path)
    }
}
