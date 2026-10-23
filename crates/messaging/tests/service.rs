//! 私信 service 集成测试：真实 PostgreSQL，每个测试一个独立临时库。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ik_auth::Principal;
use ik_kernel::bus::LocalBus;
use ik_kernel::{Id, Role, Snowflake};
use ik_messaging::MessagingState;
use ik_messaging::dto::{
    EditMessageRequest, MessagesQuery, OpenDirectRequest, SendMessageRequest,
    UpdateConversationRequest,
};
use ik_messaging::interop;
use ik_messaging::service;
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

struct Harness {
    state: MessagingState,
    bus: Arc<LocalBus>,
    db: PgPool,
}

fn harness(db: PgPool) -> Harness {
    let bus = Arc::new(LocalBus::new(256));
    Harness {
        state: MessagingState::new(
            db.clone(),
            Arc::new(Snowflake::new(1).unwrap()),
            bus.clone(),
            ik_media::MediaState::for_tests(db.clone(), Arc::new(Snowflake::new(1).unwrap())),
        ),
        bus,
        db,
    }
}

fn principal(id: i64, exam_passed: bool) -> Principal {
    Principal {
        user_id: Id::new(id),
        session_id: Id::new(id),
        role: Role::User,
        exam_passed,
        is_ai_agent: false,
        level: i16::from(exam_passed),
    }
}

impl Harness {
    async fn media(&self, id: i64, owner: i64) {
        sqlx::query!(
            "INSERT INTO media_files (id, object_key, mime, size_bytes, sha256, receipt)
             VALUES ($1, $2, 'image/png', 100, $3, $4)",
            id,
            format!("m/{id}.png"),
            "b".repeat(64),
            format!("r{id}"),
        )
        .execute(&self.db)
        .await
        .unwrap();
        sqlx::query!(
            "INSERT INTO media_uploaders (media_id, user_id) VALUES ($1, $2)",
            id,
            owner
        )
        .execute(&self.db)
        .await
        .unwrap();
    }

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
        principal(id, true)
    }

    async fn open(&self, me: &Principal, peer: i64) -> i64 {
        service::open_direct(
            &self.state,
            me,
            OpenDirectRequest {
                user_id: Id::new(peer),
            },
        )
        .await
        .unwrap()
        .id
        .get()
    }

    async fn say(&self, me: &Principal, conversation: i64, content: &str) -> i64 {
        service::send(&self.state, me, conversation, text(content))
            .await
            .unwrap()
            .1
            .id
            .get()
    }

    async fn block(&self, blocker: i64, blocked: i64) {
        sqlx::query!(
            "INSERT INTO user_blocks (blocker_id, blocked_id) VALUES ($1, $2)",
            blocker,
            blocked
        )
        .execute(&self.db)
        .await
        .unwrap();
    }

    /// 把消息的发送时间拨回 `secs` 秒之前（编辑、撤回窗口）。
    async fn age_message(&self, id: i64, secs: f64) {
        sqlx::query!(
            "UPDATE messages SET created_at = now() - make_interval(secs => $2) WHERE id = $1",
            id,
            secs
        )
        .execute(&self.db)
        .await
        .unwrap();
    }

    async fn inbox_ids(&self, user: i64) -> Vec<i64> {
        interop::page(&self.db, &self.state.media, user, None, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.id.get())
            .collect()
    }
}

fn text(content: &str) -> SendMessageRequest {
    SendMessageRequest {
        image_ids: Vec::new(),
        content: content.to_owned(),
        reply_to_id: None,
        client_id: None,
    }
}

fn first_page() -> MessagesQuery {
    MessagesQuery {
        cursor: None,
        limit: None,
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn open_direct_is_idempotent_and_hidden_for_peer_until_first_message(db: PgPool) {
    let h = harness(db);
    let alice = h.user(1).await;
    let bob = h.user(2).await;

    let id = h.open(&alice, 2).await;
    assert_eq!(h.open(&alice, 2).await, id);
    assert_eq!(h.open(&bob, 1).await, id, "两个方向是同一个会话");
    // bob 主动打开后会话出现在他那边；重来一次，验证 alice 发消息也会让对方看到。
    service::leave(&h.state, &bob, id).await.unwrap();
    assert_eq!(h.inbox_ids(1).await, vec![id]);
    assert!(h.inbox_ids(2).await.is_empty());

    h.say(&alice, id, "在吗").await;
    assert_eq!(h.inbox_ids(2).await, vec![id], "收到消息后对方列表里恢复");
    let view = service::conversation(&h.state, &bob, id).await.unwrap();
    assert_eq!(view.peer.name, "user1");
    assert_eq!(view.unread_count, 1);
    assert_eq!(view.last_message.unwrap().preview, "在吗");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn new_conversation_stays_hidden_for_target(db: PgPool) {
    let h = harness(db);
    let alice = h.user(1).await;
    h.user(2).await;
    let id = h.open(&alice, 2).await;
    assert!(h.inbox_ids(2).await.is_empty(), "空会话不打扰对方");
    assert_eq!(
        service::conversation(&h.state, &principal(2, true), id)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_opens_create_one_conversation(db: PgPool) {
    let h = harness(db);
    let alice = h.user(1).await;
    let bob = h.user(2).await;
    let (a, b) = tokio::join!(
        service::open_direct(
            &h.state,
            &alice,
            OpenDirectRequest {
                user_id: Id::new(2)
            }
        ),
        service::open_direct(
            &h.state,
            &bob,
            OpenDirectRequest {
                user_id: Id::new(1)
            }
        ),
    );
    assert_eq!(a.unwrap().id, b.unwrap().id);
    let n = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM conversations"#)
        .fetch_one(&h.db)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn open_direct_rejections(db: PgPool) {
    let h = harness(db);
    let alice = h.user(1).await;
    h.user(2).await;
    h.user(3).await;
    h.user(4).await;
    let open = |me: Principal, peer: i64| {
        let state = h.state.clone();
        async move {
            service::open_direct(
                &state,
                &me,
                OpenDirectRequest {
                    user_id: Id::new(peer),
                },
            )
            .await
            .unwrap_err()
            .code()
        }
    };
    assert_eq!(open(alice.clone(), 1).await, "DM_SELF");
    assert_eq!(open(principal(1, false), 2).await, "EXAM_REQUIRED");
    assert_eq!(open(alice.clone(), 99).await, "NOT_FOUND");
    sqlx::query!("UPDATE users SET status = 'blocked' WHERE id = 3")
        .execute(&h.db)
        .await
        .unwrap();
    assert_eq!(open(alice.clone(), 3).await, "NOT_FOUND", "封禁账号");
    h.block(4, 1).await;
    assert_eq!(open(alice.clone(), 4).await, "USER_BLOCKED", "对方拉黑了我");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn send_checks_membership_blocks_and_reply_target(db: PgPool) {
    let h = harness(db);
    let alice = h.user(1).await;
    let bob = h.user(2).await;
    let carol = h.user(3).await;
    let id = h.open(&alice, 2).await;
    let other = h.open(&alice, 3).await;
    let first = h.say(&alice, id, "第一条").await;
    let elsewhere = h.say(&alice, other, "别处").await;

    let err = service::send(&h.state, &carol, id, text("插话"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "NOT_FOUND", "不是成员");

    let reply = |reply_to: i64| SendMessageRequest {
        image_ids: Vec::new(),
        content: "回复".into(),
        reply_to_id: Some(Id::new(reply_to)),
        client_id: None,
    };
    let (_, m) = service::send(&h.state, &bob, id, reply(first))
        .await
        .unwrap();
    let r = m.reply_to.unwrap();
    assert_eq!(r.content.as_deref(), Some("第一条"));
    assert_eq!(r.sender_id.unwrap().get(), 1);
    let err = service::send(&h.state, &bob, id, reply(elsewhere))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "REPLY_TARGET_INVALID", "别的会话的消息");
    service::withdraw(&h.state, &alice, first).await.unwrap();
    let err = service::send(&h.state, &bob, id, reply(first))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "REPLY_TARGET_INVALID", "已撤回");

    let err = service::send(&h.state, &bob, id, text("   "))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "VALIDATION_FAILED");

    h.block(1, 2).await;
    let err = service::send(&h.state, &bob, id, text("还在吗"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "USER_BLOCKED", "被拉黑后不能再发");
    let history = service::messages(&h.state, &bob, id, first_page())
        .await
        .unwrap();
    assert_eq!(history.items.len(), 2, "历史仍可查看");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn client_id_deduplicates_retries(db: PgPool) {
    let h = harness(db);
    let alice = h.user(1).await;
    h.user(2).await;
    h.user(3).await;
    let id = h.open(&alice, 2).await;
    let other = h.open(&alice, 3).await;
    let request = || SendMessageRequest {
        image_ids: Vec::new(),
        content: "只发一次".into(),
        reply_to_id: None,
        client_id: Some("0b9d1c3e-retry".into()),
    };
    let (created, first) = service::send(&h.state, &alice, id, request())
        .await
        .unwrap();
    assert!(created);
    let (created, again) = service::send(&h.state, &alice, id, request())
        .await
        .unwrap();
    assert!(!created);
    assert_eq!(first.id, again.id);
    assert_eq!(again.client_id.as_deref(), Some("0b9d1c3e-retry"));
    let err = service::send(&h.state, &alice, other, request())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "ALREADY_EXISTS");

    // 并发重试只落一条。
    let concurrent = || SendMessageRequest {
        image_ids: Vec::new(),
        content: "并发".into(),
        reply_to_id: None,
        client_id: Some("same-key".into()),
    };
    let (a, b) = tokio::join!(
        service::send(&h.state, &alice, id, concurrent()),
        service::send(&h.state, &alice, id, concurrent()),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.1.id, b.1.id);
    assert!(a.0 ^ b.0, "恰好一个是新建");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unread_counts_follow_the_read_watermark(db: PgPool) {
    let h = harness(db);
    let alice = h.user(1).await;
    let bob = h.user(2).await;
    let id = h.open(&alice, 2).await;
    h.say(&alice, id, "一").await;
    let two = h.say(&alice, id, "二").await;
    h.say(&alice, id, "三").await;

    let mut conn = h.db.acquire().await.unwrap();
    assert_eq!(interop::total_unread(&mut conn, 2).await.unwrap(), 3);
    assert_eq!(
        interop::total_unread(&mut conn, 1).await.unwrap(),
        0,
        "自己发的不算"
    );
    service::withdraw(&h.state, &alice, two).await.unwrap();
    assert_eq!(
        interop::total_unread(&mut conn, 2).await.unwrap(),
        2,
        "撤回的不算"
    );

    // bob 回复：发送即已读。
    h.say(&bob, id, "收到").await;
    assert_eq!(interop::total_unread(&mut conn, 2).await.unwrap(), 0);
    assert_eq!(interop::total_unread(&mut conn, 1).await.unwrap(), 1);
    let now = chrono::Utc::now();
    assert_eq!(
        interop::mark_read(&mut conn, 1, id, now).await.unwrap(),
        Some(2)
    );
    assert_eq!(interop::total_unread(&mut conn, 1).await.unwrap(), 0);
    // 水位只前进。
    interop::mark_read(&mut conn, 1, id, now - chrono::Duration::hours(1))
        .await
        .unwrap();
    assert_eq!(interop::total_unread(&mut conn, 1).await.unwrap(), 0);
    assert_eq!(
        interop::mark_read(&mut conn, 3, id, now).await.unwrap(),
        None,
        "非成员"
    );

    // 移除后不计未读。
    h.say(&bob, id, "再来一条").await;
    service::leave(&h.state, &alice, id).await.unwrap();
    assert_eq!(interop::total_unread(&mut conn, 1).await.unwrap(), 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn edit_and_withdraw_rules(db: PgPool) {
    let h = harness(db);
    let alice = h.user(1).await;
    let bob = h.user(2).await;
    let carol = h.user(3).await;
    let id = h.open(&alice, 2).await;
    let msg = h.say(&alice, id, "原文").await;
    let edit = |content: &str| EditMessageRequest {
        content: content.to_owned(),
    };

    let err = service::edit(&h.state, &bob, msg, edit("篡改"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "FORBIDDEN", "成员看得到，但不是自己的");
    let err = service::edit(&h.state, &carol, msg, edit("篡改"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "NOT_FOUND", "非成员不暴露存在");

    let unchanged = service::edit(&h.state, &alice, msg, edit("  原文 "))
        .await
        .unwrap();
    assert!(unchanged.edited_at.is_none(), "内容没变不记编辑");
    let edited = service::edit(&h.state, &alice, msg, edit("改过"))
        .await
        .unwrap();
    assert_eq!(edited.content.as_deref(), Some("改过"));
    assert!(edited.edited_at.is_some());

    let withdrawn = service::withdraw(&h.state, &alice, msg).await.unwrap();
    assert!(withdrawn.content.is_none(), "撤回后正文不可见");
    assert!(withdrawn.withdrawn_at.is_some());
    let again = service::withdraw(&h.state, &alice, msg).await.unwrap();
    assert_eq!(again.withdrawn_at, withdrawn.withdrawn_at, "幂等");
    let err = service::edit(&h.state, &alice, msg, edit("复活"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "MESSAGE_WITHDRAWN");
    let view = service::conversation(&h.state, &bob, id).await.unwrap();
    assert_eq!(view.last_message.unwrap().preview, "[消息已撤回]");

    let old = h.say(&alice, id, "很久以前").await;
    h.age_message(old, 301.0).await;
    let err = service::edit(&h.state, &alice, old, edit("改"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "MESSAGE_TOO_OLD");
    let err = service::withdraw(&h.state, &alice, old).await.unwrap_err();
    assert_eq!(err.code(), "MESSAGE_TOO_OLD");
    let fresh = h.say(&alice, id, "刚刚").await;
    h.age_message(fresh, 290.0).await;
    service::withdraw(&h.state, &alice, fresh).await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn message_pagination_is_stable_with_equal_timestamps(db: PgPool) {
    let h = harness(db);
    let alice = h.user(1).await;
    h.user(2).await;
    let id = h.open(&alice, 2).await;
    // 直接写库：走 service 会撞上每秒 5 条的限流。
    let sent: Vec<i64> = (1..=7).map(|i| 1000 + i).collect();
    for m in &sent {
        sqlx::query!(
            "INSERT INTO messages (id, conversation_id, sender_id, content) VALUES ($1, $2, 1, 'x')",
            m,
            id
        )
        .execute(&h.db)
        .await
        .unwrap();
    }
    // 一半消息挤到同一时刻，只能靠 id 破平。
    sqlx::query!(
        "UPDATE messages SET created_at = '2026-01-01T00:00:00Z' WHERE id = ANY($1)",
        &sent[2..5]
    )
    .execute(&h.db)
    .await
    .unwrap();
    let all = service::messages(
        &h.state,
        &alice,
        id,
        MessagesQuery {
            cursor: None,
            limit: Some(100),
        },
    )
    .await
    .unwrap();
    let expected: Vec<i64> = all.items.iter().map(|m| m.id.get()).collect();
    assert_eq!(expected.len(), 7);

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = service::messages(
            &h.state,
            &alice,
            id,
            MessagesQuery {
                cursor,
                limit: Some(2),
            },
        )
        .await
        .unwrap();
        seen.extend(page.items.iter().map(|m| m.id.get()));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, expected);
    let err = service::messages(
        &h.state,
        &alice,
        id,
        MessagesQuery {
            cursor: Some("garbage".into()),
            limit: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "CURSOR_INVALID");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn pinning_is_capped_even_under_concurrency(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    let mut conversations = Vec::new();
    for peer in 2..=52 {
        h.user(peer).await;
        conversations.push(h.open(&me, peer).await);
    }
    let pin = |id: i64| {
        let state = h.state.clone();
        let me = me.clone();
        async move {
            service::update(
                &state,
                &me,
                id,
                UpdateConversationRequest {
                    muted: None,
                    pinned: Some(true),
                },
            )
            .await
        }
    };
    for id in &conversations[..49] {
        pin(*id).await.unwrap();
    }
    // 再置顶已置顶的不占名额。
    assert!(pin(conversations[0]).await.unwrap().pinned);
    let (a, b) = tokio::join!(pin(conversations[49]), pin(conversations[50]));
    assert!(a.is_ok() ^ b.is_ok(), "第 50 个名额只能给一个");
    assert_eq!(
        a.err().or(b.err()).unwrap().code(),
        "DM_PIN_LIMIT",
        "超出上限"
    );
    let pinned = interop::pinned(&h.db, &h.state.media, 1).await.unwrap();
    assert_eq!(pinned.len(), 50);
    assert!(
        interop::page(&h.db, &h.state.media, 1, None, 100)
            .await
            .unwrap()
            .len()
            == 1,
        "置顶的不进分页流"
    );

    let muted = service::update(
        &h.state,
        &me,
        conversations[0],
        UpdateConversationRequest {
            muted: Some(true),
            pinned: Some(false),
        },
    )
    .await
    .unwrap();
    assert!(muted.muted && !muted.pinned);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn sending_is_rate_limited(db: PgPool) {
    let h = harness(db);
    let alice = h.user(1).await;
    h.user(2).await;
    let id = h.open(&alice, 2).await;
    for i in 0..5 {
        h.say(&alice, id, &format!("{i}")).await;
    }
    let err = service::send(&h.state, &alice, id, text("第六条"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "RATE_LIMITED");
    assert!(err.retry_after_secs().is_some());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn events_reach_present_members_after_commit(db: PgPool) {
    let h = harness(db);
    let alice = h.user(1).await;
    let bob = h.user(2).await;
    let id = h.open(&alice, 2).await;
    let mut rx = h.bus.subscribe();
    let msg = h.say(&alice, id, "你好").await;
    let event = rx.recv().await.unwrap();
    assert_eq!(event.kind, "dm.message.created");
    match &event.audience {
        ik_kernel::bus::Audience::Users(users) => {
            let mut users = users.to_vec();
            users.sort_unstable();
            assert_eq!(users, vec![1, 2], "含发送者本人（多端同步）");
        }
        other => panic!("unexpected audience {other:?}"),
    }
    assert_eq!(event.data["message"]["content"], "你好");

    service::typing(&h.state, 2, id).await.unwrap();
    let event = rx.recv().await.unwrap();
    assert_eq!(event.kind, "dm.typing");
    assert_eq!(event.audience, ik_kernel::bus::Audience::User(1));
    assert_eq!(
        service::typing(&h.state, 3, id).await.unwrap_err().code(),
        "NOT_FOUND"
    );

    service::withdraw(&h.state, &alice, msg).await.unwrap();
    let event = rx.recv().await.unwrap();
    assert_eq!(event.kind, "dm.message.withdrawn");
    assert!(event.data["message"].get("content").is_none());
    let _ = bob;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn peers_carry_avatars(db: PgPool) {
    let h = harness(db);
    let alice = h.user(1).await;
    h.user(2).await;
    sqlx::query!(
        "INSERT INTO media_files (id, object_key, mime, size_bytes, sha256, receipt)
         VALUES (900, 'a/900.png', 'image/png', 1, $1, 'r900')",
        "d".repeat(64)
    )
    .execute(&h.db)
    .await
    .unwrap();
    sqlx::query!("UPDATE users SET avatar_media_id = 900 WHERE id = 2")
        .execute(&h.db)
        .await
        .unwrap();
    let view = service::open_direct(
        &h.state,
        &alice,
        OpenDirectRequest {
            user_id: Id::new(2),
        },
    )
    .await
    .unwrap();
    assert!(
        view.peer
            .avatar_url
            .unwrap()
            .starts_with("https://cdn.test/a/900.png")
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn images_only_in_ai_conversations(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    let peer = h.user(2).await;
    h.media(900, 1).await;
    h.media(901, 2).await;
    let direct = h.open(&me, 2).await;
    let with_images = |ids: &[i64]| SendMessageRequest {
        content: String::new(),
        reply_to_id: None,
        client_id: None,
        image_ids: ids.iter().copied().map(Id::new).collect(),
    };
    // 真人私聊不能发图（D-021）。
    let err = service::send(&h.state, &me, direct, with_images(&[900]))
        .await
        .unwrap_err();
    assert_eq!(err.field_errors()[0].code, "NOT_ALLOWED");
    // 没图又没字：仍然要求正文。
    let err = service::send(&h.state, &me, direct, with_images(&[]))
        .await
        .unwrap_err();
    assert_eq!(err.field_errors()[0].code, "REQUIRED");

    // AI 会话（MSG-030）。
    sqlx::query!("UPDATE conversations SET kind = 'ai' WHERE id = $1", direct)
        .execute(&h.db)
        .await
        .unwrap();
    let err = service::send(&h.state, &me, direct, with_images(&[1, 2, 3, 4, 5]))
        .await
        .unwrap_err();
    assert_eq!(err.field_errors()[0].code, "TOO_MANY");
    let err = service::send(&h.state, &me, direct, with_images(&[901]))
        .await
        .unwrap_err();
    assert_eq!(err.field_errors()[0].code, "NOT_FOUND", "别人的图片");
    let (_, view) = service::send(&h.state, &me, direct, with_images(&[900, 900]))
        .await
        .unwrap();
    assert_eq!(view.content.as_deref(), Some(""));
    assert_eq!(view.images.len(), 1, "去重");
    assert_eq!(view.images[0].url, "https://cdn.test/m/900.png");

    // 列表与回复摘要：纯图片消息显示「[图片]」；消息列表带图片。
    let summary = interop::page(&h.db, &h.state.media, 2, None, 10)
        .await
        .unwrap();
    assert_eq!(
        summary[0].last_message.as_ref().map(|m| m.preview.as_str()),
        Some("[图片]")
    );
    let mut conn = h.db.acquire().await.unwrap();
    let listed = interop::messages(&mut conn, &h.state.media, direct, None, 10)
        .await
        .unwrap();
    assert_eq!(listed[0].images.len(), 1);
    drop(conn);
    let (_, reply) = service::send(
        &h.state,
        &peer,
        direct,
        SendMessageRequest {
            content: "看到了".into(),
            reply_to_id: Some(view.id),
            client_id: None,
            image_ids: Vec::new(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        reply.reply_to.and_then(|r| r.content).as_deref(),
        Some("[图片]")
    );

    // 撤回后图片随正文隐藏。
    let withdrawn = service::withdraw(&h.state, &me, view.id.get())
        .await
        .unwrap();
    assert!(withdrawn.images.is_empty());
}
