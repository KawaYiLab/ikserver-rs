//! 通知 service / interop 集成测试：真实 PostgreSQL，每个测试一个独立临时库。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use chrono::{Duration, Utc};
use ik_auth::Principal;
use ik_kernel::bus::{Effects, LocalBus};
use ik_kernel::{Id, Role, Snowflake};
use ik_notify::NotifyState;
use ik_notify::dto::{ListQuery, PageQuery};
use ik_notify::interop::{self, Kind, Notice, Target};
use ik_notify::service;
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

struct Harness {
    state: NotifyState,
    ids: Snowflake,
    db: PgPool,
}

fn harness(db: PgPool) -> Harness {
    Harness {
        state: NotifyState::new(db.clone(), Arc::new(LocalBus::new(64))),
        ids: Snowflake::new(1).unwrap(),
        db,
    }
}

impl Harness {
    async fn user(&self, id: i64) -> Principal {
        sqlx::query!(
            "INSERT INTO users (id, username, email, level, exam_passed_at)
             VALUES ($1, $2, $3, 1, now())",
            id,
            format!("user{id}"),
            format!("user{id}@example.com"),
        )
        .execute(&self.db)
        .await
        .unwrap();
        Principal {
            user_id: Id::new(id),
            session_id: Id::new(id),
            role: Role::User,
            exam_passed: true,
            is_ai_agent: false,
            level: 1,
        }
    }

    async fn post(&self, id: i64, author: i64) {
        sqlx::query!(
            "INSERT INTO posts (id, author_id, title, body) VALUES ($1, $2, '帖子标题', '')",
            id,
            author,
        )
        .execute(&self.db)
        .await
        .unwrap();
    }

    /// 发一条通知并提交，返回是否写入。`ago` 秒之前发生，便于控制顺序。
    async fn send(&self, recipient: i64, sender: i64, kind: Kind, post: i64, ago: i64) -> bool {
        let mut tx = self.db.begin().await.unwrap();
        let mut effects = Effects::new();
        let sent = interop::send(
            &mut tx,
            &self.ids,
            Notice {
                recipient_id: recipient,
                sender_id: sender,
                kind,
                post_id: Some(post),
                comment_id: None,
                post_title: Some("帖子标题"),
                excerpt: "",
            },
            Utc::now() - Duration::seconds(ago),
            &mut effects,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        sent
    }

    async fn count(&self, recipient: i64) -> i64 {
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM notifications WHERE recipient_id = $1"#,
            recipient
        )
        .fetch_one(&self.db)
        .await
        .unwrap()
    }
}

fn all() -> ListQuery {
    ListQuery {
        cursor: None,
        limit: None,
        unread: false,
    }
}

fn page() -> PageQuery {
    PageQuery {
        cursor: None,
        limit: None,
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn send_is_suppressed_for_self_blocks_and_inactive_recipients(db: PgPool) {
    let h = harness(db);
    h.user(1).await;
    h.user(2).await;
    h.user(3).await;
    h.user(4).await;
    h.post(10, 1).await;

    assert!(!h.send(1, 1, Kind::Like, 10, 0).await, "自己不通知自己");
    sqlx::query!("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES (1, 2)")
        .execute(&h.db)
        .await
        .unwrap();
    assert!(!h.send(1, 2, Kind::Like, 10, 0).await, "拉黑任一方向都不发");
    sqlx::query!("UPDATE users SET status = 'blocked' WHERE id = 4")
        .execute(&h.db)
        .await
        .unwrap();
    assert!(!h.send(4, 3, Kind::Like, 10, 0).await, "封禁账号不收通知");
    assert!(h.send(1, 3, Kind::Like, 10, 0).await);
    assert_eq!(h.count(1).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn buckets_group_by_sender_and_track_unread(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    h.user(2).await;
    h.user(3).await;
    h.post(10, 1).await;
    h.send(1, 2, Kind::Like, 10, 30).await;
    h.send(1, 3, Kind::Favorite, 10, 20).await;
    h.send(1, 2, Kind::Denny, 10, 10).await;

    let buckets = service::buckets(&h.state, &me, page()).await.unwrap();
    let keys: Vec<&str> = buckets.items.iter().map(|b| b.key.as_str()).collect();
    assert_eq!(keys, vec!["user-2", "user-3"], "按最后一条的时间倒序");
    assert_eq!(buckets.items[0].unread_count, 2);
    assert_eq!(buckets.items[0].last_summary, "给你的帖子投了丁尼");
    assert_eq!(buckets.items[0].peer.name, "user2");
    assert_eq!(service::unread_count(&h.state, &me).await.unwrap().count, 3);

    let items = service::bucket_items(&h.state, &me, "user-2", page())
        .await
        .unwrap();
    assert_eq!(items.items.len(), 2);
    assert_eq!(items.items[1].summary, "赞了你的帖子");
    assert_eq!(items.items[1].post_title.as_deref(), Some("帖子标题"));

    let read = service::mark_bucket_read(&h.state, &me, "user-2")
        .await
        .unwrap();
    assert_eq!(read.updated, 2);
    assert_eq!(service::unread_count(&h.state, &me).await.unwrap().count, 1);

    // 单条已读：幂等；别人的通知 404。
    let other = h.user(4).await;
    let id = service::list(&h.state, &me, all()).await.unwrap().items[1]
        .id
        .get();
    assert_eq!(
        service::mark_read(&h.state, &me, id).await.unwrap().updated,
        1
    );
    assert_eq!(
        service::mark_read(&h.state, &me, id).await.unwrap().updated,
        0
    );
    assert_eq!(
        service::mark_read(&h.state, &other, id)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
    assert_eq!(
        service::mark_all_read(&h.state, &me).await.unwrap().updated,
        0
    );
    assert!(
        service::bucket_items(&h.state, &me, "user-abc", page())
            .await
            .is_err()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bucket_pagination_is_stable(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    h.post(10, 1).await;
    for sender in 2..=6 {
        h.user(sender).await;
        h.send(1, sender, Kind::Like, 10, 100 - sender).await;
    }
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = service::buckets(
            &h.state,
            &me,
            PageQuery {
                cursor,
                limit: Some(2),
            },
        )
        .await
        .unwrap();
        seen.extend(page.items.iter().map(|b| b.key.clone()));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, vec!["user-6", "user-5", "user-4", "user-3", "user-2"]);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn retract_removes_notifications_and_empty_buckets(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    h.user(2).await;
    h.post(10, 1).await;
    h.post(11, 1).await;
    h.send(1, 2, Kind::Like, 10, 0).await;
    h.send(1, 2, Kind::Like, 11, 0).await;

    let mut tx = h.db.begin().await.unwrap();
    let mut effects = Effects::new();
    interop::retract(&mut tx, 2, Kind::Like, Target::Post(10), &mut effects)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(h.count(1).await, 1, "只撤回对这篇帖子的");

    let mut tx = h.db.begin().await.unwrap();
    interop::retract(&mut tx, 2, Kind::Like, Target::Post(11), &mut effects)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        service::buckets(&h.state, &me, page())
            .await
            .unwrap()
            .items
            .is_empty(),
        "会话里没有通知了，会话也消失"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn fanout_skips_blocked_ai_and_inactive_followers_and_is_idempotent(db: PgPool) {
    let h = harness(db);
    h.user(1).await;
    for id in 2..=6 {
        h.user(id).await;
        sqlx::query!(
            "INSERT INTO follows (follower_id, followee_id) VALUES ($1, 1)",
            id
        )
        .execute(&h.db)
        .await
        .unwrap();
    }
    h.post(10, 1).await;
    sqlx::query!("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES (1, 4)")
        .execute(&h.db)
        .await
        .unwrap();
    sqlx::query!("UPDATE users SET is_ai_agent = TRUE WHERE id = 5")
        .execute(&h.db)
        .await
        .unwrap();
    sqlx::query!("UPDATE users SET status = 'blocked' WHERE id = 6")
        .execute(&h.db)
        .await
        .unwrap();

    let fanout = || async {
        let mut tx = h.db.begin().await.unwrap();
        let mut effects = Effects::new();
        let n = interop::fanout_new_post(&mut tx, &h.ids, 1, 10, "新帖", Utc::now(), &mut effects)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        n
    };
    assert_eq!(fanout().await, 2);
    assert_eq!(fanout().await, 0, "同一帖子对同一人只通知一次");

    let follower = Principal {
        user_id: Id::new(2),
        session_id: Id::new(2),
        role: Role::User,
        exam_passed: true,
        is_ai_agent: false,
        level: 1,
    };
    let list = service::list(&h.state, &follower, all()).await.unwrap();
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].kind, "system");
    assert_eq!(list.items[0].summary, "你关注的 user1 发布了新委托《新帖》");
    assert!(list.items[0].sender.is_none());
    let buckets = service::buckets(&h.state, &follower, page()).await.unwrap();
    assert_eq!(buckets.items[0].key, "system");
    assert_eq!(buckets.items[0].peer.name, "Fairy");
    assert_eq!(h.count(3).await, 1);
    for skipped in [4, 5, 6] {
        assert_eq!(h.count(skipped).await, 0, "用户 {skipped}");
    }
}
