use std::num::NonZeroU32;
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Utc};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use ik_kernel::bus::{Effects, EventBus};
use ik_kernel::{Problem, Snowflake};
use ik_media::MediaState;
use sqlx::{PgConnection, PgPool};

/// 每个用户每分钟最多发 10 条评论（D-037 方案 A；具体数值待产品确认）。
const COMMENTS_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(10) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};

#[derive(Clone)]
pub struct CommentsState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub bus: Arc<dyn EventBus>,
    /// 评论图片、作者头像的展示数据与归属校验。
    pub media: MediaState,
    pub(crate) limiter: Arc<DefaultKeyedRateLimiter<i64>>,
    /// 上层（ik-ai）注册的钩子；组装时注册一次，所有克隆共享。
    hooks: Arc<OnceLock<Arc<dyn CommentHooks>>>,
}

/// 被回复的评论。
#[derive(Debug, Clone)]
pub struct ReplyToRef {
    pub id: i64,
    pub author_id: i64,
    pub is_anonymous: bool,
}

/// 一条刚写入的评论（还在发表的事务里）。
#[derive(Debug, Clone)]
pub struct CreatedComment {
    pub id: i64,
    pub post_id: i64,
    pub post_author_id: i64,
    pub post_is_anonymous: bool,
    /// 回复了哪条评论。
    pub reply_to: Option<ReplyToRef>,
    pub author_id: i64,
    pub author_is_ai: bool,
    /// 展开裸 @ 之后的正文（纯图片评论为空）。
    pub body: String,
    pub is_anonymous: bool,
    pub created_at: DateTime<Utc>,
}

/// 发表评论时的钩子（AI 评论触发，TRIG-010～014）。
///
/// 在发表的事务里调用：钩子的写入（任务入队）与评论同生共死（TRIG-010）；
/// 实时事件放进 `effects`，提交后发出。钩子不能调用外部服务。
#[async_trait::async_trait]
pub trait CommentHooks: Send + Sync {
    async fn created(
        &self,
        conn: &mut PgConnection,
        comment: &CreatedComment,
        effects: &mut Effects,
    ) -> Result<(), Problem>;
}

impl CommentsState {
    pub fn new(db: PgPool, ids: Arc<Snowflake>, bus: Arc<dyn EventBus>, media: MediaState) -> Self {
        Self {
            db,
            ids,
            bus,
            media,
            limiter: Arc::new(RateLimiter::keyed(Quota::per_minute(COMMENTS_PER_MINUTE))),
            hooks: Arc::new(OnceLock::new()),
        }
    }

    /// 注册钩子（只能一次；重复注册返回 false）。
    pub fn set_hooks(&self, hooks: Arc<dyn CommentHooks>) -> bool {
        self.hooks.set(hooks).is_ok()
    }

    pub(crate) fn hooks(&self) -> Option<&Arc<dyn CommentHooks>> {
        self.hooks.get()
    }
}
