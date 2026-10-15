//! 实时网关：单条 WebSocket `/v1/realtime`，取代旧系统的 DM WebSocket、敲敲 SSE、文章流 SSE 三条通道。
//!
//! 协议见 `docs/api-guidelines.md` 实时章节；业务规则见 `docs/business-rules/messaging-notifications.md`
//! 第 3 节。各域在事务提交后把事件发到进程内总线（`ik_kernel::bus`），[`Hub`] 是总线唯一的订阅者，
//! 按受众（用户 / 会话 / 频道 / 广播）转发给连接。WS 只负责「提醒」，REST 是事实来源。

mod conn;
mod hub;
pub mod protocol;
mod routes;
mod state;

pub use hub::Hub;
pub use routes::router;
pub use state::RealtimeState;
