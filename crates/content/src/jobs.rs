//! content 的后台任务。

use chrono::Utc;
use ik_kernel::Problem;
use ik_kernel::bus::Effects;
use ik_kernel::jobs::{IntoTaskHandlerResult, JobSpec, TaskHandler, WorkerContext};
use ik_notify::interop::FanoutPage;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::ContentState;
use crate::repo;

/// 每批最多扫多少条关注记录。一批一个事务。
const FANOUT_BATCH: i64 = 1000;
/// 新帖扇出的重试次数（指数退避，约 9.5 小时用完）；之后留在队列表里作为死信。
/// 每批幂等（通知按 `follow:<帖子 id>` 去重），重试不会重复通知。
const FANOUT_MAX_ATTEMPTS: i16 = 10;

/// 新帖扇出的一批：通知帖子作者 id 大于 `after` 的粉丝（FOLLOW-009）。
/// 做完一批后在同一事务里安排下一批，所以进度不会丢，也不会有一个很长的事务。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FanoutNewPost {
    pub post_id: i64,
    pub after: i64,
}

impl FanoutNewPost {
    fn spec(&self) -> JobSpec {
        JobSpec {
            max_attempts: Some(FANOUT_MAX_ATTEMPTS),
            job_key: Some(format!("fanout:{}:{}", self.post_id, self.after)),
            ..JobSpec::default()
        }
    }

    /// 在发布事务里安排第一批。
    pub(crate) async fn schedule(
        state: &ContentState,
        conn: &mut PgConnection,
        post_id: i64,
    ) -> Result<(), Problem> {
        let job = Self { post_id, after: 0 };
        let spec = job.spec();
        state.jobs.enqueue(conn, job, spec).await
    }
}

impl TaskHandler for FanoutNewPost {
    const IDENTIFIER: &'static str = "content.fanout_new_post";

    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(state) = ctx.get_ext::<ContentState>() else {
            return Err("执行器没有注册 ContentState".to_owned());
        };
        fanout_batch(state, &self)
            .await
            .map(|_| ())
            .map_err(|p| p.to_string())
    }
}

/// 执行一批扇出，返回这一批通知到的人数。帖子已经下线、隐藏或删除时什么都不做（剩下的粉丝不再通知）。
pub async fn fanout_batch(state: &ContentState, job: &FanoutNewPost) -> Result<usize, Problem> {
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let Some(post) = repo::get_post(&mut tx, job.post_id).await? else {
        return Ok(0);
    };
    if post.status != "published" || post.hidden_at.is_some() || post.is_anonymous {
        return Ok(0);
    }
    let outcome = ik_notify::interop::fanout_new_post(
        &mut tx,
        &state.ids,
        post.author_id,
        post.id,
        &post.title,
        FanoutPage {
            after: job.after,
            limit: FANOUT_BATCH,
        },
        Utc::now(),
        &mut effects,
    )
    .await?;
    if let Some(after) = outcome.next_after {
        let next = FanoutNewPost {
            post_id: job.post_id,
            after,
        };
        let spec = next.spec();
        state.jobs.enqueue(&mut tx, next, spec).await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(outcome.sent)
}
