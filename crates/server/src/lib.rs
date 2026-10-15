//! 组装 HTTP 应用：状态、路由、中间件栈。`main.rs` 只负责命令行与进程生命周期。

pub mod app;
pub mod idempotency;
pub mod jobs;
pub mod middleware;
pub mod rate_limit;
pub mod state;
pub mod system;

pub use app::{ApiDoc, build_app, openapi};
pub use state::AppState;

/// 仓库根目录 `migrations/` 下的全部迁移，编译期嵌入二进制。
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");
