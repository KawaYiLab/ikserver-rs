//! 连接表与事件分发。
//!
//! 单进程实现：连接表在内存里，多实例部署时每个实例只推自己持有的连接
//! （总线换成 PostgreSQL LISTEN/NOTIFY 后各实例都能收到全部事件，本模块不变）。
//! 锁只在内存操作期间持有，从不跨 `.await`。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ik_kernel::bus::{Audience, Event};
use tokio::sync::{broadcast, mpsc, watch};

use crate::protocol::{
    CLOSE_EVICTED, CLOSE_GOING_AWAY, CLOSE_SLOW_CONSUMER, CloseReason, MAX_CONNECTIONS_PER_USER,
    MAX_SUBSCRIPTIONS, ServerFrame,
};

/// 每条连接的待发队列长度。写满说明客户端读得太慢，断开它（WS 只是提醒，REST 可补齐）。
const OUTBOUND_BUFFER: usize = 256;
/// 游客连接（只收广播与公开频道）的全局上限，超过时握手返回 503。
pub(crate) const MAX_ANONYMOUS_CONNECTIONS: usize = 5000;

/// 发给连接任务的消息。
#[derive(Debug)]
pub(crate) enum Outbound {
    /// 已序列化好的服务端帧。
    Frame(Arc<str>),
    /// 该用户的会话发生了吊销，连接任务去数据库复查自己的会话。
    Recheck,
}

/// 登记成功后交给连接任务的句柄。
pub(crate) struct Registration {
    pub id: u64,
    pub outbound: mpsc::Receiver<Outbound>,
    pub close: watch::Receiver<Option<CloseReason>>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SubscribeError {
    TooMany,
}

struct Conn {
    user_id: Option<i64>,
    session_id: Option<i64>,
    tx: mpsc::Sender<Outbound>,
    close: watch::Sender<Option<CloseReason>>,
    channels: HashSet<String>,
}

#[derive(Default)]
struct Registry {
    conns: HashMap<u64, Conn>,
    /// 每个用户的连接，按建立顺序（最旧在前）。
    by_user: HashMap<i64, VecDeque<u64>>,
    anonymous: usize,
    closing: bool,
}

impl Registry {
    fn close(&self, id: u64, reason: CloseReason) {
        if let Some(conn) = self.conns.get(&id) {
            // 已经在关闭的连接保留第一个原因。
            conn.close.send_if_modified(|current| {
                if current.is_none() {
                    *current = Some(reason);
                    true
                } else {
                    false
                }
            });
        }
    }
}

/// 本实例的在线快照。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Presence {
    /// 有连接的登录用户，最近建立连接的在前。
    pub user_ids: Vec<i64>,
    /// 游客连接数（游客没有身份，每条连接各算一个）。
    pub guests: usize,
}

pub struct Hub {
    registry: Mutex<Registry>,
    next_id: AtomicU64,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        Self {
            registry: Mutex::new(Registry::default()),
            next_id: AtomicU64::new(1),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Registry> {
        // 持锁期间只有内存操作，不会在中途 panic；即便中毒，表结构也仍然一致。
        self.registry.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 消费总线事件直到总线关闭。跟不上时丢掉最旧的事件（客户端靠 REST 补齐）。
    pub async fn run(self: Arc<Self>, mut events: broadcast::Receiver<Event>) {
        loop {
            match events.recv().await {
                Ok(event) => self.dispatch(&event),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "实时网关跟不上事件总线，丢弃了部分事件");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    /// 当前连接数（已登录, 游客）。
    pub fn connection_counts(&self) -> (usize, usize) {
        let registry = self.lock();
        (
            registry.conns.len() - registry.anonymous,
            registry.anonymous,
        )
    }

    /// 在线快照：已登录用户（按最近一次建立连接倒序）与游客连接数。
    pub fn presence(&self) -> Presence {
        let registry = self.lock();
        // 连接 id 单调递增，每个用户队列的最后一个就是他最新的连接。
        let mut users: Vec<(u64, i64)> = registry
            .by_user
            .iter()
            .filter_map(|(user_id, queue)| queue.back().map(|newest| (*newest, *user_id)))
            .collect();
        let guests = registry.anonymous;
        drop(registry);
        users.sort_unstable_by_key(|(newest, _)| std::cmp::Reverse(*newest));
        Presence {
            user_ids: users.into_iter().map(|(_, user_id)| user_id).collect(),
            guests,
        }
    }

    /// 停机：通知所有连接以 1001 关闭，并拒绝新连接。
    pub fn shutdown(&self) {
        let mut registry = self.lock();
        registry.closing = true;
        for id in registry.conns.keys() {
            registry.close(*id, CLOSE_GOING_AWAY);
        }
    }

    /// 登记一条连接。登录用户超过 5 条时挤掉最旧的（WS-004）；游客超过全局上限或正在停机时返回 None。
    pub(crate) fn register(&self, identity: Option<(i64, i64)>) -> Option<Registration> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, outbound) = mpsc::channel(OUTBOUND_BUFFER);
        let (close_tx, close) = watch::channel(None);
        let mut registry = self.lock();
        if registry.closing {
            return None;
        }
        match identity {
            Some((user_id, _)) => {
                let mut evicted = Vec::new();
                let queue = registry.by_user.entry(user_id).or_default();
                while queue.len() >= MAX_CONNECTIONS_PER_USER {
                    if let Some(oldest) = queue.pop_front() {
                        evicted.push(oldest);
                    }
                }
                queue.push_back(id);
                for oldest in evicted {
                    registry.close(oldest, CLOSE_EVICTED);
                }
            }
            None => {
                if registry.anonymous >= MAX_ANONYMOUS_CONNECTIONS {
                    return None;
                }
                registry.anonymous += 1;
            }
        }
        registry.conns.insert(
            id,
            Conn {
                user_id: identity.map(|(user, _)| user),
                session_id: identity.map(|(_, session)| session),
                tx,
                close: close_tx,
                channels: HashSet::new(),
            },
        );
        Some(Registration {
            id,
            outbound,
            close,
        })
    }

    pub(crate) fn unregister(&self, id: u64) {
        let mut registry = self.lock();
        let Some(conn) = registry.conns.remove(&id) else {
            return;
        };
        match conn.user_id {
            Some(user_id) => {
                if let Some(queue) = registry.by_user.get_mut(&user_id) {
                    queue.retain(|c| *c != id);
                    if queue.is_empty() {
                        registry.by_user.remove(&user_id);
                    }
                }
            }
            None => registry.anonymous = registry.anonymous.saturating_sub(1),
        }
    }

    pub(crate) fn subscribe(&self, id: u64, channel: String) -> Result<(), SubscribeError> {
        let mut registry = self.lock();
        let Some(conn) = registry.conns.get_mut(&id) else {
            return Ok(());
        };
        if !conn.channels.contains(&channel) && conn.channels.len() >= MAX_SUBSCRIPTIONS {
            return Err(SubscribeError::TooMany);
        }
        conn.channels.insert(channel);
        Ok(())
    }

    pub(crate) fn unsubscribe(&self, id: u64, channel: &str) {
        if let Some(conn) = self.lock().conns.get_mut(&id) {
            conn.channels.remove(channel);
        }
    }

    fn dispatch(&self, event: &Event) {
        if event.audience == Audience::Internal {
            return;
        }
        // 会话吊销是内部事件：不推给客户端，只让该用户的连接去复查。
        if event.kind == ik_auth::service::SESSIONS_CHANGED {
            if let Audience::User(user_id) = &event.audience {
                let targets = self.targets(&Audience::User(*user_id));
                for (_, tx) in targets {
                    // 队列满时连接已经快被判为慢消费者了，30 秒的定时复查兜底。
                    let _ = tx.try_send(Outbound::Recheck);
                }
            }
            return;
        }
        let frame = ServerFrame::Event {
            t: event.kind.to_owned(),
            d: (*event.data).clone(),
        };
        let text: Arc<str> = match serde_json::to_string(&frame) {
            Ok(text) => text.into(),
            Err(error) => {
                tracing::error!(%error, kind = event.kind, "序列化实时事件失败");
                return;
            }
        };
        let mut slow = Vec::new();
        for (id, tx) in self.targets(&event.audience) {
            if let Err(mpsc::error::TrySendError::Full(_)) =
                tx.try_send(Outbound::Frame(text.clone()))
            {
                slow.push(id);
            }
        }
        if !slow.is_empty() {
            let registry = self.lock();
            for id in slow {
                registry.close(id, CLOSE_SLOW_CONSUMER);
            }
        }
    }

    /// 受众对应的连接（复制出发送端后立即释放锁）。
    fn targets(&self, audience: &Audience) -> Vec<(u64, mpsc::Sender<Outbound>)> {
        let registry = self.lock();
        let of_user = |user_id: &i64| {
            registry
                .by_user
                .get(user_id)
                .into_iter()
                .flatten()
                .filter_map(|id| registry.conns.get(id).map(|c| (*id, c.tx.clone())))
                .collect::<Vec<_>>()
        };
        match audience {
            Audience::User(user_id) => of_user(user_id),
            Audience::Users(user_ids) => user_ids.iter().flat_map(of_user).collect(),
            Audience::Session(session_id) => registry
                .conns
                .iter()
                .filter(|(_, c)| c.session_id == Some(*session_id))
                .map(|(id, c)| (*id, c.tx.clone()))
                .collect(),
            Audience::Channel(channel) => registry
                .conns
                .iter()
                .filter(|(_, c)| c.channels.contains(channel.as_ref()))
                .map(|(id, c)| (*id, c.tx.clone()))
                .collect(),
            Audience::Broadcast => registry
                .conns
                .iter()
                .map(|(id, c)| (*id, c.tx.clone()))
                .collect(),
            Audience::BroadcastExcept(user_id) => registry
                .conns
                .iter()
                .filter(|(_, c)| c.user_id != Some(*user_id))
                .map(|(id, c)| (*id, c.tx.clone()))
                .collect(),
            Audience::Internal => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &'static str, audience: Audience) -> Event {
        Event::new(kind, audience, &serde_json::json!({ "n": 1 })).unwrap()
    }

    fn frames(reg: &mut Registration) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(item) = reg.outbound.try_recv() {
            match item {
                Outbound::Frame(text) => out.push(text.to_string()),
                Outbound::Recheck => out.push("recheck".into()),
            }
        }
        out
    }

    #[test]
    fn routes_by_audience() {
        let hub = Hub::new();
        let mut alice = hub.register(Some((1, 11))).unwrap();
        let mut bob = hub.register(Some((2, 21))).unwrap();
        let mut guest = hub.register(None).unwrap();
        hub.subscribe(guest.id, "post:9".into()).unwrap();

        hub.dispatch(&event("a", Audience::User(1)));
        hub.dispatch(&event("b", Audience::Users(Arc::from(vec![1, 2]))));
        hub.dispatch(&event("c", Audience::Session(21)));
        hub.dispatch(&event("d", Audience::Channel(Arc::from("post:9"))));
        hub.dispatch(&event("e", Audience::Broadcast));
        hub.dispatch(&event("f", Audience::BroadcastExcept(1)));

        let kinds = |reg: &mut Registration| -> Vec<String> {
            frames(reg)
                .iter()
                .map(|f| {
                    let v: serde_json::Value = serde_json::from_str(f).unwrap();
                    v["t"].as_str().unwrap().to_owned()
                })
                .collect()
        };
        assert_eq!(kinds(&mut alice), vec!["a", "b", "e"]);
        assert_eq!(kinds(&mut bob), vec!["b", "c", "e", "f"]);
        assert_eq!(kinds(&mut guest), vec!["d", "e", "f"]);
    }

    #[test]
    fn session_changes_trigger_recheck_and_are_not_forwarded() {
        let hub = Hub::new();
        let mut alice = hub.register(Some((1, 11))).unwrap();
        hub.dispatch(&event(
            ik_auth::service::SESSIONS_CHANGED,
            Audience::User(1),
        ));
        assert_eq!(frames(&mut alice), vec!["recheck"]);
    }

    #[test]
    fn sixth_connection_evicts_the_oldest() {
        let hub = Hub::new();
        let regs: Vec<Registration> = (0..6)
            .map(|i| hub.register(Some((1, 100 + i))).unwrap())
            .collect();
        assert_eq!(*regs[0].close.borrow(), Some(CLOSE_EVICTED));
        assert!(regs[1..].iter().all(|r| r.close.borrow().is_none()));
        // 被挤掉的连接已不在该用户的队列里，不再收事件。
        hub.dispatch(&event("x", Audience::User(1)));
        let mut regs = regs;
        assert!(frames(&mut regs[0]).is_empty());
        assert_eq!(frames(&mut regs[5]).len(), 1);
        assert_eq!(hub.connection_counts(), (6, 0), "注销由连接任务自己完成");
        hub.unregister(regs[0].id);
        assert_eq!(hub.connection_counts(), (5, 0));
    }

    #[test]
    fn presence_lists_users_by_latest_connection() {
        let hub = Hub::new();
        let a1 = hub.register(Some((1, 11))).unwrap();
        let _b = hub.register(Some((2, 21))).unwrap();
        let _a2 = hub.register(Some((1, 12))).unwrap();
        let g = hub.register(None).unwrap();
        let _g2 = hub.register(None).unwrap();
        assert_eq!(
            hub.presence(),
            Presence {
                user_ids: vec![1, 2],
                guests: 2
            }
        );
        hub.unregister(a1.id);
        hub.unregister(g.id);
        assert_eq!(
            hub.presence(),
            Presence {
                user_ids: vec![1, 2],
                guests: 1
            }
        );
    }

    #[test]
    fn slow_consumers_are_closed() {
        let hub = Hub::new();
        let reg = hub.register(None).unwrap();
        for _ in 0..=OUTBOUND_BUFFER {
            hub.dispatch(&event("x", Audience::Broadcast));
        }
        assert_eq!(*reg.close.borrow(), Some(CLOSE_SLOW_CONSUMER));
    }

    #[test]
    fn subscriptions_are_capped() {
        let hub = Hub::new();
        let reg = hub.register(None).unwrap();
        for i in 0..MAX_SUBSCRIPTIONS {
            hub.subscribe(reg.id, format!("post:{i}")).unwrap();
        }
        assert_eq!(
            hub.subscribe(reg.id, "post:x".into()),
            Err(SubscribeError::TooMany)
        );
        hub.subscribe(reg.id, "post:0".into()).unwrap();
        hub.unsubscribe(reg.id, "post:0");
        hub.subscribe(reg.id, "post:y".into()).unwrap();
    }

    #[test]
    fn shutdown_closes_everyone_and_refuses_new_connections() {
        let hub = Hub::new();
        let reg = hub.register(Some((1, 1))).unwrap();
        hub.shutdown();
        assert_eq!(*reg.close.borrow(), Some(CLOSE_GOING_AWAY));
        assert!(hub.register(None).is_none());
    }
}
