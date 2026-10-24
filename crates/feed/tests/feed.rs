//! 信息流集成测试：真实 PostgreSQL，每个测试一个独立临时库。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ik_auth::Principal;
use ik_comments::CommentsState;
use ik_comments::dto::CreateCommentRequest;
use ik_content::ContentState;
use ik_content::dto::{FeedQuery, MarkReadRequest, PostInput};
use ik_feed::FeedState;
use ik_feed::service::{self, HotQuery};
use ik_kernel::bus::LocalBus;
use ik_kernel::config::Hot;
use ik_kernel::{Id, Role, Snowflake};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

struct Harness {
    feed: FeedState,
    content: ContentState,
    comments: CommentsState,
    db: PgPool,
}

fn harness(db: PgPool) -> Harness {
    let bus = Arc::new(LocalBus::new(1024));
    let ids = Arc::new(Snowflake::new(1).unwrap());
    let media = ik_media::MediaState::for_tests(db.clone(), ids.clone());
    let content = ContentState::new(db.clone(), ids.clone(), bus.clone(), media.clone());
    let comments = CommentsState::new(db.clone(), ids, bus, media);
    Harness {
        feed: FeedState::new(content.clone(), Hot::default()),
        content,
        comments,
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

    async fn post(&self, author: &Principal, title: &str, anonymous: bool) -> i64 {
        let input = PostInput {
            title: title.into(),
            body: "正文".into(),
            category_id: None,
            tags: Vec::new(),
            is_anonymous: anonymous,
            external_videos: Vec::new(),
            editor_state: None,
            cover_ids: Vec::new(),
        };
        ik_content::service::create(&self.content, author, input, true)
            .await
            .unwrap()
            .id
            .get()
    }

    async fn like(&self, who: &Principal, post: i64) {
        ik_content::service::like(&self.content, who, post)
            .await
            .unwrap();
    }

    async fn comment(&self, who: &Principal, post: i64) -> i64 {
        ik_comments::service::create(
            &self.comments,
            who,
            post,
            CreateCommentRequest {
                body: "评论".into(),
                reply_to_id: None,
                is_anonymous: false,
                image_ids: Vec::new(),
            },
        )
        .await
        .unwrap()
        .id
        .get()
    }

    /// 测试里准备数据用的固定 SQL（字面量拼上数字 id），不含外部输入。
    async fn sql(&self, statement: &str) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement.to_owned()))
            .execute(&self.db)
            .await
            .unwrap();
    }
}

fn query() -> FeedQuery {
    FeedQuery {
        category: None,
        tag: None,
        cursor: None,
        limit: None,
    }
}

fn ids(cards: &[ik_content::dto::PostCard]) -> Vec<i64> {
    cards.iter().map(|c| c.id.get()).collect()
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn hot_board_counts_valid_activity_and_caps_named_authors(db: PgPool) {
    let h = harness(db);
    let author = h.user(1).await;
    let other_author = h.user(2).await;
    let fans = [h.user(3).await, h.user(4).await, h.user(5).await];
    let bot = h.user(6).await;
    h.sql("UPDATE users SET is_ai_agent = TRUE WHERE id = 6")
        .await;

    let a = h.post(&author, "A", false).await;
    let b = h.post(&author, "B", false).await;
    let anon = h.post(&author, "匿名", true).await;
    let d = h.post(&other_author, "D", false).await;
    let quiet = h.post(&other_author, "没人理", false).await;

    for fan in &fans {
        h.like(fan, a).await;
    }
    h.like(&fans[0], b).await;
    h.like(&fans[0], anon).await;
    h.like(&fans[1], anon).await;
    // 作者本人与 AI 的行为不计入。
    h.like(&other_author, d).await;
    h.like(&bot, d).await;
    h.comment(&fans[2], d).await;

    h.feed.refresh().await.unwrap();
    let board = h.feed.board();
    assert_eq!(board.full, vec![a, anon, b, d], "{board:?}");
    assert!(!board.full.contains(&quiet), "零分不上榜");
    // 同一实名作者只占一个展示位；匿名帖不参与限流，避免反推作者。
    assert_eq!(board.display, vec![a, anon, d]);

    // 隐藏的评论不算活动。
    h.sql(&format!(
        "UPDATE comments SET hidden_at = now() WHERE post_id = {d}"
    ))
    .await;
    h.feed.refresh().await.unwrap();
    assert!(!h.feed.board().full.contains(&d));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn home_feed_injects_pinned_and_hot_once(db: PgPool) {
    let h = harness(db);
    let author = h.user(1).await;
    let fan = h.user(2).await;
    let admin = h.user(9).await;
    h.sql("UPDATE users SET role = 'admin' WHERE id = 9").await;
    let admin = Principal {
        role: Role::Admin,
        ..admin
    };
    let other_author = h.user(3).await;
    let hot = h.post(&author, "热门", false).await;
    // 另一位作者：同一作者在展示位只占一篇。
    let pinned = h.post(&other_author, "置顶", false).await;
    let plain: Vec<i64> = {
        let mut v = Vec::new();
        for i in 0..3 {
            v.push(h.post(&author, &format!("普通{i}"), false).await);
        }
        v
    };
    h.like(&fan, hot).await;
    h.like(&fan, pinned).await;
    ik_content::service::pin(&h.content, &admin, pinned)
        .await
        .unwrap();
    h.feed.refresh().await.unwrap();

    let page = service::home(
        &h.feed,
        None,
        FeedQuery {
            limit: Some(2),
            ..query()
        },
    )
    .await
    .unwrap();
    assert_eq!(ids(&page.pinned), vec![pinned]);
    assert_eq!(ids(&page.hot), vec![hot], "已置顶的不再占热门位");
    assert_eq!(ids(&page.items), vec![plain[2], plain[1]]);
    let next = service::home(
        &h.feed,
        None,
        FeedQuery {
            cursor: page.next_cursor,
            limit: Some(10),
            ..query()
        },
    )
    .await
    .unwrap();
    assert!(next.pinned.is_empty() && next.hot.is_empty());
    assert_eq!(ids(&next.items), vec![plain[0]], "翻页时仍排除置顶与热门");

    // 最新流（/v1/posts）不注入热门。
    let latest = ik_content::service::feed(&h.content, None, query())
        .await
        .unwrap();
    assert!(ids(&latest.items).contains(&hot));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn hot_list_filters_before_paging(db: PgPool) {
    let h = harness(db);
    let viewer = h.user(1).await;
    let blocked_author = h.user(2).await;
    let author = h.user(3).await;
    let fans = [h.user(4).await, h.user(5).await];
    let mut posts = Vec::new();
    for i in 0..4 {
        let post = h.post(&author, &format!("帖{i}"), false).await;
        h.like(&fans[0], post).await;
        posts.push(post);
    }
    let from_blocked = h.post(&blocked_author, "被拉黑者", false).await;
    h.like(&fans[0], from_blocked).await;
    h.like(&fans[1], from_blocked).await;
    h.sql("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES (1, 2)")
        .await;
    h.feed.refresh().await.unwrap();
    assert_eq!(h.feed.board().full.len(), 5);

    let first = service::hot(
        &h.feed,
        Some(&viewer),
        HotQuery {
            cursor: None,
            limit: Some(3),
        },
    )
    .await
    .unwrap();
    assert_eq!(first.items.len(), 3);
    assert!(!ids(&first.items).contains(&from_blocked));
    let second = service::hot(
        &h.feed,
        Some(&viewer),
        HotQuery {
            cursor: first.next_cursor,
            limit: Some(3),
        },
    )
    .await
    .unwrap();
    assert_eq!(second.items.len(), 1, "过滤后一共 4 篇，不出现幽灵页");
    assert!(second.next_cursor.is_none());

    // 游客能看到全部 5 篇。
    let guest = service::hot(
        &h.feed,
        None,
        HotQuery {
            cursor: None,
            limit: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(guest.items.len(), 5);
    assert_eq!(guest.items[0].id.get(), from_blocked);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn following_feed_shows_only_named_posts_of_followees(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    let followed = h.user(2).await;
    let stranger = h.user(3).await;
    let named = h.post(&followed, "实名", false).await;
    h.post(&followed, "匿名", true).await;
    h.post(&stranger, "陌生人", false).await;

    let empty = service::following(&h.feed, &me, query()).await.unwrap();
    assert!(empty.items.is_empty());
    h.sql("INSERT INTO follows (follower_id, followee_id) VALUES (1, 2)")
        .await;
    let page = service::following(&h.feed, &me, query()).await.unwrap();
    assert_eq!(ids(&page.items), vec![named]);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unread_feed_and_read_marks(db: PgPool) {
    let h = harness(db);
    let me = h.user(1).await;
    let author = h.user(2).await;
    let first = h.post(&author, "一", false).await;
    let second = h.post(&author, "二", false).await;

    let marked = service::mark_read(
        &h.feed,
        &me,
        MarkReadRequest {
            post_ids: vec![format!(" {first} "), first.to_string(), String::new()],
        },
    )
    .await
    .unwrap();
    assert_eq!(marked.marked, 1);
    let again = service::mark_read(
        &h.feed,
        &me,
        MarkReadRequest {
            post_ids: vec![first.to_string()],
        },
    )
    .await
    .unwrap();
    assert_eq!(again.marked, 0, "读过的不重复计");

    let unread = service::unread(&h.feed, &me, query()).await.unwrap();
    assert_eq!(ids(&unread.items), vec![second]);

    let home = service::home(&h.feed, Some(&me), query()).await.unwrap();
    let read_flags: Vec<(i64, bool)> = home.items.iter().map(|c| (c.id.get(), c.is_read)).collect();
    assert_eq!(read_flags, vec![(second, false), (first, true)]);
    let guest = service::home(&h.feed, None, query()).await.unwrap();
    assert!(guest.items.iter().all(|c| !c.is_read));

    // 有不存在的帖子时整批拒绝、不写入。
    let err = service::mark_read(
        &h.feed,
        &me,
        MarkReadRequest {
            post_ids: vec![second.to_string(), "999".into()],
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "NOT_FOUND");
    assert_eq!(
        service::unread(&h.feed, &me, query())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let too_many: Vec<String> = (1..=101).map(|i| i.to_string()).collect();
    let err = service::mark_read(&h.feed, &me, MarkReadRequest { post_ids: too_many })
        .await
        .unwrap_err();
    assert_eq!(err.code(), "VALIDATION_FAILED");
}
