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
    let ids = Arc::new(Snowflake::new(1).unwrap());
    let media = ik_media::MediaState::for_tests(db.clone(), ids.clone());
    let state = ContentState::new(db.clone(), ids, bus.clone(), media);
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
        cover_ids: Vec::new(),
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

/// 实名发布过的帖子下线后也不能改成匿名再发：粉丝的新帖通知、看过的人都能把匿名名对上真人。
#[sqlx::test(migrator = "MIGRATOR")]
async fn anonymity_stays_locked_after_unpublish(db: PgPool) {
    let h = harness(db);
    let me = h.writer(1).await;
    let mut post = input("实名");
    let id = service::create(&h.state, &me, post.clone(), true)
        .await
        .unwrap()
        .id
        .get();
    service::unpublish(&h.state, &me, id, false).await.unwrap();

    post.is_anonymous = true;
    let err = service::save_draft(&h.state, &me, id, post.clone(), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "VALIDATION_FAILED");
    let err = service::publish(&h.state, &me, id, Some(post), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "VALIDATION_FAILED");

    let anonymous = sqlx::query_scalar!("SELECT is_anonymous FROM posts WHERE id = $1", id)
        .fetch_one(&h.db)
        .await
        .unwrap();
    assert!(!anonymous);
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

// ---------- 投币与发帖经验 ----------

async fn balance(db: &PgPool, id: i64) -> (i32, i32) {
    sqlx::query!(
        "SELECT denny_balance, denny_given FROM user_economy WHERE user_id = $1",
        id
    )
    .fetch_optional(db)
    .await
    .unwrap()
    .map_or((30, 0), |r| (r.denny_balance, r.denny_given))
}

async fn exp_of(db: &PgPool, id: i64) -> i64 {
    sqlx::query_scalar!("SELECT exp FROM user_economy WHERE user_id = $1", id)
        .fetch_optional(db)
        .await
        .unwrap()
        .unwrap_or(0)
}

fn gift() -> ik_content::dto::GiftRequest {
    ik_content::dto::GiftRequest {
        message: "好帖".into(),
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn gift_moves_one_denny_and_counts_once(db: PgPool) {
    let h = harness(db);
    let author = h.writer(1).await;
    let fan = h.writer(2).await;
    let post = service::create(&h.state, &author, input("帖子"), true)
        .await
        .unwrap()
        .id
        .get();

    let result = service::gift_denny(&h.state, &fan, post, gift())
        .await
        .unwrap();
    assert_eq!((result.balance, result.denny_count), (29, 1));
    assert_eq!(balance(&h.db, 2).await, (29, 1));
    assert_eq!(balance(&h.db, 1).await, (31, 0));
    // 作者：发帖 4 + 被投币 1（被动桶）。
    assert_eq!(exp_of(&h.db, 1).await, 5);

    let err = service::gift_denny(&h.state, &fan, post, gift())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "DENNY_ALREADY_GIVEN");
    assert_eq!(balance(&h.db, 2).await, (29, 1), "重复投币不扣费");

    let detail = service::detail(&h.state, Some(&fan), post).await.unwrap();
    assert!(detail.denny_given);
    assert_eq!(detail.denny_count, 1);
    let other = service::detail(&h.state, Some(&author), post)
        .await
        .unwrap();
    assert!(!other.denny_given);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn gift_rejects_self_anonymous_hidden_and_broke(db: PgPool) {
    let h = harness(db);
    let author = h.writer(1).await;
    let fan = h.writer(2).await;
    let post = service::create(&h.state, &author, input("帖子"), true)
        .await
        .unwrap()
        .id
        .get();
    let err = service::gift_denny(&h.state, &author, post, gift())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "DENNY_SELF_GIVE");

    let mut anon = input("匿名");
    anon.is_anonymous = true;
    let anon_post = service::create(&h.state, &author, anon, true)
        .await
        .unwrap()
        .id
        .get();
    let err = service::gift_denny(&h.state, &fan, anon_post, gift())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "DENNY_ANONYMOUS_POST");

    let hidden = service::create(&h.state, &author, input("隐藏"), true)
        .await
        .unwrap()
        .id
        .get();
    sqlx::query!("UPDATE posts SET hidden_at = now() WHERE id = $1", hidden)
        .execute(&h.db)
        .await
        .unwrap();
    let err = service::gift_denny(&h.state, &fan, hidden, gift())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "NOT_FOUND");

    sqlx::query!(
        "INSERT INTO user_economy (user_id, denny_balance) VALUES (2, 0)
                  ON CONFLICT (user_id) DO UPDATE SET denny_balance = 0"
    )
    .execute(&h.db)
    .await
    .unwrap();
    let err = service::gift_denny(&h.state, &fan, post, gift())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "INSUFFICIENT_DENNY");
    let count: i32 = sqlx::query_scalar!("SELECT denny_count FROM posts WHERE id = $1", post)
        .fetch_one(&h.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_gifts_from_same_user_count_once(db: PgPool) {
    let h = harness(db);
    let author = h.writer(1).await;
    let fan = h.writer(2).await;
    let post = service::create(&h.state, &author, input("帖子"), true)
        .await
        .unwrap()
        .id
        .get();
    let tasks: Vec<_> = (0..6)
        .map(|_| {
            let state = h.state.clone();
            let fan = fan.clone();
            tokio::spawn(async move { service::gift_denny(&state, &fan, post, gift()).await })
        })
        .collect();
    let mut ok = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => ok += 1,
            Err(e) => assert_eq!(e.code(), "DENNY_ALREADY_GIVEN"),
        }
    }
    assert_eq!(ok, 1);
    assert_eq!(balance(&h.db, 2).await, (29, 1));
    assert_eq!(balance(&h.db, 1).await, (31, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn mutual_gifts_do_not_deadlock(db: PgPool) {
    let h = harness(db);
    let a = h.writer(1).await;
    let b = h.writer(2).await;
    let post_a = service::create(&h.state, &a, input("A"), true)
        .await
        .unwrap()
        .id
        .get();
    let post_b = service::create(&h.state, &b, input("B"), true)
        .await
        .unwrap()
        .id
        .get();
    let (s1, s2) = (h.state.clone(), h.state.clone());
    let (b2, a2) = (b.clone(), a.clone());
    let t1 = tokio::spawn(async move { service::gift_denny(&s1, &b2, post_a, gift()).await });
    let t2 = tokio::spawn(async move { service::gift_denny(&s2, &a2, post_b, gift()).await });
    t1.await.unwrap().unwrap();
    t2.await.unwrap().unwrap();
    assert_eq!(balance(&h.db, 1).await, (30, 1));
    assert_eq!(balance(&h.db, 2).await, (30, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn first_publish_grants_exp_once_per_day(db: PgPool) {
    let h = harness(db);
    let author = h.writer(1).await;
    let first = service::create(&h.state, &author, input("一"), true)
        .await
        .unwrap()
        .id
        .get();
    assert_eq!(exp_of(&h.db, 1).await, 4);
    // 同一天第二篇不再给；下线再发布也不算首次发布。
    service::create(&h.state, &author, input("二"), true)
        .await
        .unwrap();
    service::unpublish(&h.state, &author, first, false)
        .await
        .unwrap();
    service::publish(&h.state, &author, first, None, None)
        .await
        .unwrap();
    assert_eq!(exp_of(&h.db, 1).await, 4);
    // 草稿不给经验。
    let newbie = h.writer(2).await;
    service::create(&h.state, &newbie, input("草稿"), false)
        .await
        .unwrap();
    assert_eq!(exp_of(&h.db, 2).await, 0);
}

// ---------- 拉黑 ----------

async fn block(db: &PgPool, blocker: i64, blocked: i64) {
    sqlx::query!(
        "INSERT INTO user_blocks (blocker_id, blocked_id) VALUES ($1, $2)",
        blocker,
        blocked,
    )
    .execute(db)
    .await
    .unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn blocks_hide_non_anonymous_posts_both_ways(db: PgPool) {
    let h = harness(db);
    let author = h.writer(1).await;
    let reader = h.writer(2).await;
    let bystander = h.writer(3).await;
    let named = service::create(&h.state, &author, input("实名"), true)
        .await
        .unwrap()
        .id
        .get();
    let mut anon = input("匿名");
    anon.is_anonymous = true;
    let anonymous = service::create(&h.state, &author, anon, true)
        .await
        .unwrap()
        .id
        .get();
    let pinned = service::create(&h.state, &author, input("置顶"), true)
        .await
        .unwrap()
        .id
        .get();
    let admin = h.user(9, Role::Admin, 7, true).await;
    service::pin(&h.state, &admin, pinned).await.unwrap();
    // 作者拉黑了读者：对读者来说，方向无关。
    block(&h.db, 1, 2).await;

    let page = service::feed(&h.state, Some(&reader), latest())
        .await
        .unwrap();
    let ids: Vec<i64> = page.items.iter().map(|c| c.id.get()).collect();
    assert_eq!(ids, vec![anonymous], "匿名帖不按拉黑过滤");
    assert!(page.pinned.is_empty());

    let err = service::detail(&h.state, Some(&reader), named)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "USER_BLOCKED");
    service::detail(&h.state, Some(&reader), anonymous)
        .await
        .unwrap();

    // 旁观者和游客不受影响。
    let page = service::feed(&h.state, Some(&bystander), latest())
        .await
        .unwrap();
    assert_eq!(page.items.len() + page.pinned.len(), 3);
    service::detail(&h.state, None, named).await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn gift_is_rejected_between_blocked_users(db: PgPool) {
    let h = harness(db);
    let author = h.writer(1).await;
    let fan = h.writer(2).await;
    let post = service::create(&h.state, &author, input("帖子"), true)
        .await
        .unwrap()
        .id
        .get();
    block(&h.db, 1, 2).await;
    let err = service::gift_denny(&h.state, &fan, post, gift())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "USER_BLOCKED");
    assert_eq!(balance(&h.db, 2).await, (30, 0));
}

// ---------- 点赞、收藏、三连 ----------

async fn publish(h: &Harness, author: &Principal, title: &str, anonymous: bool) -> i64 {
    let mut post = input(title);
    post.is_anonymous = anonymous;
    service::create(&h.state, author, post, true)
        .await
        .unwrap()
        .id
        .get()
}

fn saved_page() -> ik_content::dto::SavedQuery {
    ik_content::dto::SavedQuery {
        cursor: None,
        limit: None,
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn like_and_unlike_are_idempotent(db: PgPool) {
    let h = harness(db);
    let author = h.writer(1).await;
    let fan = h.writer(2).await;
    let post = publish(&h, &author, "帖子", false).await;
    let other = publish(&h, &author, "另一篇", false).await;

    let liked = service::like(&h.state, &fan, post).await.unwrap();
    assert!(liked.liked);
    assert_eq!(liked.like_count, 1);
    assert_eq!(
        service::like(&h.state, &fan, post)
            .await
            .unwrap()
            .like_count,
        1
    );
    let detail = service::detail(&h.state, Some(&fan), post).await.unwrap();
    assert!(detail.liked && !detail.favorited);
    assert_eq!(detail.like_count, 1);
    assert!(
        !service::detail(&h.state, Some(&author), post)
            .await
            .unwrap()
            .liked
    );

    // 点赞者每个业务日第一次点赞 +1 经验；被点赞者没有经验。
    service::like(&h.state, &fan, other).await.unwrap();
    assert_eq!(exp_of(&h.db, 2).await, 1);
    assert_eq!(exp_of(&h.db, 1).await, 4, "作者只有发帖经验");
    // 赞自己的帖子：计数照常，不给经验。
    assert_eq!(
        service::like(&h.state, &author, post)
            .await
            .unwrap()
            .like_count,
        2
    );
    assert_eq!(exp_of(&h.db, 1).await, 4);

    let off = service::unlike(&h.state, &fan, post).await.unwrap();
    assert!(!off.liked);
    assert_eq!(off.like_count, 1);
    assert_eq!(
        service::unlike(&h.state, &fan, post)
            .await
            .unwrap()
            .like_count,
        1
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn like_rules(db: PgPool) {
    let h = harness(db);
    let author = h.writer(1).await;
    let fan = h.writer(2).await;
    let newbie = h.user(3, Role::User, 0, false).await;
    let post = publish(&h, &author, "帖子", false).await;
    let anonymous = publish(&h, &author, "匿名", true).await;

    assert_eq!(
        service::like(&h.state, &newbie, post)
            .await
            .unwrap_err()
            .code(),
        "EXAM_REQUIRED"
    );
    service::like(&h.state, &fan, post).await.unwrap();
    sqlx::query!("UPDATE posts SET hidden_at = now() WHERE id = $1", post)
        .execute(&h.db)
        .await
        .unwrap();
    // 不可见的帖子不能再赞，但能取消（LIKE-018）。
    assert_eq!(
        service::favorite(&h.state, &fan, post)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
    assert_eq!(
        service::unlike(&h.state, &fan, post)
            .await
            .unwrap()
            .like_count,
        0
    );
    assert_eq!(
        service::unlike(&h.state, &fan, 999)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );

    // 拉黑拦截非匿名帖，不拦匿名帖。
    block(&h.db, 1, 2).await;
    let other = publish(&h, &author, "另一篇", false).await;
    assert_eq!(
        service::like(&h.state, &fan, other)
            .await
            .unwrap_err()
            .code(),
        "USER_BLOCKED"
    );
    assert_eq!(
        service::favorite(&h.state, &fan, other)
            .await
            .unwrap_err()
            .code(),
        "USER_BLOCKED"
    );
    service::like(&h.state, &fan, anonymous).await.unwrap();
    service::favorite(&h.state, &fan, anonymous).await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_likes_from_same_user_count_once(db: PgPool) {
    let h = harness(db);
    let author = h.writer(1).await;
    let fan = h.writer(2).await;
    let post = publish(&h, &author, "帖子", false).await;
    let tasks: Vec<_> = (0..6)
        .map(|_| {
            let state = h.state.clone();
            let fan = fan.clone();
            tokio::spawn(async move { service::like(&state, &fan, post).await })
        })
        .collect();
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    let detail = service::detail(&h.state, None, post).await.unwrap();
    assert_eq!(detail.like_count, 1);
    assert_eq!(exp_of(&h.db, 2).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn favorites_and_saved_lists(db: PgPool) {
    let h = harness(db);
    let author = h.writer(1).await;
    let fan = h.writer(2).await;
    let a = publish(&h, &author, "A", false).await;
    let b = publish(&h, &author, "B", false).await;
    let c = publish(&h, &author, "C", false).await;

    let fav = service::favorite(&h.state, &fan, a).await.unwrap();
    assert!(fav.favorited);
    assert_eq!(fav.favorite_count, 1);
    assert_eq!(
        service::favorite(&h.state, &fan, a)
            .await
            .unwrap()
            .favorite_count,
        1
    );
    service::favorite(&h.state, &fan, b).await.unwrap();
    service::favorite(&h.state, &fan, c).await.unwrap();
    sqlx::query!("UPDATE posts SET hidden_at = now() WHERE id = $1", b)
        .execute(&h.db)
        .await
        .unwrap();

    let page = service::my_favorites(
        &h.state,
        &fan,
        ik_content::dto::SavedQuery {
            cursor: None,
            limit: Some(1),
        },
    )
    .await
    .unwrap();
    assert_eq!(page.items[0].post.id.get(), c);
    let page = service::my_favorites(
        &h.state,
        &fan,
        ik_content::dto::SavedQuery {
            cursor: page.next_cursor,
            limit: Some(1),
        },
    )
    .await
    .unwrap();
    // 被隐藏的 b 不出现，也不会产生空页。
    assert_eq!(page.items[0].post.id.get(), a);
    assert!(page.next_cursor.is_none());

    let off = service::unfavorite(&h.state, &fan, a).await.unwrap();
    assert!(!off.favorited);
    assert_eq!(off.favorite_count, 0);

    service::like(&h.state, &fan, c).await.unwrap();
    let likes = service::my_likes(&h.state, &fan, saved_page())
        .await
        .unwrap();
    assert_eq!(likes.items.len(), 1);
    assert_eq!(likes.items[0].post.like_count, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn triple_likes_favorites_and_gifts_once(db: PgPool) {
    let h = harness(db);
    let author = h.writer(1).await;
    let fan = h.writer(2).await;
    let post = publish(&h, &author, "帖子", false).await;

    let first = service::triple(&h.state, &fan, post).await.unwrap();
    assert_eq!((first.like_count, first.favorite_count), (1, 1));
    assert!(first.denny_given);
    assert_eq!(first.denny_error, None);
    assert_eq!((first.denny_count, first.balance), (1, Some(29)));

    // 再来一次：赞、藏保持，投币软失败，不扣费。
    let again = service::triple(&h.state, &fan, post).await.unwrap();
    assert_eq!((again.like_count, again.favorite_count), (1, 1));
    assert!(!again.denny_given);
    assert_eq!(again.denny_error.as_deref(), Some("DENNY_ALREADY_GIVEN"));
    assert_eq!((again.denny_count, again.balance), (1, None));
    assert_eq!(balance(&h.db, 2).await, (29, 1));

    // 自己的帖子：赞、藏照常，投币原因是 DENNY_SELF_GIVE。
    let own = service::triple(&h.state, &author, post).await.unwrap();
    assert_eq!((own.like_count, own.favorite_count), (2, 2));
    assert_eq!(own.denny_error.as_deref(), Some("DENNY_SELF_GIVE"));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn blocked_triple_writes_nothing(db: PgPool) {
    let h = harness(db);
    let author = h.writer(1).await;
    let fan = h.writer(2).await;
    let post = publish(&h, &author, "帖子", false).await;
    block(&h.db, 2, 1).await;
    let err = service::triple(&h.state, &fan, post).await.unwrap_err();
    assert_eq!(err.code(), "USER_BLOCKED");
    let detail = service::detail(&h.state, None, post).await.unwrap();
    assert_eq!(
        (detail.like_count, detail.favorite_count, detail.denny_count),
        (0, 0, 0)
    );
}

// ---------- 通知 ----------

async fn notifications(db: &PgPool, recipient: i64) -> Vec<String> {
    sqlx::query_scalar!(
        "SELECT kind FROM notifications WHERE recipient_id = $1 ORDER BY created_at, id",
        recipient
    )
    .fetch_all(db)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn reactions_notify_the_author_and_unlike_retracts(db: PgPool) {
    let h = harness(db);
    let author = h.writer(1).await;
    let fan = h.writer(2).await;
    let post = publish(&h, &author, "帖子", false).await;
    let anonymous = publish(&h, &author, "匿名", true).await;

    service::like(&h.state, &fan, post).await.unwrap();
    service::like(&h.state, &fan, post).await.unwrap();
    service::favorite(&h.state, &fan, post).await.unwrap();
    service::gift_denny(&h.state, &fan, post, gift())
        .await
        .unwrap();
    assert_eq!(
        notifications(&h.db, 1).await,
        vec!["like", "favorite", "denny"]
    );

    service::unlike(&h.state, &fan, post).await.unwrap();
    service::unfavorite(&h.state, &fan, post).await.unwrap();
    assert_eq!(
        notifications(&h.db, 1).await,
        vec!["denny"],
        "D-011：取消即撤回"
    );

    // 匿名帖、自己的帖子不通知。
    service::triple(&h.state, &fan, anonymous).await.unwrap();
    service::like(&h.state, &author, post).await.unwrap();
    assert_eq!(notifications(&h.db, 1).await, vec!["denny"]);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn first_publish_notifies_followers_once(db: PgPool) {
    let h = harness(db);
    h.state.jobs.migrate().await.unwrap();
    let author = h.writer(1).await;
    h.writer(2).await;
    sqlx::query!("INSERT INTO follows (follower_id, followee_id) VALUES (2, 1)")
        .execute(&h.db)
        .await
        .unwrap();

    let draft = service::create(&h.state, &author, input("草稿"), false)
        .await
        .unwrap()
        .id
        .get();
    assert!(notifications(&h.db, 2).await.is_empty(), "草稿不扇出");
    service::publish(&h.state, &author, draft, None, None)
        .await
        .unwrap();
    h.run_jobs().await;
    assert_eq!(notifications(&h.db, 2).await, vec!["system"]);
    // 下线后重新发布不再通知。
    service::unpublish(&h.state, &author, draft, false)
        .await
        .unwrap();
    service::publish(&h.state, &author, draft, None, None)
        .await
        .unwrap();
    publish(&h, &author, "匿名", true).await;
    publish(&h, &author, "直接发布", false).await;
    h.run_jobs().await;
    assert_eq!(notifications(&h.db, 2).await, vec!["system", "system"]);
}

// ---------- 封面与作者头像 ----------

impl Harness {
    /// 写一个媒体文件，`owner` 为上传者。
    async fn media(&self, id: i64, owner: i64) {
        sqlx::query!(
            "INSERT INTO media_files (id, object_key, mime, size_bytes, sha256, receipt)
             VALUES ($1, $2, 'image/png', 100, $3, $4)",
            id,
            format!("2026/10/{id}.png"),
            "a".repeat(64),
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
}

fn with_covers(title: &str, covers: &[i64]) -> PostInput {
    PostInput {
        cover_ids: covers.iter().map(|id| Id::new(*id)).collect(),
        ..input(title)
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn covers_are_owned_ordered_and_capped_by_level(db: PgPool) {
    let h = harness(db);
    let me = h.writer(1).await;
    h.writer(2).await;
    for id in 101..=111 {
        h.media(id, 1).await;
    }
    h.media(200, 2).await;

    // 别人的图不能用作封面（D-012 方案 A）。
    let err = service::create(&h.state, &me, with_covers("t", &[101, 200]), false)
        .await
        .unwrap_err();
    assert_eq!(err.field_errors()[0].field, "coverIds");
    assert_eq!(err.field_errors()[0].code, "NOT_FOUND");

    // Lv1 最多 9 张：按原始条数拦截（重复的也算），去重后保序保存。
    let ten: Vec<i64> = (101..=110).collect();
    let err = service::create(&h.state, &me, with_covers("t", &ten), false)
        .await
        .unwrap_err();
    assert_eq!(err.field_errors()[0].code, "TOO_MANY");
    let post = service::create(
        &h.state,
        &me,
        with_covers("封面帖", &[103, 101, 103, 102]),
        true,
    )
    .await
    .unwrap();
    let ids: Vec<i64> = post.covers.iter().map(|m| m.id.get()).collect();
    assert_eq!(ids, [103, 101, 102]);
    assert_eq!(post.covers[0].url, "https://cdn.test/2026/10/103.png");

    // 卡片只带第一张；详情带全部。
    let feed = service::feed(
        &h.state,
        None,
        FeedQuery {
            category: None,
            tag: None,
            cursor: None,
            limit: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(feed.items[0].cover.as_ref().unwrap().id.get(), 103);
    let detail = service::detail(&h.state, None, post.id.get())
        .await
        .unwrap();
    assert_eq!(detail.covers.len(), 3);

    // 作者之后把图从图库移除：已经用上的封面照样可以保留（编辑时不再校验旧的）。
    sqlx::query!("DELETE FROM media_uploaders WHERE media_id = 101")
        .execute(&h.db)
        .await
        .unwrap();
    service::save_draft(
        &h.state,
        &me,
        post.id.get(),
        with_covers("封面帖", &[101, 104]),
        None,
    )
    .await
    .unwrap();
    let err = service::save_draft(
        &h.state,
        &me,
        post.id.get(),
        with_covers("封面帖", &[200]),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(err.field_errors()[0].code, "NOT_FOUND");

    // 正文插图同样按等级封顶（POST-014）。
    let body = "![a](https://x/a.png)".repeat(10);
    let err = service::create(
        &h.state,
        &me,
        PostInput {
            body,
            ..input("插图")
        },
        false,
    )
    .await
    .unwrap_err();
    assert_eq!(err.field_errors()[0].code, "TOO_MANY_IMAGES");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authors_show_avatars_except_when_anonymous(db: PgPool) {
    let h = harness(db);
    let me = h.writer(1).await;
    h.media(300, 1).await;
    sqlx::query!("UPDATE users SET avatar_media_id = 300 WHERE id = 1")
        .execute(&h.db)
        .await
        .unwrap();
    let named = service::create(&h.state, &me, input("实名"), true)
        .await
        .unwrap();
    let anon = service::create(
        &h.state,
        &me,
        PostInput {
            is_anonymous: true,
            ..input("匿名")
        },
        true,
    )
    .await
    .unwrap();
    let detail = service::detail(&h.state, None, named.id.get())
        .await
        .unwrap();
    assert_eq!(
        detail.author.avatar_url.as_deref(),
        Some(
            "https://cdn.test/2026/10/300.png?image_process=resize,w_360/format,webp/quality,q_80"
        )
    );
    let detail = service::detail(&h.state, None, anon.id.get())
        .await
        .unwrap();
    assert!(
        detail.author.avatar_url.is_none(),
        "匿名帖不带头像（防反推）"
    );
    let feed = service::feed(
        &h.state,
        None,
        FeedQuery {
            category: None,
            tag: None,
            cursor: None,
            limit: None,
        },
    )
    .await
    .unwrap();
    let card = feed.items.iter().find(|c| c.id == named.id).unwrap();
    assert!(card.author.avatar_url.is_some());
}

// ---------- 新帖扇出（后台任务） ----------

impl Harness {
    /// 用户 `from..=to` 全部关注 `followee`。
    async fn followers(&self, followee: i64, from: i64, to: i64) {
        sqlx::query!(
            "INSERT INTO users (id, username, email, role, level)
             SELECT g, 'user' || g, 'user' || g || '@example.com', 'user', 1
             FROM generate_series($1::bigint, $2::bigint) AS g",
            from,
            to
        )
        .execute(&self.db)
        .await
        .unwrap();
        sqlx::query!(
            "INSERT INTO follows (follower_id, followee_id)
             SELECT g, $3 FROM generate_series($1::bigint, $2::bigint) AS g",
            from,
            to,
            followee
        )
        .execute(&self.db)
        .await
        .unwrap();
    }

    /// 把队列里的任务（包括执行中新安排的）全部做完。
    async fn run_jobs(&self) {
        graphile_worker::WorkerOptions::default()
            .schema(ik_kernel::jobs::SCHEMA)
            // run_once 在队列取空时结束；并发大于 1 时它会在上一批还没提交下一批之前就去取，
            // 取不到就提前结束。串行执行才能把任务链跑完。
            .concurrency(1)
            .pg_pool(self.db.clone())
            .listen_os_shutdown_signals(false)
            .define_job::<ik_content::jobs::FanoutNewPost>()
            .add_extension(self.state.clone())
            .init()
            .await
            .unwrap()
            .run_once()
            .await
            .unwrap();
    }

    async fn follow_notices(&self, post_id: i64) -> i64 {
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM notifications WHERE dedup_key = $1"#,
            format!("follow:{post_id}")
        )
        .fetch_one(&self.db)
        .await
        .unwrap()
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn first_publish_fans_out_to_followers_in_batches(db: PgPool) {
    let h = harness(db);
    h.state.jobs.migrate().await.unwrap();
    let me = h.writer(1).await;
    // 超过一批（1000），走任务链。
    h.followers(1, 10_000, 11_499).await;

    let post = service::create(&h.state, &me, input("新委托"), true)
        .await
        .unwrap();
    assert_eq!(
        h.follow_notices(post.id.get()).await,
        0,
        "发布事务里只安排任务"
    );
    h.run_jobs().await;
    assert_eq!(h.follow_notices(post.id.get()).await, 1500);
    let excerpt = sqlx::query_scalar!(
        "SELECT excerpt FROM notifications WHERE recipient_id = 10000 AND kind = 'system'"
    )
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_eq!(excerpt, "你关注的 user1 发布了新委托《新委托》");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn fanout_skips_anonymous_and_withdrawn_posts(db: PgPool) {
    let h = harness(db);
    h.state.jobs.migrate().await.unwrap();
    let me = h.writer(1).await;
    h.followers(1, 100, 102).await;

    let anon = service::create(
        &h.state,
        &me,
        PostInput {
            is_anonymous: true,
            ..input("匿名")
        },
        true,
    )
    .await
    .unwrap();
    // 任务执行前帖子就下线了：不再通知。
    let withdrawn = service::create(&h.state, &me, input("撤回"), true)
        .await
        .unwrap();
    service::unpublish(&h.state, &me, withdrawn.id.get(), false)
        .await
        .unwrap();
    h.run_jobs().await;
    assert_eq!(h.follow_notices(anon.id.get()).await, 0);
    assert_eq!(h.follow_notices(withdrawn.id.get()).await, 0);
}
