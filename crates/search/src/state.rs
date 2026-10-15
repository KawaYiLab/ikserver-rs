use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use ik_comments::CommentsState;
use ik_content::ContentState;
use ik_kernel::bus::LocalBus;
use ik_kernel::config::Search as SearchConfig;

use crate::engine::Engine;
use crate::indexer::Indexer;

/// 搜索接口限流：登录用户每秒 10 次，游客按 IP 每秒 5 次（保护搜索引擎与数据库兜底）。
const SEARCHES_PER_SECOND_USER: NonZeroU32 = match NonZeroU32::new(10) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};
const SEARCHES_PER_SECOND_IP: NonZeroU32 = match NonZeroU32::new(5) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};

#[derive(Clone)]
pub struct SearchState {
    pub content: ContentState,
    pub comments: CommentsState,
    pub(crate) engine: Option<Arc<Engine>>,
    pub(crate) indexer: Option<Arc<Indexer>>,
    pub(crate) limits: Arc<SearchLimits>,
}

pub(crate) struct SearchLimits {
    pub per_user: DefaultKeyedRateLimiter<i64>,
    pub per_ip: DefaultKeyedRateLimiter<IpAddr>,
}

impl SearchState {
    /// `config.url` 为空时不连接搜索引擎，所有查询走数据库兜底。
    pub fn new(
        content: ContentState,
        comments: CommentsState,
        config: &SearchConfig,
    ) -> anyhow::Result<Self> {
        let engine = config
            .url
            .as_deref()
            .map(|url| Engine::new(config, url).map(Arc::new))
            .transpose()?;
        let indexer = engine
            .as_ref()
            .map(|engine| Arc::new(Indexer::new(engine.clone(), content.db.clone())));
        Ok(Self {
            content,
            comments,
            engine,
            indexer,
            limits: Arc::new(SearchLimits {
                per_user: RateLimiter::keyed(Quota::per_second(SEARCHES_PER_SECOND_USER)),
                per_ip: RateLimiter::keyed(Quota::per_second(SEARCHES_PER_SECOND_IP)),
            }),
        })
    }

    /// 在当前 tokio 运行时上启动索引同步（订阅 `bus`）。没有配置搜索引擎时什么都不做。
    pub fn start_indexer(&self, bus: &LocalBus, config: &SearchConfig) -> anyhow::Result<()> {
        let Some(indexer) = self.indexer.clone() else {
            return Ok(());
        };
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| anyhow::anyhow!("搜索索引同步需要在 tokio 运行时里启动：{e}"))?;
        runtime.spawn(indexer.run(
            bus.subscribe(),
            Duration::from_secs(config.sweep_secs),
            Duration::from_secs(config.reconcile_hours * 3600),
        ));
        Ok(())
    }

    /// 立即做一次全量对账并等搜索引擎处理完（CLI `reindex` 与测试用）。
    pub async fn reindex(&self) -> anyhow::Result<()> {
        let (Some(engine), Some(indexer)) = (&self.engine, &self.indexer) else {
            anyhow::bail!("没有配置搜索引擎（IK_SEARCH__URL）");
        };
        engine.ensure().await?;
        indexer.reconcile().await?;
        engine.settle().await
    }

    /// 测试用：删除索引（每个测试用独立的索引前缀）。
    #[doc(hidden)]
    pub async fn drop_indexes(&self) -> anyhow::Result<()> {
        match &self.engine {
            Some(engine) => engine.drop_indexes().await,
            None => Ok(()),
        }
    }

    /// 测试用：同步这些帖子 / 评论并等处理完。
    #[doc(hidden)]
    pub async fn sync_now(&self, posts: &[i64], comments: &[i64]) -> anyhow::Result<()> {
        let (Some(engine), Some(indexer)) = (&self.engine, &self.indexer) else {
            return Ok(());
        };
        indexer.sync_posts(posts, true).await?;
        indexer.sync_comments(comments).await?;
        engine.settle().await
    }

    /// 测试用：处理改名并等处理完。
    #[doc(hidden)]
    pub async fn rename_now(&self, user_ids: &[i64]) -> anyhow::Result<()> {
        let (Some(engine), Some(indexer)) = (&self.engine, &self.indexer) else {
            return Ok(());
        };
        indexer.rename(user_ids).await?;
        engine.settle().await
    }

    /// 测试用：删除评论文档并等处理完。
    #[doc(hidden)]
    pub async fn remove_comments_now(&self, ids: &[i64]) -> anyhow::Result<()> {
        let (Some(engine), Some(indexer)) = (&self.engine, &self.indexer) else {
            return Ok(());
        };
        indexer.remove_comments(ids).await?;
        engine.settle().await
    }
}
