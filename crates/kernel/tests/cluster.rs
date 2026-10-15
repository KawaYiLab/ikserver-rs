//! 多实例事件总线：同一个数据库上的两个实例。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use ik_kernel::bus::{Audience, Event, EventBus, LocalBus, Origin};
use ik_kernel::cluster::ClusterBus;
use sqlx::PgPool;
use tokio::sync::broadcast;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

struct Node {
    local: Arc<LocalBus>,
    bus: Arc<ClusterBus>,
}

fn node(db: &PgPool, id: u16) -> Node {
    let local = Arc::new(LocalBus::new(64));
    let bus = ClusterBus::start(local.clone(), db.clone(), id).unwrap();
    Node { local, bus }
}

async fn next(rx: &mut broadcast::Receiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("5 秒内没有收到事件")
        .unwrap()
}

async fn nothing(rx: &mut broadcast::Receiver<Event>) {
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(rx.try_recv().is_err(), "不该收到事件");
}

/// 等 `from` 发出的事件能到达 `to`：反复发探针，直到对面收到一个远端探针。
async fn wait_ready(from: &Node, to: &mut broadcast::Receiver<Event>) {
    for _ in 0..50 {
        from.bus
            .publish(Event::new("cluster.probe", Audience::Broadcast, &()).unwrap());
        let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
        while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, to.recv()).await {
            if event.origin == Origin::Remote {
                return;
            }
        }
    }
    panic!("LISTEN 一直没有就绪");
}

/// 等在途的探针都到达，再清空。
async fn settle(receivers: &mut [&mut broadcast::Receiver<Event>]) {
    tokio::time::sleep(Duration::from_millis(300)).await;
    for rx in receivers {
        while rx.try_recv().is_ok() {}
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn events_reach_other_instances_once(db: PgPool) {
    let a = node(&db, 1);
    let b = node(&db, 2);
    let mut at_a = a.local.subscribe();
    let mut at_b = b.local.subscribe();
    wait_ready(&a, &mut at_b).await;
    wait_ready(&b, &mut at_a).await;
    settle(&mut [&mut at_a, &mut at_b]).await;

    let event = Event::new(
        "dm.message.created",
        Audience::User(7),
        &serde_json::json!({ "body": "你好" }),
    )
    .unwrap();
    a.bus.publish(event.clone());
    a.bus.publish(event);

    // 本实例立刻收到（本地发布），不会再从数据库绕回来一次。
    for _ in 0..2 {
        assert_eq!(next(&mut at_a).await.origin, Origin::Local);
    }
    nothing(&mut at_a).await;
    // 对面收到两次（内容相同也不会被合并），标记为远端。
    for _ in 0..2 {
        let got = next(&mut at_b).await;
        assert_eq!(got.kind, "dm.message.created");
        assert_eq!(got.audience, Audience::User(7));
        assert_eq!(got.data["body"], "你好");
        assert_eq!(got.origin, Origin::Remote);
    }

    // 只给进程内订阅者的事件不出本实例。
    a.bus
        .publish(Event::new("comment.removed", Audience::Internal, &()).unwrap());
    assert_eq!(next(&mut at_a).await.kind, "comment.removed");
    nothing(&mut at_b).await;

    a.bus.shutdown().await;
    b.bus.shutdown().await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn large_events_go_through_the_outbox(db: PgPool) {
    let a = node(&db, 1);
    let b = node(&db, 2);
    let mut at_b = b.local.subscribe();
    wait_ready(&a, &mut at_b).await;
    settle(&mut [&mut at_b]).await;

    // 2000 个收件人远超 NOTIFY 的 8000 字节上限。
    let recipients: Vec<i64> = (1_000_000_000..1_000_002_000).collect();
    a.bus.publish(
        Event::new(
            "notification.created",
            Audience::Users(recipients.clone().into()),
            &serde_json::json!({ "bucket": "system" }),
        )
        .unwrap(),
    );
    // 顺序不乱：大事件之后的小事件后到。
    a.bus
        .publish(Event::new("after", Audience::Broadcast, &()).unwrap());

    let got = next(&mut at_b).await;
    assert_eq!(got.kind, "notification.created");
    assert_eq!(got.audience, Audience::Users(recipients.into()));
    assert_eq!(next(&mut at_b).await.kind, "after");
    let stored = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM bus_outbox"#)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(stored, 1);

    a.bus.shutdown().await;
    b.bus.shutdown().await;
}
