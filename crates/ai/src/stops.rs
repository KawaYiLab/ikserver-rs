//! 停止信号的推送（GEN-011）。
//!
//! `ai_jobs.stop_requested_at` 第一次写入时，数据库触发器 `NOTIFY ai_stop, '<任务 id>'`（提交后才发出，
//! 所有实例都收得到）。每个实例一条 LISTEN 连接，收到后叫醒本实例正在生成这个任务的循环，用户点停止后
//! 不必等下一次轮询。生成方仍按 [`StopSignals::poll_interval`] 查库兜底：监听断开重连期间的通知会丢。
//!
//! 监听由服务启动时 [`StopSignals::start`] 打开；没打开（测试、命令行）时按原来的 600ms 轮询。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::sync::Notify;

const CHANNEL: &str = "ai_stop";
/// 没有监听时的轮询间隔（GEN-011 要求不超过 600ms）。
const POLL: Duration = Duration::from_millis(600);
/// 有监听时的兜底轮询间隔。
const FALLBACK_POLL: Duration = Duration::from_secs(3);
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct StopSignals {
    inner: Arc<Inner>,
}

struct Inner {
    db: PgPool,
    listening: AtomicBool,
    waiters: Mutex<HashMap<i64, Arc<Notify>>>,
}

impl StopSignals {
    pub fn new(db: PgPool) -> Self {
        Self {
            inner: Arc::new(Inner {
                db,
                listening: AtomicBool::new(false),
                waiters: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// 打开监听（服务启动时调用一次，重复调用无效）。
    pub fn start(&self) {
        if self.inner.listening.swap(true, Ordering::SeqCst) {
            return;
        }
        tokio::spawn(listen_loop(self.inner.clone()));
    }

    /// 生成方查库的间隔：有监听时只是兜底。
    pub fn poll_interval(&self) -> Duration {
        if self.inner.listening.load(Ordering::SeqCst) {
            FALLBACK_POLL
        } else {
            POLL
        }
    }

    /// 登记一个正在生成的任务；返回的守卫丢弃时注销。
    pub(crate) fn watch(&self, job_id: i64) -> StopWatch {
        let notify = Arc::new(Notify::new());
        self.inner.waiters.lock().insert(job_id, notify.clone());
        StopWatch {
            inner: self.inner.clone(),
            job_id,
            notify,
        }
    }

    fn signal(inner: &Inner, job_id: i64) {
        if let Some(notify) = inner.waiters.lock().get(&job_id) {
            // notify_one 在没人等的时候也会留下一个许可，下一次等待立即返回。
            notify.notify_one();
        }
    }
}

impl std::fmt::Debug for StopSignals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StopSignals")
            .field("listening", &self.inner.listening.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

/// 一个任务的停止信号。
pub(crate) struct StopWatch {
    inner: Arc<Inner>,
    job_id: i64,
    notify: Arc<Notify>,
}

impl StopWatch {
    /// 收到这个任务的停止通知时返回。
    pub(crate) async fn notified(&self) {
        self.notify.notified().await;
    }
}

impl Drop for StopWatch {
    fn drop(&mut self) {
        let mut waiters = self.inner.waiters.lock();
        if waiters
            .get(&self.job_id)
            .is_some_and(|n| Arc::ptr_eq(n, &self.notify))
        {
            waiters.remove(&self.job_id);
        }
    }
}

async fn listen_loop(inner: Arc<Inner>) {
    loop {
        match PgListener::connect_with(&inner.db).await {
            Ok(mut listener) => match listener.listen(CHANNEL).await {
                Ok(()) => loop {
                    match listener.recv().await {
                        Ok(notification) => {
                            if let Ok(job_id) = notification.payload().parse::<i64>() {
                                StopSignals::signal(&inner, job_id);
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "AI 停止信号的监听断开，稍后重连");
                            break;
                        }
                    }
                },
                Err(error) => tracing::warn!(%error, "AI 停止信号 LISTEN 失败"),
            },
            Err(error) => tracing::warn!(%error, "AI 停止信号的监听连接失败"),
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[tokio::test]
    async fn signal_wakes_only_the_watched_job() {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/unused")
            .unwrap();
        let signals = StopSignals::new(db);
        assert_eq!(signals.poll_interval(), POLL);
        let watch = signals.watch(7);
        StopSignals::signal(&signals.inner, 8);
        StopSignals::signal(&signals.inner, 7);
        // 先发后等也能收到（许可留着）。
        tokio::time::timeout(Duration::from_secs(1), watch.notified())
            .await
            .unwrap();
        drop(watch);
        assert!(signals.inner.waiters.lock().is_empty());
    }
}
