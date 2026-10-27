use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use chrono::Utc;
use ik_content::ContentState;
use ik_kernel::Problem;
use ik_kernel::config::Hot;
use ik_kernel::site::Site;

use crate::hot::{self, HotBoard};

#[derive(Clone)]
pub struct FeedState {
    pub content: ContentState,
    /// 部署配置里的热榜参数；设置了 `site` 时以后台的为准。
    pub settings: Arc<Hot>,
    site: Option<Site>,
    board: Arc<RwLock<Arc<HotBoard>>>,
}

impl FeedState {
    /// 不启动定时刷新（测试与 CLI 用）；需要时手动调用 [`FeedState::refresh`]。
    pub fn new(content: ContentState, settings: Hot) -> Self {
        Self {
            content,
            settings: Arc::new(settings),
            site: None,
            board: Arc::new(RwLock::new(Arc::new(HotBoard::default()))),
        }
    }

    /// 创建并在当前 tokio 运行时上启动热榜刷新：立即算一次，之后每 `refresh_secs` 秒一次（CRON-001）。
    /// 每轮按当时的参数决定间隔与是否计算，所以后台改了开关或间隔不用重启。
    /// 没有 `site` 且部署配置关闭热榜时不启动。
    pub fn start(content: ContentState, settings: Hot, site: Option<Site>) -> anyhow::Result<Self> {
        let mut state = Self::new(content, settings);
        state.site = site;
        if state.site.is_some() || state.settings.enabled {
            let runtime = tokio::runtime::Handle::try_current()
                .map_err(|e| anyhow::anyhow!("热榜刷新需要在 tokio 运行时里启动：{e}"))?;
            let refresher = state.clone();
            runtime.spawn(async move {
                loop {
                    if let Err(error) = refresher.refresh().await {
                        // 计算失败时保留上一份榜单（CACHE-002）。
                        tracing::warn!(?error, "热榜刷新失败，继续使用上一份榜单");
                    }
                    // 关闭期间每分钟看一次开关。
                    let params = refresher.params().await;
                    let wait = if params.enabled {
                        params.refresh_secs.max(1)
                    } else {
                        60
                    };
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                }
            });
        }
        Ok(state)
    }

    /// 当前生效的热榜参数。
    pub async fn params(&self) -> Arc<Hot> {
        match &self.site {
            Some(site) => Arc::new(Hot::from(&site.get().await.hot)),
            None => self.settings.clone(),
        }
    }

    /// 重新计算热榜并替换当前榜单。热榜关闭时清空榜单。
    pub async fn refresh(&self) -> Result<(), Problem> {
        let settings = self.params().await;
        if !settings.enabled {
            *self.board.write().unwrap_or_else(PoisonError::into_inner) =
                Arc::new(HotBoard::default());
            return Ok(());
        }
        let board = hot::compute(&self.content.db, &settings, Utc::now()).await?;
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
        // 有 `site` 时关闭热榜由刷新任务清空榜单。
        if self.site.is_none() && !self.settings.enabled {
            return Arc::new(HotBoard::default());
        }
        self.board
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
