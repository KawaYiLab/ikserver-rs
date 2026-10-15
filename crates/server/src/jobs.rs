//! 后台任务执行器：登记各域的任务、定时任务与执行时要用的 state。

use graphile_worker::{Cron, WorkerOptions};
use ik_kernel::jobs::SCHEMA;

use crate::AppState;

/// 执行器选项。`serve` 用它启动常驻执行器；测试用它 `run_once`。
pub fn worker_options(state: &AppState) -> anyhow::Result<WorkerOptions> {
    let sweep_staging = Cron::hourly_at::<ik_media::jobs::SweepStaging>(17)
        .map_err(|e| anyhow::anyhow!("定时任务配置错误：{e:?}"))?
        .max_attempts(1)
        .build();
    Ok(WorkerOptions::default()
        .schema(SCHEMA)
        .concurrency(state.config.jobs.concurrency)
        .pg_pool(state.db.clone())
        // 停机由 `serve` 统一处理（先停 HTTP，再等在途任务）。
        .listen_os_shutdown_signals(false)
        .define_job::<ik_content::jobs::FanoutNewPost>()
        .define_job::<ik_media::jobs::SweepStaging>()
        .add_extension(state.content.clone())
        .add_extension(state.media.clone())
        .with_cron(sweep_staging))
}
