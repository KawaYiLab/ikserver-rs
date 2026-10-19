use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use arc_swap::ArcSwapOption;
use ik_kernel::config::{Secret, Story};

use crate::corpus::Corpus;
use crate::mcp::Server;

#[derive(Clone)]
pub struct StoryState {
    inner: Arc<Inner>,
}

struct Inner {
    /// 语料来源；`None` 表示未启用。
    source: Option<PathBuf>,
    /// 访问密钥；空表示公开（是否允许由配置校验决定）。
    access_key: Secret,
    /// 当前语料。加载前与未启用时为空；重新加载时原子替换，进行中的请求继续用旧的。
    corpus: ArcSwapOption<Corpus>,
    server: Server,
}

impl StoryState {
    pub fn new(config: &Story) -> anyhow::Result<Self> {
        let source = config.source.trim();
        Ok(Self {
            inner: Arc::new(Inner {
                source: (!source.is_empty()).then(|| PathBuf::from(source)),
                access_key: config.access_key.clone(),
                corpus: ArcSwapOption::empty(),
                server: Server::new().context("序列化 MCP 工具清单失败")?,
            }),
        })
    }

    /// 测试与工具用：直接给定语料。
    pub fn with_corpus(corpus: Corpus, access_key: Secret) -> anyhow::Result<Self> {
        Ok(Self {
            inner: Arc::new(Inner {
                source: None,
                access_key,
                corpus: ArcSwapOption::from_pointee(corpus),
                server: Server::new().context("序列化 MCP 工具清单失败")?,
            }),
        })
    }

    pub fn is_configured(&self) -> bool {
        self.inner.source.is_some()
    }

    /// 从配置的来源加载（或重新加载）语料。未配置时什么也不做。
    /// 解析与建索引是 CPU 活，放到阻塞线程池里做；失败时保留旧语料。
    pub async fn load(&self) -> anyhow::Result<()> {
        let Some(source) = self.inner.source.clone() else {
            return Ok(());
        };
        let started = Instant::now();
        let shown = source.display().to_string();
        let corpus = tokio::task::spawn_blocking(move || crate::load_source(&source))
            .await
            .context("加载剧情库的任务异常退出")?
            .with_context(|| format!("加载剧情库 {shown} 失败"))?;
        tracing::info!(
            source = %shown,
            version = %corpus.meta.source_version,
            docs = corpus.docs.len(),
            lines = corpus.lines.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "剧情库已加载"
        );
        self.inner.corpus.store(Some(Arc::new(corpus)));
        Ok(())
    }

    pub fn corpus(&self) -> Option<Arc<Corpus>> {
        self.inner.corpus.load_full()
    }

    pub(crate) fn access_key(&self) -> &Secret {
        &self.inner.access_key
    }

    pub(crate) fn server(&self) -> &Server {
        &self.inner.server
    }
}
