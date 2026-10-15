use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use chrono::Utc;
use ik_content::ContentState;
use ik_kernel::Problem;
use ik_kernel::config::Hot;

use crate::hot::{self, HotBoard};

#[derive(Clone)]
pub struct FeedState {
    pub content: ContentState,
    pub settings: Arc<Hot>,
    board: Arc<RwLock<Arc<HotBoard>>>,
}

impl FeedState {
    /// 不启动定时刷新（测试与 CLI 用）；需要时手动调用 [`FeedState::refresh`]。
    pub fn new(content: ContentState, settings: Hot) -> Self {
        Self {
            content,
            settings: Arc::new(settings),
            board: Arc::new(RwLock::new(Arc::new(HotBoard::default()))),
        }
    }

    /// 创建并在当前 tokio 运行时上启动热榜刷新：立即算一次，之后每 `refresh_secs` 秒一次（CRON-001）。
    /// 热榜关闭时不启动。
    pub fn start(content: ContentState, settings: Hot) -> anyhow::Result<Self> {
        let state = Self::new(content, settings);
        if state.settings.enabled {
            let runtime = tokio::runtime::Handle::try_current()
                .map_err(|e| anyhow::anyhow!("热榜刷新需要在 tokio 运行时里启动：{e}"))?;
            let refresher = state.clone();
            runtime.spawn(async move {
                let mut ticker =
                    tokio::time::interval(Duration::from_secs(refresher.settings.refresh_secs));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    ticker.tick().await;
                    if let Err(error) = refresher.refresh().await {
                        // 计算失败时保留上一份榜单（CACHE-002）。
                        tracing::warn!(?error, "热榜刷新失败，继续使用上一份榜单");
                    }
                }
            });
        }
        Ok(state)
    }

    /// 重新计算热榜并替换当前榜单。
    pub async fn refresh(&self) -> Result<(), Problem> {
        let board = hot::compute(&self.content.db, &self.settings, Utc::now()).await?;
        tracing::debug!(
            full = board.full.len(),
            display = board.display.len(),
            "热榜已刷新"
        );
        *self.board.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(board);
        Ok(())
    }

    /// 当前榜单。热榜关闭时为空。
    pub fn board(&self) -> Arc<HotBoard> {
        if !self.settings.enabled {
            return Arc::new(HotBoard::default());
        }
        self.board
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
