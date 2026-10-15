//! ai 的后台任务。

use chrono::Utc;
use ik_kernel::jobs::{IntoTaskHandlerResult, TaskHandler, WorkerContext};
use serde::{Deserialize, Serialize};

use crate::{AiState, quota};

/// 每次最多回收的预占份数。
const RECLAIM_BATCH: i64 = 500;

/// 回收过期未结算的预占（QUOTA-016），顺带清理过期的结算去重标记与旧计数器。
/// 由执行器每分钟安排一次；失败不重试，下一分钟再来。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReclaimQuotaHolds {}

impl TaskHandler for ReclaimQuotaHolds {
    const IDENTIFIER: &'static str = "ai.reclaim_quota_holds";

    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(state) = ctx.get_ext::<AiState>() else {
            return Err("执行器没有注册 AiState".to_owned());
        };
        run(state).await.map_err(|p| format!("{p:?}"))
    }
}

async fn run(state: &AiState) -> Result<(), ik_kernel::Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let reclaimed = quota::reclaim_expired(&mut tx, now, RECLAIM_BATCH).await?;
    quota::prune(&mut tx, now).await?;
    tx.commit().await?;
    if reclaimed > 0 {
        tracing::warn!(reclaimed, "回收了过期未结算的 AI 额度预占");
    }
    Ok(())
}
