//! 后台任务执行器：登记各域的任务、定时任务与执行时要用的 state。

use graphile_worker::{Cron, Crontab, CrontabTimerError, WorkerOptions};
use ik_kernel::jobs::SCHEMA;

use crate::AppState;

/// 定时任务。payload 必须显式给出：不给时执行器派发 `null`，而任务结构体（`struct X {}`）
/// 只能从 JSON 对象反序列化，任务会每次都失败。
pub fn crontabs() -> anyhow::Result<Vec<Crontab>> {
    let invalid = |e: CrontabTimerError| anyhow::anyhow!("定时任务配置错误：{e:?}");
    Ok(vec![
        Cron::hourly_at::<ik_media::jobs::SweepStaging>(17)
            .map_err(invalid)?
            .payload(ik_media::jobs::SweepStaging::default())?
            .max_attempts(1)
            .build(),
        Cron::daily_at::<crate::idempotency::PruneIdempotencyKeys>(4, 23)
            .map_err(invalid)?
            .payload(crate::idempotency::PruneIdempotencyKeys::default())?
            .max_attempts(1)
            .build(),
        Cron::hourly_at::<ik_mihoyo::jobs::PruneQrLogins>(41)
            .map_err(invalid)?
            .payload(ik_mihoyo::jobs::PruneQrLogins::default())?
            .max_attempts(1)
            .build(),
        Cron::every_minute::<ik_ai::jobs::ReclaimQuotaHolds>()
            .payload(ik_ai::jobs::ReclaimQuotaHolds::default())?
            .max_attempts(1)
            .build(),
        Cron::hourly_at::<ik_ai::mcp::RefreshMcpTools>(53)
            .map_err(invalid)?
            .payload(ik_ai::mcp::RefreshMcpTools::default())?
            .max_attempts(1)
            .build(),
    ])
}

/// 执行器选项。`serve` 用它启动常驻执行器；测试用它 `run_once`。
pub fn worker_options(state: &AppState) -> anyhow::Result<WorkerOptions> {
    let options = WorkerOptions::default()
        .schema(SCHEMA)
        .concurrency(state.config.jobs.concurrency)
        .pg_pool(state.db.clone())
        // 停机由 `serve` 统一处理（先停 HTTP，再等在途任务）。
        .listen_os_shutdown_signals(false)
        .define_job::<ik_content::jobs::FanoutNewPost>()
        .define_job::<ik_media::jobs::SweepStaging>()
        .define_job::<crate::idempotency::PruneIdempotencyKeys>()
        .define_job::<ik_mihoyo::jobs::PruneQrLogins>()
        .define_job::<ik_ai::jobs::ReclaimQuotaHolds>()
        .define_job::<ik_ai::mcp::RefreshMcpTools>()
        .define_job::<ik_ai::dm::DmReply>()
        .define_job::<ik_ai::comment::CommentReply>()
        .define_job::<ik_ai::memory_jobs::SummarizeSession>()
        .define_job::<ik_ai::memory_jobs::ExtractMemories>()
        .add_extension(state.mihoyo.clone())
        .add_extension(state.ai.clone())
        .add_extension(state.db.clone())
        .add_extension(state.content.clone())
        .add_extension(state.media.clone());
    Ok(crontabs()?
        .into_iter()
        .fold(options, WorkerOptions::with_cron))
}

#[cfg(test)]
mod tests {
    use graphile_worker::TaskHandler;
    use serde::de::DeserializeOwned;

    use super::*;

    fn decodes<T: TaskHandler + DeserializeOwned>(crontab: &Crontab) -> bool {
        crontab.task_identifier != T::IDENTIFIER
            || crontab
                .payload
                .clone()
                .is_some_and(|p| serde_json::from_value::<T>(p).is_ok())
    }

    #[test]
    fn cron_payloads_decode_into_their_tasks() {
        let crontabs = crontabs().unwrap();
        assert_eq!(crontabs.len(), 5);
        for crontab in &crontabs {
            assert!(
                decodes::<ik_media::jobs::SweepStaging>(crontab)
                    && decodes::<crate::idempotency::PruneIdempotencyKeys>(crontab)
                    && decodes::<ik_mihoyo::jobs::PruneQrLogins>(crontab)
                    && decodes::<ik_ai::jobs::ReclaimQuotaHolds>(crontab)
                    && decodes::<ik_ai::mcp::RefreshMcpTools>(crontab),
                "{} 的 payload 解不出任务结构体",
                crontab.task_identifier
            );
        }
    }
}
