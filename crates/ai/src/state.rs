use std::sync::Arc;

use ik_comments::CommentsState;
use ik_kernel::Snowflake;
use ik_kernel::bus::EventBus;
use ik_kernel::config::Ai;
use ik_kernel::jobs::Jobs;
use ik_media::MediaState;
use ik_messaging::MessagingState;
use sqlx::PgPool;

use crate::catalog::CatalogCache;
use crate::llm::Providers;

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
    pub jobs: Jobs,
    pub catalog: CatalogCache,
    pub providers: Providers,
    pub config: Arc<Ai>,
}

impl AiState {
    /// 数据库、ID、事件总线、媒体与私信共用。
    pub fn new(
        messaging: MessagingState,
        comments: CommentsState,
        config: Ai,
        providers: Providers,
    ) -> Self {
        Self {
            catalog: CatalogCache::new(messaging.db.clone()),
            jobs: Jobs::new(messaging.db.clone()),
            db: messaging.db.clone(),
            ids: messaging.ids.clone(),
            bus: messaging.bus.clone(),
            media: messaging.media.clone(),
            messaging,
            comments,
            providers,
            config: Arc::new(config),
        }
    }

    /// 按配置里的提供方建上游客户端。
    pub fn from_config(
        messaging: MessagingState,
        comments: CommentsState,
        config: &Ai,
    ) -> anyhow::Result<Self> {
        let providers = Providers::from_config(&config.providers)?;
        Ok(Self::new(messaging, comments, config.clone(), providers))
    }

    /// 测试用的便捷构造。
    #[doc(hidden)]
    pub fn for_tests(db: PgPool, ids: Arc<Snowflake>, bus: Arc<dyn EventBus>, config: Ai) -> Self {
        let media = MediaState::for_tests(db.clone(), ids.clone());
        Self::new(
            MessagingState::new(db.clone(), ids.clone(), bus.clone(), media.clone()),
            CommentsState::new(db, ids, bus, media),
            config,
            Providers::default(),
        )
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
