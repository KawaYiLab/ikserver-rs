//! media 的后台任务。

use ik_kernel::jobs::{IntoTaskHandlerResult, TaskHandler, WorkerContext};
use serde::{Deserialize, Serialize};

use crate::MediaState;

/// 清理过期的上传暂存对象（UPL-060）。由执行器按定时任务每小时安排一次，多实例时也只有一个实例执行。
/// 失败不重试，下一个小时再来。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SweepStaging {}

impl TaskHandler for SweepStaging {
    const IDENTIFIER: &'static str = "media.sweep_staging";

    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(state) = ctx.get_ext::<MediaState>() else {
            return Err("执行器没有注册 MediaState".to_owned());
        };
        match state.sweep_staging().await {
            Ok(0) => Ok(()),
            Ok(deleted) => {
                tracing::info!(deleted, "清理了过期的上传暂存对象");
                Ok(())
            }
            Err(error) => Err(format!("{error:#}")),
        }
    }
}
