use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use ik_kernel::Problem;
use ik_kernel::bus::LocalBus;

use crate::Hub;

pub type TypingFuture = Pin<Box<dyn Future<Output = Result<(), Problem>> + Send>>;
/// 处理客户端的 `typing` 帧：`(用户 id, 会话 id)`。由 server 用私信模块的实现注入，
/// 网关本身不依赖任何业务域。返回错误时网关回 `error` 帧。
pub type TypingHandler = Arc<dyn Fn(i64, i64) -> TypingFuture + Send + Sync>;

#[derive(Clone)]
pub struct RealtimeState {
    pub hub: Arc<Hub>,
    /// 允许发起 WebSocket 的网页来源（与 CORS / CSRF 白名单相同）。
    pub allowed_origins: Arc<[String]>,
    pub(crate) typing: Option<TypingHandler>,
}

impl RealtimeState {
    /// 创建网关并在当前 tokio 运行时上启动分发任务（订阅 `bus`）。
    pub fn start(bus: &LocalBus, allowed_origins: Vec<String>) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| anyhow::anyhow!("实时网关需要在 tokio 运行时里创建：{e}"))?;
        let hub = Arc::new(Hub::new());
        runtime.spawn(hub.clone().run(bus.subscribe()));
        Ok(Self {
            hub,
            allowed_origins: allowed_origins.into(),
            typing: None,
        })
    }

    /// 开放 `typing` 操作。
    #[must_use]
    pub fn with_typing(mut self, handler: TypingHandler) -> Self {
        self.typing = Some(handler);
        self
    }
}
