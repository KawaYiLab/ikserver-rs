//! 上传校验的并发闸门（UPL-040～043、D-003 方案 A）。
//!
//! 每个校验任务要把整个文件读进内存（最多 30 MiB），所以同时只跑 `slots` 个；排队的最多 64 个，
//! 再多直接拒绝（busy）。排队超时与任务超时都立即回给调用方，但任务本身继续跑完才释放槽位
//! （它可能还拿着缓冲区）。进程内计数，多实例时总并发 = 实例数 × 槽位数。

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Semaphore;

/// 排队上限（UPL-040）。
pub const QUEUE_LIMIT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateError {
    /// 排队的已经太多。
    Busy,
    /// 排队等槽位超时。
    QueueTimeout,
    /// 拿到槽位后任务总时长超时。
    TaskTimeout,
    /// 任务 panic（不应发生）。
    Failed,
}

pub struct Gate {
    slots: Arc<Semaphore>,
    waiting: AtomicUsize,
    queue_timeout: Duration,
    task_timeout: Duration,
}

struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Gate {
    pub fn new(slots: usize, queue_timeout: Duration, task_timeout: Duration) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(slots.max(1))),
            waiting: AtomicUsize::new(0),
            queue_timeout,
            task_timeout,
        }
    }

    /// 在一个槽位里运行 `task`。
    pub async fn run<F, T>(&self, task: F) -> Result<T, GateError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let permit = match self.slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                if self.waiting.fetch_add(1, Ordering::AcqRel) >= QUEUE_LIMIT {
                    self.waiting.fetch_sub(1, Ordering::AcqRel);
                    return Err(GateError::Busy);
                }
                let _waiting = Waiting(&self.waiting);
                match tokio::time::timeout(self.queue_timeout, self.slots.clone().acquire_owned())
                    .await
                {
                    Ok(Ok(permit)) => permit,
                    Ok(Err(_)) => return Err(GateError::Failed),
                    Err(_) => return Err(GateError::QueueTimeout),
                }
            }
        };
        let handle = tokio::spawn(async move {
            let _permit = permit;
            task.await
        });
        match tokio::time::timeout(self.task_timeout, handle).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(GateError::Failed),
            Err(_) => Err(GateError::TaskTimeout),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn queue_timeout_and_task_timeout_keep_the_slot_until_done() {
        let gate = Arc::new(Gate::new(
            1,
            Duration::from_secs(5),
            Duration::from_secs(10),
        ));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let long = {
            let gate = gate.clone();
            tokio::spawn(async move {
                gate.run(async move {
                    let _ = release_rx.await;
                    1
                })
                .await
            })
        };
        tokio::task::yield_now().await;
        // 槽位被占：排队 5 秒超时。
        assert_eq!(gate.run(async { 2 }).await, Err(GateError::QueueTimeout));
        // 长任务 10 秒超时，调用方先得到错误，但槽位要等任务真正结束。
        assert_eq!(long.await.unwrap(), Err(GateError::TaskTimeout));
        assert_eq!(gate.run(async { 3 }).await, Err(GateError::QueueTimeout));
        release_tx.send(()).unwrap();
        tokio::task::yield_now().await;
        assert_eq!(gate.run(async { 4 }).await, Ok(4));
    }

    #[tokio::test(start_paused = true)]
    async fn rejects_when_queue_is_full() {
        let gate = Arc::new(Gate::new(
            1,
            Duration::from_secs(600),
            Duration::from_secs(600),
        ));
        let (_hold, rx) = tokio::sync::oneshot::channel::<()>();
        let g = gate.clone();
        tokio::spawn(async move {
            g.run(async move {
                let _ = rx.await;
            })
            .await
        });
        tokio::task::yield_now().await;
        for _ in 0..QUEUE_LIMIT {
            let g = gate.clone();
            tokio::spawn(async move { g.run(async {}).await });
        }
        while gate.waiting.load(Ordering::Acquire) < QUEUE_LIMIT {
            tokio::task::yield_now().await;
        }
        assert_eq!(gate.run(async {}).await, Err(GateError::Busy));
    }
}
