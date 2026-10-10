//! 帖子 service 层集成测试：真实 PostgreSQL，每个测试一个独立临时库。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ik_auth::Principal;
use ik_content::ContentState;
use ik_content::dto::{FeedQuery, PostInput, PostStatus};
use ik_content::service;
use ik_kernel::bus::LocalBus;
use ik_kernel::{Id, Role, Snowflake};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

struct Harness {
    state: ContentState,
    bus: Arc<LocalBus>,
    db: PgPool,
}

fn harness(db: PgPool) -> Harness {
    let bus = Arc::new(LocalBus::new(1024));
    let state = ContentState::new(
        db.clone(),
        Arc::new(Snowflake::new(1).unwrap()),
        bus.clone(),
    );
    Harness { state, bus, db }
}

impl Harness {
    async fn user(&self, id: i64, role: Role, level: i16, exam: bool) -> Principal {
        sqlx::query!(
            "INSERT INTO users (id, username, email, role, level, exam_passed_at)
             VALUES ($1, $2, $3, $4, $5, CASE WHEN $6 THEN now() END)",
            id,
            format!("user{id}"),
            format!("user{id}@example.com"),
            role as Role,
            level,
            exam,
        )
        .execute(&self.db)
        .await
        .unwrap();
        Principal {
            user_id: Id::new(id),
            session_id: Id::new(id),
            role,
            exam_passed: exam,
            is_ai_agent: false,
            level,
        }
    }

    async fn writer(&self, id: i64) -> Principal {
        self.user(id, Role::User, 1, true).await
    }

    async fn category(&self, slug: &str, admin_only: bool) -> i64 {
        service::create_category(
            &self.state,
            service::NewCategoryInput {
                slug: slug.into(),
                name: slug.into(),
                admin_only,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id
        .get()
    }
}

fn input(title: &str) -> PostInput {
    PostInput {
        title: title.into(),
        body: "正文".into(),
        category_id: None,
        tags: Vec::new(),
        is_anonymous: false,
        external_videos: Vec::new(),
        editor_state: None,
    }
}

async fn feed_ids(h: &Harness, query: FeedQuery) -> (Vec<i64>, Option<String>) {
    let page = service::feed(&h.state, None, query).await.unwrap();
    (
        page.items.iter().map(|c| c.id.get()).collect(),
        page.next_cursor,
    )
}

fn latest() -> FeedQuery {
    FeedQuery {
        category: None,
        tag: None,
        cursor: None,
        limit: None,
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn draft_is_private_until_published(db: PgPool) {
    let h = harness(db);
    let me = h.writer(1).await;
    let draft = service::create(&h.state, &me, input("草稿"), false)
        .await
        .unwrap();
    assert_eq!(draft.status, PostStatus::Draft);
    let id = draft.id.get();

    assert_eq!(
        service::detail(&h.state, None, id)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
    assert!(feed_ids(&h, latest()).await.0.is_empty());

    let published = service::publish(&h.state, &me, id, None, None)
        .await
        .unwrap();
    assert_eq!(published.status, PostStatus::Published);
    let detail = service::detail(&h.state, None, id).await.unwrap();
    assert_eq!(detail.title, "草稿");
    assert_eq!(detail.author.name, "user1");
    assert_eq!(feed_ids(&h, latest()).await.0, vec![id]);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn writes_require_exam_but_delete_does_not(db: PgPool) {
    let h = harness(db);
    let writer = h.writer(1).await;
    let id = service::create(&h.state, &writer, input("x"), false)
        .await
        .unwrap()
        .id
        .get();

    let newbie = h.user(2, Role::User, 0, false).await;
    assert_eq!(
        service::create(&h.state, &newbie, input("x"), false)
            .await
            .unwrap_err()
            .code(),
        "EXAM_REQUIRED"
    );
    // 考试状态被收回后，仍可删除自己的帖子（D-015）。
    let revoked = Principal {
        exam_passed: false,
        ..writer
    };
    service::delete(&h.state, &revoked, id).await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn editing_published_post_stages_changes_without_bumping(db: PgPool) {
    let h = harness(db);
    let me = h.writer(1).await;
    let id = service::create(&h.state, &me, input("旧标题"), true)
        .await
        .unwrap()
        .id
        .get();
    let before = service::detail(&h.state, None, id).await.unwrap();

    let staged = service::save_draft(&h.state, &me, id, input("新标题"), None)
        .await
        .unwrap();
    assert!(staged.has_pending_changes);
    assert_eq!(staged.title, "新标题");
    assert_eq!(
        service::detail(&h.state, None, id).await.unwrap().title,
        "旧标题"
    );

    service::publish(&h.state, &me, id, None, None)
        .await
        .unwrap();
    let after = service::detail(&h.state, None, id).await.unwrap();
    assert_eq!(after.title, "新标题");
    assert_eq!(after.bumped_at, before.bumped_at, "编辑重发不顶帖");
    assert_eq!(after.first_published_at, before.first_published_at);
    assert!(after.edited_at.is_some());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_revision_is_rejected(db: PgPool) {
    let h = harness(db);
    let me = h.writer(1).await;
    let view = service::create(&h.state, &me, input("a"), false)
        .await
        .unwrap();
    let id = view.id.get();
    service::save_draft(&h.state, &me, id, input("b"), Some(view.revision))
        .await
        .unwrap();
    let err = service::save_draft(&h.state, &me, id, input("c"), Some(view.revision))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "REVISION_CONFLICT");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_posts_with_same_new_tag_share_one_tag(db: PgPool) {
    let h = harness(db.clone());
    let me = h.writer(1).await;
    let mut tasks = Vec::new();
    for i in 0..8 {
        let state = h.state.clone();
        let me = me.clone();
        tasks.push(tokio::spawn(async move {
            let mut post = input(&format!("帖子{i}"));
            post.tags = vec![
                "绝区零".into(),
                if i % 2 == 0 {
                    "ＺＺＺ".into()
                } else {
                    "zzz".into()
                },
            ];
            service::create(&state, &me, post, true).await
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap().unwrap().tags.len(), 2);
    }
    let count = sqlx::query_scalar!("SELECT count(*) FROM tags")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, Some(2));
    let tagged = sqlx::query_scalar!("SELECT count(*) FROM post_tags")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(tagged, Some(16));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_publish_announces_first_publish_once(db: PgPool) {
    let h = harness(db);
    let me = h.writer(1).await;
    let id = service::create(&h.state, &me, input("x"), false)
        .await
        .unwrap()
        .id
        .get();
    let mut rx = h.bus.subscribe();

    let mut tasks = Vec::new();
    for _ in 0..6 {
        let state = h.state.clone();
        let me = me.clone();
        tasks.push(tokio::spawn(async move {
            service::publish(&state, &me, id, None, None).await
        }));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    let mut first = 0;
    while let Ok(event) = rx.try_recv() {
        if event.kind == "post.published" && event.data["firstPublish"] == true {
            first += 1;
        }
    }
    assert_eq!(first, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn anonymous_posts_hide_author_and_lock_after_publish(db: PgPool) {
    let h = harness(db);
    let me = h.writer(1).await;
    let mut post = input("匿名");
    post.is_anonymous = true;
    let id = service::create(&h.state, &me, post.clone(), true)
        .await
        .unwrap()
        .id
        .get();

    let seen_by_other = service::detail(&h.state, None, id).await.unwrap();
    let seen_by_owner = service::detail(&h.state, Some(&me), id).await.unwrap();
    assert!(seen_by_other.author.id.is_none());
    assert!(seen_by_other.author.name.starts_with("绳网用户#"));
    assert_eq!(
        seen_by_other.author.name, seen_by_owner.author.name,
        "对作者本人也匿名"
    );
    assert!(seen_by_owner.is_owner);

    assert_eq!(
        service::save_draft(&h.state, &me, id, post, None)
            .await
            .unwrap_err()
            .code(),
        "ANONYMOUS_POST_LOCKED"
    );
    assert_eq!(
        service::delete(&h.state, &me, id).await.unwrap_err().code(),
        "ANONYMOUS_POST_LOCKED"
    );
    let mine = service::my_posts(
        &h.state,
        &me,
        ik_content::dto::MyPostsQuery {
            state: Default::default(),
            cursor: None,
            limit: None,
        },
    )
    .await
    .unwrap();
    assert!(mine.items.is_empty(), "已发布的匿名帖不出现在我的帖子里");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn anonymity_toggle_in_draft_keeps_the_same_name(db: PgPool) {
    let h = harness(db);
    let me = h.writer(1).await;
    let mut post = input("x");
    post.is_anonymous = true;
    let id = service::create(&h.state, &me, post.clone(), false)
        .await
        .unwrap()
        .id
        .get();
    post.is_anonymous = false;
    service::save_draft(&h.state, &me, id, post.clone(), None)
        .await
        .unwrap();
    post.is_anonymous = true;
    service::save_draft(&h.state, &me, id, post, None)
        .await
        .unwrap();
    service::publish(&h.state, &me, id, None, None)
        .await
        .unwrap();

    let seeds = sqlx::query_scalar!("SELECT anonymous_seed FROM posts WHERE id = $1", id)
        .fetch_one(&h.db)
        .await
        .unwrap();
    assert!(seeds.is_some());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unpublish_keeps_or_discards_pending_edits(db: PgPool) {
    let h = harness(db);
    let me = h.writer(1).await;
    let a = service::create(&h.state, &me, input("A"), true)
        .await
        .unwrap()
        .id
        .get();
    let b = service::create(&h.state, &me, input("B"), true)
        .await
        .unwrap()
        .id
        .get();
    service::save_draft(&h.state, &me, a, input("A2"), None)
        .await
        .unwrap();
    service::save_draft(&h.state, &me, b, input("B2"), None)
        .await
        .unwrap();

    let kept = service::unpublish(&h.state, &me, a, false).await.unwrap();
    assert_eq!(
        (kept.status, kept.title.as_str(), kept.has_pending_changes),
        (PostStatus::Draft, "A2", false)
    );
    let dropped = service::unpublish(&h.state, &me, b, true).await.unwrap();
    assert_eq!(
        (dropped.status, dropped.title.as_str()),
        (PostStatus::Draft, "B")
    );

    assert_eq!(
        service::unpublish(&h.state, &me, a, false)
            .await
            .unwrap_err()
            .code(),
        "NOT_PUBLISHED"
    );
    assert!(feed_ids(&h, latest()).await.0.is_empty());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn others_cannot_touch_my_post(db: PgPool) {
    let h = harness(db);
    let me = h.writer(1).await;
    let other = h.writer(2).await;
    let id = service::create(&h.state, &me, input("x"), false)
        .await
        .unwrap()
        .id
        .get();
    for code in [
        service::save_draft(&h.state, &other, id, input("y"), None)
            .await
            .unwrap_err()
            .code(),
        service::publish(&h.state, &other, id, None, None)
            .await
            .unwrap_err()
            .code(),
        service::delete(&h.state, &other, id)
            .await
            .unwrap_err()
            .code(),
        service::edit_view(&h.state, &other, id)
            .await
            .unwrap_err()
            .code(),
    ] {
        assert_eq!(code, "FORBIDDEN");
    }
    assert_eq!(
        service::delete(&h.state, &me, 999)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn body_limit_follows_level(db: PgPool) {
    let h = harness(db);
    let lv1 = h.writer(1).await;
    let lv7 = h.user(2, Role::User, 7, true).await;
    let mut post = input("长文");
    post.body = "字".repeat(3001);
    let err = service::create(&h.state, &lv1, post.clone(), false)
        .await
        .unwrap_err();
    assert_eq!(err.field_errors()[0].field, "body");
    service::create(&h.state, &lv7, post, false).await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn blocked_tag_rejects_whole_post(db: PgPool) {
    let h = harness(db.clone());
    let me = h.writer(1).await;
    let mut post = input("x");
    post.tags = vec!["正常".into()];
    service::create(&h.state, &me, post, false).await.unwrap();
    sqlx::query!("UPDATE tags SET is_blocked = TRUE")
        .execute(&db)
        .await
        .unwrap();

    let mut post = input("y");
    post.tags = vec!["新标签".into(), "正常".into()];
    let err = service::create(&h.state, &me, post, false)
        .await
        .unwrap_err();
    assert_eq!(err.field_errors()[0].code, "BLOCKED");
    let tags = sqlx::query_scalar!("SELECT count(*) FROM tags")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(tags, Some(1), "被拒绝时不应部分创建新标签");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn admin_only_category(db: PgPool) {
    let h = harness(db);
    let notice = h.category("notice", true).await;
    let me = h.writer(1).await;
    let admin = h.user(2, Role::Admin, 0, false).await;
    let mut post = input("公告");
    post.category_id = Some(Id::new(notice));
    assert_eq!(
        service::create(&h.state, &me, post.clone(), true)
            .await
            .unwrap_err()
            .code(),
        "CATEGORY_ADMIN_ONLY"
    );
    service::create(&h.state, &admin, post, true).await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn feed_pagination_is_stable_on_equal_timestamps(db: PgPool) {
    let h = harness(db.clone());
    let me = h.writer(1).await;
    let mut all = Vec::new();
    for i in 0..7 {
        all.push(
            service::create(&h.state, &me, input(&format!("p{i}")), true)
                .await
                .unwrap()
                .id
                .get(),
        );
    }
    sqlx::query!("UPDATE posts SET bumped_at = '2026-01-01T00:00:00Z'")
        .execute(&db)
        .await
        .unwrap();

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let (ids, next) = feed_ids(
            &h,
            FeedQuery {
                category: None,
                tag: None,
                cursor,
                limit: Some(2),
            },
        )
        .await;
        seen.extend(ids);
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    all.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(seen, all, "同一时间戳按 id 倒序，不重不漏");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn pinned_posts_move_to_pinned_section(db: PgPool) {
    let h = harness(db);
    let me = h.writer(1).await;
    let admin = h.user(2, Role::Admin, 0, false).await;
    let a = service::create(&h.state, &me, input("A"), true)
        .await
        .unwrap()
        .id
        .get();
    let b = service::create(&h.state, &me, input("B"), true)
        .await
        .unwrap()
        .id
        .get();
    assert_eq!(
        service::pin(&h.state, &me, a).await.unwrap_err().code(),
        "FORBIDDEN"
    );
    service::pin(&h.state, &admin, a).await.unwrap();
    service::pin(&h.state, &admin, a).await.unwrap();

    let page = service::feed(&h.state, None, latest()).await.unwrap();
    assert_eq!(
        page.pinned.iter().map(|c| c.id.get()).collect::<Vec<_>>(),
        vec![a]
    );
    assert_eq!(
        page.items.iter().map(|c| c.id.get()).collect::<Vec<_>>(),
        vec![b]
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn views_respect_cooldown_per_viewer(db: PgPool) {
    let h = harness(db);
    let me = h.writer(1).await;
    let id = service::create(&h.state, &me, input("x"), true)
        .await
        .unwrap()
        .id
        .get();

    assert!(
        service::record_view(&h.state, "ip:1".into(), id)
            .await
            .unwrap()
            .counted
    );
    assert!(
        !service::record_view(&h.state, "ip:1".into(), id)
            .await
            .unwrap()
            .counted
    );
    let mut tasks = Vec::new();
    for i in 2..12 {
        let state = h.state.clone();
        tasks.push(tokio::spawn(async move {
            service::record_view(&state, format!("ip:{i}"), id).await
        }));
    }
    for task in tasks {
        assert!(task.await.unwrap().unwrap().counted);
    }
    assert_eq!(
        service::detail(&h.state, None, id)
            .await
            .unwrap()
            .view_count,
        11
    );
    assert_eq!(
        service::record_view(&h.state, "ip:x".into(), 424242)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn delete_cascades(db: PgPool) {
    let h = harness(db.clone());
    let me = h.writer(1).await;
    let mut post = input("x");
    post.tags = vec!["t".into()];
    let id = service::create(&h.state, &me, post.clone(), true)
        .await
        .unwrap()
        .id
        .get();
    service::save_draft(&h.state, &me, id, post, None)
        .await
        .unwrap();
    service::delete(&h.state, &me, id).await.unwrap();
    let left = sqlx::query_scalar!(
        "SELECT (SELECT count(*) FROM post_tags) + (SELECT count(*) FROM post_drafts)"
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(left, Some(0));
}
