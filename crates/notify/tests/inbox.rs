//! 统一收件箱集成测试：私聊会话与通知分组的双源归并、合并、已读。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use ik_auth::Principal;
use ik_kernel::bus::{Effects, LocalBus};
use ik_kernel::{Id, Role, Snowflake};
use ik_messaging::MessagingState;
use ik_messaging::dto::{OpenDirectRequest, SendMessageRequest};
use ik_notify::NotifyState;
use ik_notify::dto::{InboxEntry, InboxItem, PageQuery};
use ik_notify::inbox;
use ik_notify::interop::{self, Kind, Notice};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

struct Harness {
    notify: NotifyState,
    dm: MessagingState,
    ids: Arc<Snowflake>,
    db: PgPool,
}

fn harness(db: PgPool) -> Harness {
    let bus = Arc::new(LocalBus::new(256));
    let ids = Arc::new(Snowflake::new(1).unwrap());
    Harness {
        notify: NotifyState::new(
            db.clone(),
            bus.clone(),
            ik_media::MediaState::for_tests(db.clone(), ids.clone()),
        ),
        dm: MessagingState::new(
            db.clone(),
            ids.clone(),
            bus,
            ik_media::MediaState::for_tests(db.clone(), ids.clone()),
        ),
        ids,
        db,
    }
}

fn base() -> DateTime<Utc> {
    Utc::now() - Duration::hours(1)
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
            "INSERT INTO posts (id, author_id, title, body) VALUES ($1, $2, '帖子', '')",
            id,
            author,
        )
        .execute(&self.db)
        .await
        .unwrap();
    }

    /// 一条点赞通知，发生在 `base() + secs`。
    async fn like(&self, recipient: i64, sender: i64, post: i64, secs: i64) {
        let mut tx = self.db.begin().await.unwrap();
        let mut effects = Effects::new();
        assert!(
            interop::send(
                &mut tx,
                &self.ids,
                Notice {
                    recipient_id: recipient,
                    sender_id: sender,
                    kind: Kind::Like,
                    post_id: Some(post),
                    comment_id: None,
                    post_title: Some("帖子"),
                    excerpt: "",
                },
                base() + Duration::seconds(secs),
                &mut effects,
            )
            .await
            .unwrap()
        );
        tx.commit().await.unwrap();
    }

    /// 打开私聊并发一条消息，再把会话双方的排序时间与消息时间拨到 `base() + secs`。
    async fn conversation(&self, from: &Principal, to: i64, secs: i64) -> i64 {
        let id = ik_messaging::service::open_direct(
            &self.dm,
            from,
            OpenDirectRequest {
                user_id: Id::new(to),
            },
        )
        .await
        .unwrap()
        .id
        .get();
        ik_messaging::service::send(
            &self.dm,
            from,
            id,
            SendMessageRequest {
                content: format!("hi {to}"),
                reply_to_id: None,
                client_id: None,
                image_ids: Vec::new(),
            },
        )
        .await
        .unwrap();
        let at = base() + Duration::seconds(secs);
        sqlx::query!(
            "UPDATE conversation_members SET activity_at = $2, last_read_at = NULL
             WHERE conversation_id = $1",
            id,
            at
        )
        .execute(&self.db)
        .await
        .unwrap();
        sqlx::query!(
            "UPDATE messages SET created_at = $2 WHERE conversation_id = $1",
            id,
            at
        )
        .execute(&self.db)
        .await
        .unwrap();
        id
    }

    async fn all_pages(&self, me: &Principal, limit: u32) -> Vec<String> {
        let mut keys = Vec::new();
        let mut cursor = None;
        loop {
            let page = inbox::list(
                &self.notify,
                me,
                PageQuery {
                    cursor,
                    limit: Some(limit),
                },
            )
            .await
            .unwrap();
            keys.extend(page.items.into_iter().map(|e| e.key));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        keys
    }
}

fn first() -> PageQuery {
    PageQuery {
        cursor: None,
        limit: None,
    }
}

fn find<'a>(items: &'a [InboxEntry], key: &str) -> &'a InboxEntry {
    items.iter().find(|e| e.key == key).unwrap()
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn conversations_and_buckets_interleave_and_merge_by_peer(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    for id in 2..=5 {
        h.user(id).await;
    }
    h.post(10, 1).await;
    h.post(11, 1).await;
    // 2：只有通知；3：只有私聊；4：私聊 + 通知（合并）；5：系统通知没有，就一条点赞。
    h.like(1, 2, 10, 50).await;
    let with_3 = h.conversation(&h.user_ref(3), 1, 40).await;
    let with_4 = h.conversation(&h.user_ref(4), 1, 10).await;
    h.like(1, 4, 10, 60).await;
    h.like(1, 4, 11, 61).await;
    h.like(1, 5, 10, 30).await;

    let page = inbox::list(&h.notify, &me, first()).await.unwrap();
    let keys: Vec<&str> = page.items.iter().map(|e| e.key.as_str()).collect();
    let dm3 = format!("dm-{with_3}");
    let dm4 = format!("dm-{with_4}");
    assert_eq!(
        keys,
        vec![dm4.as_str(), "user-2", dm3.as_str(), "user-5"],
        "通知推进了与 4 的会话；4 的通知不单独成项"
    );
    let merged = find(&page.items, &dm4);
    assert_eq!(merged.kind, "conversation");
    assert_eq!(merged.unread_count, 3, "1 条私信 + 2 条通知");
    let preview = merged.preview.as_ref().unwrap();
    assert_eq!(preview.kind, "notification", "通知比最后一条私信新");
    assert_eq!(preview.text, "赞了你的帖子");
    let bucket = find(&page.items, "user-2");
    assert_eq!(bucket.kind, "notifications");
    assert_eq!(bucket.peer.name, "user2");
    assert_eq!(page.total_unread, Some(6));
    assert_eq!(inbox::unread_count(&h.notify, &me).await.unwrap().count, 6);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn conversation_entries_carry_kind_and_ai_title(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    h.user(2).await;
    h.user(3).await;
    h.post(10, 1).await;
    let direct = h.conversation(&h.user_ref(2), 1, 10).await;
    let ai = h.conversation(&h.user_ref(3), 1, 20).await;
    sqlx::query!(
        "UPDATE conversations SET kind = 'ai', title = '攻略问答' WHERE id = $1",
        ai
    )
    .execute(&h.db)
    .await
    .unwrap();
    h.like(1, 2, 10, 30).await;

    let page = inbox::list(&h.notify, &me, first()).await.unwrap();
    let direct = find(&page.items, &format!("dm-{direct}"));
    assert_eq!(direct.conversation_kind.as_deref(), Some("direct"));
    assert_eq!(direct.title, None);
    let ai = find(&page.items, &format!("dm-{ai}"));
    assert_eq!(ai.conversation_kind.as_deref(), Some("ai"));
    assert_eq!(ai.title.as_deref(), Some("攻略问答"));

    // 通知分组不是会话：两个字段都没有。
    h.user(4).await;
    h.like(1, 4, 10, 40).await;
    let page = inbox::list(&h.notify, &me, first()).await.unwrap();
    let bucket = find(&page.items, "user-4");
    assert_eq!(bucket.conversation_kind, None);
    assert_eq!(bucket.title, None);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn pagination_across_sources_is_stable_with_ties(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    h.post(10, 1).await;
    // 交错且大量同一时刻：通知分组与会话在同一秒时，通知排前面。
    let mut expected_len = 0;
    for peer in 2..=13 {
        h.user(peer).await;
        let secs = (peer / 3) * 10;
        if peer % 2 == 0 {
            h.like(1, peer, 10, secs).await;
        } else {
            h.conversation(&h.user_ref(peer), 1, secs).await;
        }
        expected_len += 1;
    }
    let all = h.all_pages(&me, 50).await;
    assert_eq!(all.len(), expected_len);
    for limit in 1..=5 {
        assert_eq!(h.all_pages(&me, limit).await, all, "limit = {limit}");
    }
    // 同一时刻：通知在前。
    let page = inbox::list(&h.notify, &me, first()).await.unwrap();
    for pair in page.items.windows(2) {
        assert!(pair[0].activity_at >= pair[1].activity_at);
        if pair[0].activity_at == pair[1].activity_at {
            assert!(!(pair[0].kind == "conversation" && pair[1].kind == "notifications"));
        }
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn pinned_conversations_lead_the_first_page_only(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    h.post(10, 1).await;
    for peer in 2..=4 {
        h.user(peer).await;
        h.like(1, peer, 10, peer * 10).await;
    }
    h.user(5).await;
    let old = h.conversation(&h.user_ref(5), 1, 0).await;
    ik_messaging::service::update(
        &h.dm,
        &me,
        old,
        ik_messaging::dto::UpdateConversationRequest {
            muted: None,
            pinned: Some(true),
        },
    )
    .await
    .unwrap();
    let page = inbox::list(
        &h.notify,
        &me,
        PageQuery {
            cursor: None,
            limit: Some(2),
        },
    )
    .await
    .unwrap();
    assert_eq!(page.items.len(), 3, "置顶不占分页名额");
    assert!(page.items[0].pinned);
    let all = h.all_pages(&me, 2).await;
    assert_eq!(
        all,
        vec![
            format!("dm-{old}"),
            "user-4".into(),
            "user-3".into(),
            "user-2".into()
        ]
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn reading_a_conversation_also_reads_the_peers_notifications(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    h.user(2).await;
    h.user(3).await;
    h.post(10, 1).await;
    let id = h.conversation(&h.user_ref(2), 1, 0).await;
    h.like(1, 2, 10, 5).await;
    h.like(1, 3, 10, 6).await;

    let key = format!("dm-{id}");
    let read = inbox::mark_read(&h.notify, &me, &key).await.unwrap();
    assert!(read.last_read_at.is_some());
    assert_eq!(read.notifications_updated, 1);
    let page = inbox::list(&h.notify, &me, first()).await.unwrap();
    assert_eq!(find(&page.items, &key).unread_count, 0);
    assert_eq!(find(&page.items, "user-3").unread_count, 1);

    let read = inbox::mark_read(&h.notify, &me, "user-3").await.unwrap();
    assert_eq!(read.notifications_updated, 1);
    assert!(read.last_read_at.is_none());
    assert_eq!(inbox::unread_count(&h.notify, &me).await.unwrap().count, 0);

    let stranger = h.user(9).await;
    assert_eq!(
        inbox::mark_read(&h.notify, &stranger, &key)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
    assert!(inbox::mark_read(&h.notify, &me, "dm-abc").await.is_err());

    h.like(1, 3, 10, 7).await;
    h.conversation(&h.user_ref(3), 1, 8).await;
    let all = inbox::mark_all_read(&h.notify, &me).await.unwrap();
    assert_eq!(all.notifications_updated, 1);
    assert_eq!(inbox::unread_count(&h.notify, &me).await.unwrap().count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn conversation_timeline_mixes_messages_and_notifications(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    h.user(2).await;
    h.user(3).await;
    h.post(10, 1).await;
    h.post(11, 1).await;
    let id = h.conversation(&h.user_ref(2), 1, 10).await;
    h.like(1, 2, 10, 5).await;
    h.like(1, 2, 11, 10).await; // 与消息同一时刻：通知在前。
    h.like(1, 3, 10, 20).await; // 别人的通知不混进来。

    let key = format!("dm-{id}");
    let collect = |limit: u32| {
        let notify = h.notify.clone();
        let me = me.clone();
        let key = key.clone();
        async move {
            let mut out = Vec::new();
            let mut cursor = None;
            loop {
                let page = inbox::items(
                    &notify,
                    &me,
                    &key,
                    PageQuery {
                        cursor,
                        limit: Some(limit),
                    },
                )
                .await
                .unwrap();
                out.extend(page.items.into_iter().map(|item| match item {
                    InboxItem::Message { message } => format!("m:{}", message.content.unwrap()),
                    InboxItem::Notification { notification } => {
                        format!("n:{}", notification.post_id.unwrap())
                    }
                }));
                match page.next_cursor {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
            out
        }
    };
    let expected = vec!["n:11", "m:hi 1", "n:10"];
    assert_eq!(collect(50).await, expected);
    assert_eq!(collect(1).await, expected);

    let bucket = inbox::items(&h.notify, &me, "user-3", first())
        .await
        .unwrap();
    assert_eq!(bucket.items.len(), 1);
    let other = h.user(4).await;
    assert_eq!(
        inbox::items(&h.notify, &other, &key, first())
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn ai_sessions_do_not_mix_in_the_ais_notifications(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    h.user(3).await;
    h.post(10, 1).await;
    // 3 是 AI：一条 AI 会话，外加它在评论区发来的通知。
    let ai = h.conversation(&h.user_ref(3), 1, 10).await;
    sqlx::query!("UPDATE conversations SET kind = 'ai' WHERE id = $1", ai)
        .execute(&h.db)
        .await
        .unwrap();
    h.like(1, 3, 10, 20).await;

    let key = format!("dm-{ai}");
    let page = inbox::list(&h.notify, &me, first()).await.unwrap();
    let keys: Vec<&str> = page.items.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(
        keys,
        vec!["user-3", key.as_str()],
        "通知单独成项，不推进 AI 会话"
    );
    let entry = find(&page.items, &key);
    assert_eq!(entry.unread_count, 1, "只算私信");
    assert_eq!(entry.preview.as_ref().unwrap().kind, "message");
    assert_eq!(find(&page.items, "user-3").unread_count, 1);

    let timeline = inbox::items(&h.notify, &me, &key, first()).await.unwrap();
    assert_eq!(timeline.items.len(), 1);
    assert!(matches!(timeline.items[0], InboxItem::Message { .. }));

    let read = inbox::mark_read(&h.notify, &me, &key).await.unwrap();
    assert_eq!(read.notifications_updated, 0, "读 AI 会话不顺带读通知");
    assert_eq!(inbox::unread_count(&h.notify, &me).await.unwrap().count, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn notifications_bump_the_conversation_but_do_not_revive_it(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    h.user(2).await;
    h.user(3).await;
    h.post(10, 1).await;
    let id = h.conversation(&h.user_ref(2), 1, 0).await;
    h.like(1, 3, 10, 10).await;
    h.like(1, 2, 10, 20).await;
    let keys = h.all_pages(&me, 50).await;
    assert_eq!(keys, vec![format!("dm-{id}"), "user-3".into()]);

    // 移除会话后，对方的通知重新单独成项；通知不会让会话复活。
    ik_messaging::service::leave(&h.dm, &me, id).await.unwrap();
    h.like(1, 2, 10, 30).await;
    let keys = h.all_pages(&me, 50).await;
    assert_eq!(keys, vec!["user-2".to_owned(), "user-3".into()]);
}

impl Harness {
    fn user_ref(&self, id: i64) -> Principal {
        Principal {
            user_id: Id::new(id),
            session_id: Id::new(id),
            role: Role::User,
            exam_passed: true,
            is_ai_agent: false,
            level: 1,
        }
    }
}
