use std::sync::Arc;

use ik_kernel::bus::LocalBus;

use crate::Hub;

#[derive(Clone)]
pub struct RealtimeState {
    pub hub: Arc<Hub>,
    /// 允许发起 WebSocket 的网页来源（与 CORS / CSRF 白名单相同）。
    pub allowed_origins: Arc<[String]>,
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
        })
    }
}
