//! 进程内事件总线：业务事务提交后发布实时事件，WebSocket 网关订阅并按受众推送。
//!
//! 发布方不假设订阅方在线；事件丢失由客户端通过 REST 补齐。
//! 需要多实例部署时，换成基于 PostgreSQL LISTEN/NOTIFY 的实现，调用方不变。

use std::sync::Arc;

use serde::Serialize;
use tokio::sync::broadcast;

use crate::Problem;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience {
    User(i64),
    Users(Arc<[i64]>),
    /// 只发给某个会话（例如会话被吊销时通知该连接）。
    Session(i64),
    /// 订阅了某频道的连接（例如某个私信会话、某个信息流）。
    Channel(Arc<str>),
    Broadcast,
}

#[derive(Debug, Clone)]
pub struct Event {
    /// 事件类型，如 `dm.message.created`。
    pub kind: &'static str,
    pub audience: Audience,
    pub data: Arc<serde_json::Value>,
}

impl Event {
    pub fn new<T: Serialize>(
        kind: &'static str,
        audience: Audience,
        data: &T,
    ) -> Result<Self, Problem> {
        let data = serde_json::to_value(data).map_err(|e| Problem::internal_from(&e))?;
        Ok(Self {
            kind,
            audience,
            data: Arc::new(data),
        })
    }
}

pub trait EventBus: Send + Sync + 'static {
    fn publish(&self, event: Event);
}

/// 单进程实现。订阅者处理过慢时会丢失最旧的事件（由订阅方自行感知 `Lagged`）。
#[derive(Debug, Clone)]
pub struct LocalBus {
    sender: broadcast::Sender<Event>,
}

impl LocalBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

impl EventBus for LocalBus {
    fn publish(&self, event: Event) {
        // 没有订阅者时 send 返回错误，这是正常情况。
        let _ = self.sender.send(event);
    }
}

/// 事务提交后才执行的副作用收集器。service 在事务内收集，提交成功后由调用方统一派发。
#[derive(Debug, Default)]
#[must_use = "副作用必须在事务提交后派发，否则事件会丢失"]
pub struct Effects {
    events: Vec<Event>,
}

impl Effects {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&mut self, event: Event) {
        self.events.push(event);
    }

    pub fn extend(&mut self, other: Self) {
        self.events.extend(other.events);
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn dispatch(self, bus: &dyn EventBus) {
        for event in self.events {
            bus.publish(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn effects_dispatch_after_commit() {
        let bus = LocalBus::new(8);
        let mut rx = bus.subscribe();
        let mut effects = Effects::new();
        effects.publish(
            Event::new("test.ping", Audience::User(1), &serde_json::json!({"n": 1})).unwrap(),
        );
        assert!(rx.try_recv().is_err(), "提交前不得发布");
        effects.dispatch(&bus);
        let event = rx.recv().await.unwrap();
        assert_eq!(event.kind, "test.ping");
        assert_eq!(event.audience, Audience::User(1));
    }

    #[test]
    fn publish_without_subscribers_is_noop() {
        LocalBus::new(1).publish(Event::new("x", Audience::Broadcast, &()).unwrap());
    }
}
