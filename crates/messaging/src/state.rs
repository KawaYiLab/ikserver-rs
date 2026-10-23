use std::num::NonZeroU32;
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Utc};
use ik_kernel::Problem;
use ik_kernel::bus::Effects;
use sqlx::PgConnection;

use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use ik_kernel::Snowflake;
use ik_kernel::bus::EventBus;
use ik_media::MediaState;
use sqlx::PgPool;

/// 发消息：每个用户每秒 5 条、每分钟 60 条（MSG-020）。
const SENDS_PER_SECOND: NonZeroU32 = match NonZeroU32::new(5) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};
const SENDS_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(60) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};

#[derive(Clone)]
pub struct MessagingState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub bus: Arc<dyn EventBus>,
    /// 对方头像。
    pub media: MediaState,
    pub(crate) send_limits: Arc<SendLimits>,
    /// 上层（ik-ai）注册的钩子；组装时注册一次，所有克隆共享。
    hooks: Arc<OnceLock<Arc<dyn MessageHooks>>>,
}

/// 一条刚写入的用户消息（还在发送的事务里）。
#[derive(Debug, Clone)]
pub struct SentMessage {
    pub id: i64,
    pub conversation_id: i64,
    /// `direct` | `ai`
    pub conversation_kind: String,
    pub sender_id: i64,
    pub peer_id: i64,
    /// 带图时可以为空串。
    pub content: String,
    /// 图片数（MSG-030）。
    pub image_count: usize,
    pub created_at: DateTime<Utc>,
}

/// 消息写入时的钩子（AI 私聊触发，TRIG-020；撤回触发消息时停止生成，CTX-026）。
///
/// 在发送 / 撤回的事务里调用：钩子的写入（任务入队）与消息同生共死（TRIG-010）；
/// 实时事件放进 `effects`，提交后发出。钩子不能调用外部服务。
#[async_trait::async_trait]
pub trait MessageHooks: Send + Sync {
    async fn sent(
        &self,
        conn: &mut PgConnection,
        message: &SentMessage,
        effects: &mut Effects,
    ) -> Result<(), Problem>;

    async fn withdrawn(
        &self,
        conn: &mut PgConnection,
        message_id: i64,
        conversation_id: i64,
        effects: &mut Effects,
    ) -> Result<(), Problem>;
}

pub(crate) struct SendLimits {
    pub per_second: DefaultKeyedRateLimiter<i64>,
    pub per_minute: DefaultKeyedRateLimiter<i64>,
}

impl MessagingState {
    pub fn new(db: PgPool, ids: Arc<Snowflake>, bus: Arc<dyn EventBus>, media: MediaState) -> Self {
        Self {
            db,
            ids,
            bus,
            media,
            send_limits: Arc::new(SendLimits {
                per_second: RateLimiter::keyed(Quota::per_second(SENDS_PER_SECOND)),
                per_minute: RateLimiter::keyed(Quota::per_minute(SENDS_PER_MINUTE)),
            }),
            hooks: Arc::new(OnceLock::new()),
        }
    }

    /// 注册钩子（只能一次；重复注册返回 false）。
    pub fn set_hooks(&self, hooks: Arc<dyn MessageHooks>) -> bool {
        self.hooks.set(hooks).is_ok()
    }

    pub(crate) fn hooks(&self) -> Option<&Arc<dyn MessageHooks>> {
        self.hooks.get()
    }
}
