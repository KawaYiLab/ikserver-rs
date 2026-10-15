//! 多实例事件总线：经 PostgreSQL LISTEN/NOTIFY 把本实例发布的事件转给其他实例。
//!
//! - 发布：先发给本实例的 [`LocalBus`]（与单实例一样），再把非 `Internal` 的事件放进发送队列，由后台任务
//!   批量 `pg_notify`。`publish` 不阻塞调用方；队列满或数据库出错时丢弃并记日志——事件只是提醒，
//!   客户端靠 REST 补齐。
//! - 接收：每个实例一条 LISTEN 连接，收到其他实例的事件后以 [`Origin::Remote`] 发给本实例的 `LocalBus`；
//!   自己发出的跳过（本地已经发过）。断线自动重连，期间的事件丢失。
//! - NOTIFY 载荷上限约 8000 字节；超过的事件（例如上千个收件人的新帖通知）写进 `bus_outbox`，通知里只带 id，
//!   接收方按 id 读取。发送任务定期删除几分钟前的行。
//! - 同一事务里内容完全相同的 NOTIFY 会被合并，所以每条带递增序号。

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use sqlx::postgres::{PgListener, PgNotification};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::bus::{Audience, Event, EventBus, LocalBus, Origin};

const CHANNEL: &str = "ik_events";
/// 超过这个长度的载荷走 `bus_outbox`（NOTIFY 上限 8000 字节，留余量）。
const INLINE_MAX_BYTES: usize = 7000;
const QUEUE_CAPACITY: usize = 4096;
/// 一次 `pg_notify` 语句最多带几条。
const BATCH_MAX: usize = 100;
const OUTBOX_RETENTION: chrono::Duration = chrono::Duration::minutes(10);
const OUTBOX_SWEEP_EVERY: Duration = Duration::from_secs(5 * 60);
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
/// 事件类型名的上限（远超实际种类数），防止异常载荷把内存撑大。
const MAX_KINDS: usize = 1024;

pub struct ClusterBus {
    local: Arc<LocalBus>,
    queue: mpsc::Sender<Event>,
    stop: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl ClusterBus {
    /// 在当前 tokio 运行时上启动发送与接收任务。`node` 是本实例的节点号（与雪花 ID 相同，各实例不同）。
    pub fn start(local: Arc<LocalBus>, db: PgPool, node: u16) -> anyhow::Result<Arc<Self>> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| anyhow::anyhow!("集群事件总线需要在 tokio 运行时里启动：{e}"))?;
        let (queue, rx) = mpsc::channel(QUEUE_CAPACITY);
        let (stop, stopped) = watch::channel(false);
        let sender = runtime.spawn(send_loop(db.clone(), node, rx, stopped.clone()));
        let listener = runtime.spawn(listen_loop(db, node, local.clone(), stopped));
        Ok(Arc::new(Self {
            local,
            queue,
            stop,
            tasks: Mutex::new(vec![sender, listener]),
        }))
    }

    /// 停止后台任务并等它们关闭连接（测试结束前调用，否则临时库删不掉）。
    pub async fn shutdown(&self) {
        let _ = self.stop.send(true);
        let tasks = std::mem::take(&mut *self.tasks.lock());
        for task in tasks {
            let _ = task.await;
        }
    }
}

impl EventBus for ClusterBus {
    fn publish(&self, event: Event) {
        if event.audience != Audience::Internal
            && let Err(error) = self.queue.try_send(event.clone())
        {
            tracing::warn!(kind = event.kind, %error, "集群事件发送队列已满，丢弃事件");
        }
        self.local.publish(event);
    }
}

impl Drop for ClusterBus {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        for task in self.tasks.get_mut().drain(..) {
            task.abort();
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Wire {
    #[serde(rename = "n")]
    node: u16,
    #[serde(rename = "s")]
    seq: u64,
    #[serde(rename = "k")]
    kind: String,
    #[serde(rename = "a")]
    audience: WireAudience,
    #[serde(rename = "d")]
    data: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
enum WireAudience {
    User(i64),
    Users(Vec<i64>),
    Session(i64),
    Channel(String),
    Broadcast,
}

/// 通知载荷：`bus_outbox` 里的行，或完整事件。
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Notice {
    Stored {
        #[serde(rename = "n")]
        node: u16,
        #[serde(rename = "r")]
        id: i64,
    },
    Inline(Wire),
}

impl Wire {
    fn new(node: u16, seq: u64, event: &Event) -> Option<Self> {
        let audience = match &event.audience {
            Audience::User(id) => WireAudience::User(*id),
            Audience::Users(ids) => WireAudience::Users(ids.to_vec()),
            Audience::Session(id) => WireAudience::Session(*id),
            Audience::Channel(name) => WireAudience::Channel(name.to_string()),
            Audience::Broadcast => WireAudience::Broadcast,
            Audience::Internal => return None,
        };
        Some(Self {
            node,
            seq,
            kind: event.kind.to_owned(),
            audience,
            data: (*event.data).clone(),
        })
    }

    fn into_event(self) -> Option<Event> {
        let audience = match self.audience {
            WireAudience::User(id) => Audience::User(id),
            WireAudience::Users(ids) => Audience::Users(ids.into()),
            WireAudience::Session(id) => Audience::Session(id),
            WireAudience::Channel(name) => Audience::Channel(name.into()),
            WireAudience::Broadcast => Audience::Broadcast,
        };
        Some(Event {
            kind: intern(&self.kind)?,
            audience,
            data: Arc::new(self.data),
            origin: Origin::Remote,
        })
    }
}

/// `Event::kind` 是 `&'static str`：远端事件的类型名按首次出现泄漏一份，之后复用。
fn intern(kind: &str) -> Option<&'static str> {
    static KINDS: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut kinds = KINDS.get_or_init(Default::default).lock();
    if let Some(existing) = kinds.get(kind) {
        return Some(existing);
    }
    if kinds.len() >= MAX_KINDS {
        tracing::warn!(kind, "事件类型过多，丢弃远端事件");
        return None;
    }
    let leaked: &'static str = Box::leak(kind.to_owned().into_boxed_str());
    kinds.insert(leaked);
    Some(leaked)
}

async fn send_loop(
    db: PgPool,
    node: u16,
    mut rx: mpsc::Receiver<Event>,
    mut stopped: watch::Receiver<bool>,
) {
    let mut seq = 0u64;
    let mut sweep = tokio::time::interval(OUTBOX_SWEEP_EVERY);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = stopped.changed() => break,
            first = rx.recv() => {
                let Some(first) = first else { break };
                let mut batch = vec![first];
                while batch.len() < BATCH_MAX {
                    match rx.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
                let wires: Vec<Wire> = batch
                    .iter()
                    .filter_map(|event| {
                        seq = seq.wrapping_add(1);
                        Wire::new(node, seq, event)
                    })
                    .collect();
                if let Err(error) = send(&db, wires).await {
                    tracing::warn!(?error, count = batch.len(), "集群事件发送失败，已丢弃");
                }
            }
            _ = sweep.tick() => {
                let cutoff = chrono::Utc::now() - OUTBOX_RETENTION;
                if let Err(error) = sqlx::query!("DELETE FROM bus_outbox WHERE created_at < $1", cutoff)
                    .execute(&db)
                    .await
                {
                    tracing::warn!(?error, "清理 bus_outbox 失败");
                }
            }
        }
    }
}

async fn send(db: &PgPool, wires: Vec<Wire>) -> anyhow::Result<()> {
    let mut payloads = Vec::with_capacity(wires.len());
    for wire in wires {
        let text = serde_json::to_string(&wire)?;
        if text.len() <= INLINE_MAX_BYTES {
            payloads.push(text);
            continue;
        }
        let id = sqlx::query_scalar!(
            "INSERT INTO bus_outbox (payload) VALUES ($1) RETURNING id",
            serde_json::to_value(&wire)?
        )
        .fetch_one(db)
        .await?;
        payloads.push(serde_json::to_string(&Notice::Stored {
            node: wire.node,
            id,
        })?);
    }
    if !payloads.is_empty() {
        // 同一条语句里按数组顺序发出，接收方按同样的顺序收到。
        sqlx::query!(
            "SELECT pg_notify($1, t.p) FROM unnest($2::text[]) WITH ORDINALITY AS t(p, i) ORDER BY t.i",
            CHANNEL,
            &payloads,
        )
        .fetch_all(db)
        .await?;
    }
    Ok(())
}

async fn listen_loop(
    db: PgPool,
    node: u16,
    local: Arc<LocalBus>,
    mut stopped: watch::Receiver<bool>,
) {
    let mut listener = loop {
        let connected = async {
            let mut listener = PgListener::connect_with(&db).await?;
            listener.listen(CHANNEL).await?;
            Ok::<_, sqlx::Error>(listener)
        };
        tokio::select! {
            _ = stopped.changed() => return,
            result = connected => match result {
                Ok(listener) => break listener,
                Err(error) => {
                    tracing::warn!(?error, "集群事件总线连接失败，稍后重试");
                    tokio::time::sleep(RECONNECT_DELAY).await;
                }
            },
        }
    };
    loop {
        tokio::select! {
            _ = stopped.changed() => break,
            received = listener.recv() => match received {
                Ok(notification) => {
                    if let Some(event) = receive(&db, node, &notification).await {
                        local.publish(event);
                    }
                }
                Err(error) => {
                    // 下一次 recv 会自动重连；断线期间的事件丢失。
                    tracing::warn!(?error, "集群事件总线断线，正在重连");
                    tokio::time::sleep(RECONNECT_DELAY).await;
                }
            },
        }
    }
    if let Err(error) = listener.unlisten_all().await {
        tracing::debug!(?error, "取消监听失败");
    }
}

async fn receive(db: &PgPool, node: u16, notification: &PgNotification) -> Option<Event> {
    let notice: Notice = match serde_json::from_str(notification.payload()) {
        Ok(notice) => notice,
        Err(error) => {
            tracing::warn!(?error, "无法解析集群事件");
            return None;
        }
    };
    let wire = match notice {
        Notice::Inline(wire) if wire.node == node => return None,
        Notice::Stored { node: from, .. } if from == node => return None,
        Notice::Inline(wire) => wire,
        Notice::Stored { id, .. } => {
            let payload = sqlx::query_scalar!("SELECT payload FROM bus_outbox WHERE id = $1", id)
                .fetch_optional(db)
                .await;
            let payload = match payload {
                Ok(Some(payload)) => payload,
                Ok(None) => {
                    tracing::warn!(id, "bus_outbox 里的事件已经被清理");
                    return None;
                }
                Err(error) => {
                    tracing::warn!(?error, id, "读取 bus_outbox 失败");
                    return None;
                }
            };
            match serde_json::from_value(payload) {
                Ok(wire) => wire,
                Err(error) => {
                    tracing::warn!(?error, id, "无法解析集群事件");
                    return None;
                }
            }
        }
    };
    wire.into_event()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_round_trips_and_skips_internal() {
        let event = Event::new(
            "dm.message.created",
            Audience::Users(vec![1, 2].into()),
            &serde_json::json!({ "x": 1 }),
        )
        .unwrap();
        let wire = Wire::new(3, 9, &event).unwrap();
        let text = serde_json::to_string(&Notice::Inline(wire)).unwrap();
        let Notice::Inline(back) = serde_json::from_str(&text).unwrap() else {
            panic!("应当解析成完整事件");
        };
        assert_eq!((back.node, back.seq), (3, 9));
        let back = back.into_event().unwrap();
        assert_eq!(back.kind, "dm.message.created");
        assert_eq!(back.audience, Audience::Users(vec![1, 2].into()));
        assert_eq!(*back.data, serde_json::json!({ "x": 1 }));
        assert_eq!(back.origin, Origin::Remote);

        let stored = serde_json::to_string(&Notice::Stored { node: 3, id: 5 }).unwrap();
        assert!(matches!(
            serde_json::from_str(&stored).unwrap(),
            Notice::Stored { node: 3, id: 5 }
        ));
        let internal = Event::new("x", Audience::Internal, &()).unwrap();
        assert!(Wire::new(1, 1, &internal).is_none());
    }

    #[test]
    fn kinds_are_interned_once() {
        let a = intern("cluster.test.kind").unwrap();
        let b = intern(&String::from("cluster.test.kind")).unwrap();
        assert!(std::ptr::eq(a, b));
    }
}
