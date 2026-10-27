//! 管理后台的跨域部分（api-guidelines.md「管理后台」）：账号管理、仪表盘、搜索索引运维、审计日志查看。
//!
//! 各业务域自己的后台接口在各自的 crate 里（题库 `ik_exam::admin`、AI `ik_ai::admin`、分区与标签
//! `ik_content::admin`、装扮 `ik_cosmetics::admin`、举报与封禁 `ik_moderation`）；这里只放要编排
//! 多个域的部分。操作者与审计见 `ik_auth::admin`。

mod dashboard;
mod routes;
mod users;

pub use dashboard::{Dashboard, SearchStatus, TrendQuery, Trends};
pub use routes::router;
pub use users::{
    AdminUserDetail, AiAgentUpdate, DennyAdjustment, EconomyResult, ExpGrant, LedgerQuery,
    ProfileReset, ProfileResetResult, RevokeResult, RoleUpdate, UsernameUpdate,
};

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use ik_ai::AiState;
use ik_content::ContentState;
use ik_exam::ExamState;
use ik_kernel::Snowflake;
use ik_kernel::site::Site;
use ik_moderation::ModerationState;
use ik_search::SearchState;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AdminState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub content: ContentState,
    pub exam: ExamState,
    pub ai: AiState,
    pub moderation: ModerationState,
    pub search: SearchState,
    /// 站点参数（「系统设置」页读写，保存后让本实例缓存失效）。
    pub site: Site,
    /// 正在后台重建索引（同一进程同时只跑一个）。
    pub(crate) reindexing: Arc<AtomicBool>,
}

impl AdminState {
    pub fn new(
        content: ContentState,
        exam: ExamState,
        ai: AiState,
        moderation: ModerationState,
        search: SearchState,
        site: Site,
    ) -> Self {
        Self {
            db: content.db.clone(),
            ids: content.ids.clone(),
            content,
            exam,
            ai,
            moderation,
            search,
            site,
            reindexing: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl std::fmt::Debug for AdminState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminState").finish_non_exhaustive()
    }
}
