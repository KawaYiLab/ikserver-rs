//! 米游社的后台任务。

use ik_kernel::jobs::{IntoTaskHandlerResult, TaskHandler, WorkerContext};
use serde::{Deserialize, Serialize};

use crate::MihoyoState;

/// 清理过期的扫码会话（旧系统靠 Redis TTL）。定时任务每小时一次，失败不重试。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PruneQrLogins {}

impl TaskHandler for PruneQrLogins {
    const IDENTIFIER: &'static str = "mihoyo.prune_qr_logins";

    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(state) = ctx.get_ext::<MihoyoState>() else {
            return Err("执行器没有注册 MihoyoState".to_owned());
        };
        crate::service::prune(state)
            .await
            .map(|_| ())
            .map_err(|p| p.to_string())
    }
}
