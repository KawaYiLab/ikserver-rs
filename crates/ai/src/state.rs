use std::sync::Arc;

use ik_comments::CommentsState;
use ik_content::ContentState;
use ik_feed::FeedState;
use ik_kernel::Snowflake;
use ik_kernel::bus::EventBus;
use ik_kernel::config::Ai;
use ik_kernel::jobs::Jobs;
use ik_media::MediaState;
use ik_messaging::MessagingState;
use ik_profile::ProfileState;
use ik_search::SearchState;
use ik_story::StoryState;
use sqlx::PgPool;

use crate::catalog::CatalogCache;
use crate::llm::Providers;
use crate::mcp_client::McpClient;

#[derive(Clone)]
pub struct AiState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub bus: Arc<dyn EventBus>,
    pub media: MediaState,
    /// AI 私聊建在私信之上（会话视图、消息写入）。
    pub messaging: MessagingState,
    /// AI 评论回复写在评论之下（触发钩子、以 AI 名义回复）。
    pub comments: CommentsState,
    /// 论坛工具读取的各域（TOOL 第 8 节）。
    pub forum: Forum,
    pub jobs: Jobs,
    /// 含可用的提供方（[`crate::catalog::Catalog::providers`]）；生成时从这里取上游。
    pub catalog: CatalogCache,
    /// 部署配置里的提供方（启动时固定）。后台维护的在 `catalog` 里。
    pub providers: Providers,
    pub config: Arc<Ai>,
    /// 剧情库（角色卡开了剧情库工具时在进程内调用，ADR 0004）。没有组装时不提供剧情库工具。
    pub story: Option<StoryState>,
    /// 站外 MCP 服务的客户端（连接池、会话、熔断）。
    pub mcp: McpClient,
}

/// 论坛工具读取的各域：都以游客身份调用它们的 service，只读公开内容（TOOL-001/008）。
#[derive(Clone)]
pub struct Forum {
    pub content: ContentState,
    pub comments: CommentsState,
    pub profile: ProfileState,
    pub feed: FeedState,
    pub search: SearchState,
}

impl Forum {
    /// 测试用：搜索不连搜索引擎（走数据库兜底），热榜不自动刷新。
    #[doc(hidden)]
    pub fn for_tests(
        db: PgPool,
        ids: Arc<Snowflake>,
        bus: Arc<dyn EventBus>,
        media: MediaState,
    ) -> anyhow::Result<Self> {
        let content = ContentState::new(db.clone(), ids.clone(), bus.clone(), media.clone());
        let comments = CommentsState::new(db.clone(), ids.clone(), bus, media.clone());
        let profile = ProfileState::new(db, ids, content.clone(), comments.clone(), media);
        let feed = FeedState::new(content.clone(), ik_kernel::config::Hot::default());
        let search = SearchState::new(
            content.clone(),
            comments.clone(),
            &ik_kernel::config::Search::default(),
        )?;
        Ok(Self {
            content,
            comments,
            profile,
            feed,
            search,
        })
    }
}

impl AiState {
    /// 数据库、ID、事件总线、媒体与私信共用。
    pub fn new(messaging: MessagingState, forum: Forum, config: Ai, providers: Providers) -> Self {
        Self {
            catalog: CatalogCache::new(messaging.db.clone(), providers.clone()),
            jobs: Jobs::new(messaging.db.clone()),
            db: messaging.db.clone(),
            ids: messaging.ids.clone(),
            bus: messaging.bus.clone(),
            media: messaging.media.clone(),
            messaging,
            comments: forum.comments.clone(),
            forum,
            providers,
            config: Arc::new(config),
            story: None,
            mcp: McpClient::default(),
        }
    }

    /// 接上剧情库（组装时调用一次）。
    #[must_use]
    pub fn with_story(mut self, story: StoryState) -> Self {
        self.story = Some(story);
        self
    }

    /// 按配置里的提供方建上游客户端。
    pub fn from_config(
        messaging: MessagingState,
        forum: Forum,
        config: &Ai,
    ) -> anyhow::Result<Self> {
        let providers = Providers::from_config(&config.providers)?;
        Ok(Self::new(messaging, forum, config.clone(), providers))
    }

    /// 测试用的便捷构造。
    #[doc(hidden)]
    pub fn for_tests(
        db: PgPool,
        ids: Arc<Snowflake>,
        bus: Arc<dyn EventBus>,
        config: Ai,
    ) -> anyhow::Result<Self> {
        let media = MediaState::for_tests(db.clone(), ids.clone());
        Ok(Self::new(
            MessagingState::new(db.clone(), ids.clone(), bus.clone(), media.clone()),
            Forum::for_tests(db, ids, bus, media)?,
            config,
            Providers::default(),
        ))
    }

    /// 让私信（AI 会话里发消息、撤回消息）与评论（发表评论）通知本模块（组装时调用一次）。
    /// 两个钩子都注册成功才返回 true。
    pub fn install_hooks(&self) -> bool {
        let dm = self
            .messaging
            .set_hooks(Arc::new(crate::sessions::Hooks::new(self.clone())));
        let comments = self
            .comments
            .set_hooks(Arc::new(crate::comment::Hooks::new(self)));
        dm && comments
    }
}

impl std::fmt::Debug for AiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiState").finish_non_exhaustive()
    }
}
