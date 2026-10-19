use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use arc_swap::ArcSwapOption;
use ik_kernel::Snowflake;
use ik_kernel::config::{Secret, Story};
use sqlx::PgPool;

use crate::corpus::{Bundle, Corpus, build_from_bundle};
use crate::mcp::Server;
use crate::repo;

/// 多实例部署时，其他实例最多晚这么久切到新版本。
const WATCH_EVERY: Duration = Duration::from_secs(60);

/// 当前在用的语料。
pub struct Loaded {
    pub corpus: Corpus,
    /// 数据库里的版本 id；来自本地文件（`IK_STORY__SOURCE`）时为空。
    pub version: Option<i64>,
}

#[derive(Clone)]
pub struct StoryState {
    inner: Arc<Inner>,
}

struct Inner {
    db: Option<PgPool>,
    ids: Option<Arc<Snowflake>>,
    /// 本地语料（开发用）。设置后启动时用它，不读数据库。
    source: Option<PathBuf>,
    access_key: Secret,
    public: bool,
    current: ArcSwapOption<Loaded>,
    /// 串行化加载与切换，避免旧版本的慢加载覆盖新版本。
    swap: tokio::sync::Mutex<()>,
    server: Server,
}

impl StoryState {
    pub fn new(config: &Story, db: PgPool, ids: Arc<Snowflake>) -> anyhow::Result<Self> {
        Self::build(config, Some(db), Some(ids), None)
    }

    /// 测试与工具用：不连数据库，直接给定语料。
    pub fn with_corpus(corpus: Corpus, config: &Story) -> anyhow::Result<Self> {
        Self::build(config, None, None, Some(corpus))
    }

    fn build(
        config: &Story,
        db: Option<PgPool>,
        ids: Option<Arc<Snowflake>>,
        corpus: Option<Corpus>,
    ) -> anyhow::Result<Self> {
        let source = config.source.trim();
        Ok(Self {
            inner: Arc::new(Inner {
                db,
                ids,
                source: (!source.is_empty()).then(|| PathBuf::from(source)),
                access_key: config.access_key.clone(),
                public: config.public,
                current: ArcSwapOption::new(corpus.map(|corpus| {
                    Arc::new(Loaded {
                        corpus,
                        version: None,
                    })
                })),
                swap: tokio::sync::Mutex::new(()),
                server: Server::new().context("序列化 MCP 工具清单失败")?,
            }),
        })
    }

    /// 启动时加载：设置了 `IK_STORY__SOURCE` 用本地语料，否则用数据库里的生效版本；都没有则不启用。
    /// 解析与建索引放到阻塞线程池里做；失败时保留原来的语料。
    pub async fn load(&self) -> anyhow::Result<()> {
        if let Some(source) = self.inner.source.clone() {
            let _guard = self.inner.swap.lock().await;
            let shown = source.display().to_string();
            let corpus = build_blocking(move || crate::load_source(&source))
                .await
                .with_context(|| format!("加载剧情库 {shown} 失败"))?;
            self.install(corpus, None, &shown);
            return Ok(());
        }
        self.load_active().await
    }

    /// 加载数据库里的生效版本（没有生效版本时清空）。
    pub(crate) async fn load_active(&self) -> anyhow::Result<()> {
        let Some(db) = &self.inner.db else {
            return Ok(());
        };
        let _guard = self.inner.swap.lock().await;
        let Some((id, bundle)) = repo::active_bundle(db)
            .await
            .context("读取剧情库版本失败")?
        else {
            if self.inner.current.load().is_some() {
                tracing::info!("数据库里没有生效的剧情库版本，停用");
            }
            self.inner.current.store(None);
            return Ok(());
        };
        if self.version() == Some(id) {
            return Ok(());
        }
        let corpus = build_blocking(move || {
            build_from_bundle(&Bundle::decode(&bundle, crate::MAX_BUNDLE_BYTES)?)
        })
        .await
        .with_context(|| format!("构建剧情库版本 {id} 失败"))?;
        self.install(corpus, Some(id), "database");
        Ok(())
    }

    /// 上传后直接换上已经构建好的语料，不用再从数据库读一遍。
    pub(crate) async fn install_uploaded(&self, corpus: Corpus, id: i64) {
        let _guard = self.inner.swap.lock().await;
        self.install(corpus, Some(id), "upload");
    }

    fn install(&self, corpus: Corpus, version: Option<i64>, from: &str) {
        tracing::info!(
            from,
            version = ?version,
            source_version = %corpus.meta.source_version,
            docs = corpus.docs.len(),
            lines = corpus.lines.len(),
            "剧情库已加载"
        );
        if !self.inner.public && self.inner.access_key.is_empty() {
            tracing::warn!(
                "剧情库已加载，但没有设置 IK_STORY__ACCESS_KEY 也没有 IK_STORY__PUBLIC=true，MCP 接口会拒绝所有请求"
            );
        }
        self.inner
            .current
            .store(Some(Arc::new(Loaded { corpus, version })));
    }

    /// 定期检查数据库里的生效版本，变了就切换（多实例时由别的实例上传或回滚）。
    /// 设置了本地语料时不启动。
    pub fn spawn_watcher(&self) -> Option<tokio::task::JoinHandle<()>> {
        if self.inner.source.is_some() {
            return None;
        }
        let db = self.inner.db.clone()?;
        let state = self.clone();
        Some(tokio::spawn(async move {
            let mut tick = tokio::time::interval(WATCH_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            loop {
                tick.tick().await;
                match repo::active_id(&db).await {
                    Ok(id) if id != state.version() => {
                        if let Err(error) = state.load_active().await {
                            tracing::error!(error = %format!("{error:#}"), "剧情库切换版本失败，继续使用当前语料");
                        }
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!(%error, "检查剧情库版本失败"),
                }
            }
        }))
    }

    pub fn is_configured(&self) -> bool {
        self.inner.source.is_some() || self.inner.db.is_some()
    }

    pub fn current(&self) -> Option<Arc<Loaded>> {
        self.inner.current.load_full()
    }

    pub fn version(&self) -> Option<i64> {
        self.inner.current.load().as_ref().and_then(|l| l.version)
    }

    pub(crate) fn uses_local_source(&self) -> bool {
        self.inner.source.is_some()
    }

    pub(crate) fn db(&self) -> Option<&PgPool> {
        self.inner.db.as_ref()
    }

    pub(crate) fn ids(&self) -> Option<&Snowflake> {
        self.inner.ids.as_deref()
    }

    pub(crate) fn access_key(&self) -> &Secret {
        &self.inner.access_key
    }

    pub(crate) fn is_public(&self) -> bool {
        self.inner.public
    }

    pub(crate) fn server(&self) -> &Server {
        &self.inner.server
    }
}

/// 在阻塞线程池里构建语料（解压、解析、建索引是纯 CPU 活）。
pub(crate) async fn build_blocking(
    work: impl FnOnce() -> anyhow::Result<Corpus> + Send + 'static,
) -> anyhow::Result<Corpus> {
    let started = Instant::now();
    let corpus = tokio::task::spawn_blocking(work)
        .await
        .context("构建剧情库的任务异常退出")??;
    tracing::debug!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        "剧情库构建完成"
    );
    Ok(corpus)
}
