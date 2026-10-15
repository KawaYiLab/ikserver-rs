//! 后台任务：graphile_worker（PostgreSQL 队列）的薄封装。
//!
//! - 任务由业务域定义（实现 [`TaskHandler`]，payload 是 serde 结构体），在业务事务里用 [`Jobs::enqueue`]
//!   入队，事务回滚时任务一起消失。
//! - 执行器由 `ik-server` 组装（登记各域的任务与定时任务），任务通过 `WorkerContext::get_ext`
//!   取各域的 state。
//! - 失败按 graphile_worker 的指数退避重试（`exp(min(attempts, 10))` 秒）；用完 `max_attempts` 后
//!   任务留在队列表里不再执行（即死信），`last_error` 记录最后一次错误。每个任务在定义旁写明自己的
//!   `max_attempts`。

use graphile_worker::WorkerUtils;
use sqlx::{PgConnection, PgPool};

use crate::Problem;

pub use graphile_worker::{IntoTaskHandlerResult, JobSpec, TaskHandler, WorkerContext};

/// graphile_worker 自己的表所在的 schema（由它自己迁移，不进 `migrations/`）。
pub const SCHEMA: &str = "graphile_worker";

/// 入队句柄。克隆代价很低。
#[derive(Clone)]
pub struct Jobs {
    utils: WorkerUtils,
}

impl Jobs {
    pub fn new(db: PgPool) -> Self {
        Self {
            utils: WorkerUtils::new(db, SCHEMA),
        }
    }

    /// 在调用方的事务里入队。
    pub async fn enqueue<T: TaskHandler>(
        &self,
        conn: &mut PgConnection,
        payload: T,
        spec: JobSpec,
    ) -> Result<(), Problem> {
        self.utils
            .clone()
            .with_executor(conn)
            .add_job(payload, spec)
            .await
            .map_err(|error| {
                tracing::error!(?error, task = T::IDENTIFIER, "任务入队失败");
                Problem::internal()
            })?;
        Ok(())
    }

    /// 创建或升级 graphile_worker 的表。执行器启动时会自动做；只入队、不执行任务的进程
    /// （CLI、测试）要先调用一次。
    pub async fn migrate(&self) -> anyhow::Result<()> {
        self.utils
            .migrate()
            .await
            .map_err(|e| anyhow::anyhow!("graphile_worker 迁移失败：{e:?}"))
    }
}
