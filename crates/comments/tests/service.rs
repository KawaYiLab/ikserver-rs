//! 评论 service 层集成测试：真实 PostgreSQL，每个测试一个独立临时库。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ik_auth::Principal;
use ik_comments::CommentsState;
use ik_comments::dto::{CreateCommentRequest, PageQuery};
use ik_comments::service;
use ik_content::ContentState;
use ik_content::dto::PostInput;
use ik_kernel::bus::LocalBus;
use ik_kernel::{Id, Role, Snowflake};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

struct Harness {
    comments: CommentsState,
    content: ContentState,
    db: PgPool,
}

fn harness(db: PgPool) -> Harness {
    let bus = Arc::new(LocalBus::new(1024));
    let ids = Arc::new(Snowflake::new(1).unwrap());
    Harness {
        comments: CommentsState::new(db.clone(), ids.clone(), bus.clone()),
        content: ContentState::new(db.clone(), ids, bus),
        db,
    }
}

impl Harness {
    async fn user_named(&self, id: i64, name: &str, role: Role, exam: bool) -> Principal {
        sqlx::query!(
            "INSERT INTO users (id, username, email, role, level, exam_passed_at)
             VALUES ($1, $2, $3, $4, 1, CASE WHEN $5 THEN now() END)",
            id,
            name,
            format!("{name}@example.com"),
            role as Role,
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
            level: 1,
        }
    }

    async fn user(&self, id: i64) -> Principal {
        self.user_named(id, &format!("user{id}"), Role::User, true)
            .await
    }

    async fn post(&self, author: &Principal, allow_accept: bool) -> i64 {
        let category_id = if allow_accept {
            let slug = format!("qa{}", author.user_id);
            let c = ik_content::service::create_category(
                &self.content,
                ik_content::service::NewCategoryInput {
                    slug,
                    name: "问答".into(),
                    allow_comment_accept: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            Some(c.id)
        } else {
            None
        };
        let input = PostInput {
            title: "帖子".into(),
            body: String::new(),
            category_id,
            tags: Vec::new(),
            is_anonymous: false,
            external_videos: Vec::new(),
            editor_state: None,
        };
        ik_content::service::create(&self.content, author, input, true)
            .await
            .unwrap()
            .id
            .get()
    }

    async fn comment(&self, who: &Principal, post: i64, parent: Option<i64>, body: &str) -> i64 {
        self.try_comment(who, post, parent, body).await.unwrap()
    }

    async fn try_comment(
        &self,
        who: &Principal,
        post: i64,
        parent: Option<i64>,
        body: &str,
    ) -> Result<i64, ik_kernel::Problem> {
        service::create(
            &self.comments,
            who,
            post,
            CreateCommentRequest {
                body: body.into(),
                parent_id: parent.map(Id::new),
                is_anonymous: false,
            },
        )
        .await
        .map(|c| c.id.get())
    }

    async fn post_counts(&self, post: i64) -> (i32, Option<i64>, bool) {
        let row = sqlx::query!(
            "SELECT comment_count, accepted_comment_id, solved_at IS NOT NULL AS \"solved!\"
             FROM posts WHERE id = $1",
            post
        )
        .fetch_one(&self.db)
        .await
        .unwrap();
        (row.comment_count, row.accepted_comment_id, row.solved)
    }
}

fn first_page() -> PageQuery {
    PageQuery {
        cursor: None,
        limit: None,
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn floors_counts_and_bump(db: PgPool) {
    let h = harness(db);
    let op = h.user(1).await;
    let me = h.user(2).await;
    let post = h.post(&op, false).await;
    let before = ik_content::service::detail(&h.content, None, post)
        .await
        .unwrap()
        .bumped_at;

    let a = h.comment(&me, post, None, "一楼").await;
    let b = h.comment(&me, post, None, "二楼").await;
    let reply = h.comment(&op, post, Some(a), "回复").await;

    let page = service::list(&h.comments, Some(&me), post, first_page())
        .await
        .unwrap();
    let floors: Vec<_> = page
        .items
        .iter()
        .map(|t| (t.comment.id.get(), t.comment.floor))
        .collect();
    assert_eq!(
        floors,
        vec![(b, Some(2)), (a, Some(1))],
        "新的在前，楼层号固定"
    );
    let thread_a = &page.items[1];
    assert_eq!(thread_a.reply_count, 1);
    assert_eq!(thread_a.replies[0].id.get(), reply);
    assert_eq!(thread_a.replies[0].floor, None);
    assert!(page.items[0].comment.is_mine);

    assert_eq!(h.post_counts(post).await.0, 3);
    let after = ik_content::service::detail(&h.content, None, post)
        .await
        .unwrap();
    assert!(after.bumped_at > before, "新评论顶帖");

    // 删除不会让后面的楼层号移动。
    service::delete(&h.comments, &me, a).await.unwrap();
    let c = h.comment(&me, post, None, "三楼").await;
    let page = service::list(&h.comments, None, post, first_page())
        .await
        .unwrap();
    assert_eq!(page.items[0].comment.id.get(), c);
    assert_eq!(page.items[0].comment.floor, Some(3));
    assert_eq!(h.post_counts(post).await.0, 2, "删一级评论连带扣掉它的回复");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_comments_get_unique_floors(db: PgPool) {
    let h = harness(db.clone());
    let op = h.user(1).await;
    let post = h.post(&op, false).await;
    let mut tasks = Vec::new();
    for i in 0..10 {
        let who = h.user(100 + i).await;
        let state = h.comments.clone();
        tasks.push(tokio::spawn(async move {
            service::create(
                &state,
                &who,
                post,
                CreateCommentRequest {
                    body: format!("#{i}"),
                    parent_id: None,
                    is_anonymous: false,
                },
            )
            .await
        }));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    let floors: Vec<Option<i32>> = sqlx::query_scalar!(
        "SELECT floor FROM comments WHERE post_id = $1 ORDER BY floor",
        post
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(floors, (1..=10).map(Some).collect::<Vec<_>>());
    assert_eq!(h.post_counts(post).await.0, 10);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn reply_structure_is_one_level_within_the_post(db: PgPool) {
    let h = harness(db.clone());
    let op = h.user(1).await;
    let me = h.user(2).await;
    let post = h.post(&op, false).await;
    let other_post = h.post(&op, false).await;
    let top = h.comment(&me, post, None, "楼").await;
    let reply = h.comment(&me, post, Some(top), "回复").await;

    for (target_post, parent) in [(post, reply), (other_post, top), (post, 424242)] {
        let err = h
            .try_comment(&me, target_post, Some(parent), "x")
            .await
            .unwrap_err();
        assert_eq!(err.field_errors()[0].field, "parentId");
    }
    sqlx::query!("UPDATE comments SET hidden_at = now() WHERE id = $1", top)
        .execute(&db)
        .await
        .unwrap();
    assert!(
        h.try_comment(&me, post, Some(top), "x").await.is_err(),
        "不能回复被隐藏的评论"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn only_public_posts_accept_comments(db: PgPool) {
    let h = harness(db.clone());
    let op = h.user(1).await;
    let post = h.post(&op, false).await;
    sqlx::query!("UPDATE posts SET hidden_at = now() WHERE id = $1", post)
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(
        h.try_comment(&op, post, None, "x")
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
    // 作者能看到自己被隐藏的帖子的评论区，其他人不能。
    assert!(
        service::list(&h.comments, Some(&op), post, first_page())
            .await
            .is_ok()
    );
    assert!(
        service::list(&h.comments, None, post, first_page())
            .await
            .is_err()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exam_gate_rate_limit_and_body_rules(db: PgPool) {
    let h = harness(db);
    let op = h.user(1).await;
    let post = h.post(&op, false).await;
    let newbie = h.user_named(2, "newbie", Role::User, false).await;
    assert_eq!(
        h.try_comment(&newbie, post, None, "x")
            .await
            .unwrap_err()
            .code(),
        "EXAM_REQUIRED"
    );

    let me = h.user(3).await;
    assert_eq!(
        h.try_comment(&me, post, None, "   ")
            .await
            .unwrap_err()
            .field_errors()[0]
            .code,
        "REQUIRED"
    );
    let emotes = ":ik-a:".repeat(21);
    assert_eq!(
        h.try_comment(&me, post, None, &emotes)
            .await
            .unwrap_err()
            .field_errors()[0]
            .code,
        "TOO_MANY_EMOTES"
    );
    assert_eq!(
        h.try_comment(&me, post, None, &"字".repeat(2001))
            .await
            .unwrap_err()
            .field_errors()[0]
            .code,
        "TOO_LONG"
    );

    // 被拒绝的请求也计入频率（上面 3 次），换一个新用户测上限。
    // 11 条同时发出：限流判断在访问数据库之前，结果与远程库的耗时无关
    // （串行发送时，10 条评论耗时一旦超过 6 秒就会回补 1 个额度）。
    let me = h.user(4).await;
    let bodies: Vec<String> = (0..11).map(|i| format!("第 {i} 条")).collect();
    let attempts = futures_util::future::join_all(
        bodies
            .iter()
            .map(|body| h.try_comment(&me, post, None, body)),
    )
    .await;
    let limited = attempts
        .iter()
        .filter(|r| r.as_ref().is_err_and(|e| e.code() == "RATE_LIMITED"))
        .count();
    assert_eq!(limited, 1);
    assert_eq!(attempts.iter().filter(|r| r.is_ok()).count(), 10);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bare_mentions_expand_to_tokens(db: PgPool) {
    let h = harness(db.clone());
    let op = h.user(1).await;
    let alice = h.user_named(7, "alice", Role::User, true).await;
    let post = h.post(&op, false).await;
    let id = h
        .comment(&op, post, None, "@alice 看看 @Alice 和 me@alice.com @ghost")
        .await;
    let body = sqlx::query_scalar!("SELECT body FROM comments WHERE id = $1", id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(
        body, "@[alice](7) 看看 @Alice 和 me@alice.com @ghost",
        "大小写敏感，邮箱不改写"
    );
    assert_eq!(
        ik_comments::text::mentioned_user_ids(&body),
        vec![alice.user_id.get()]
    );

    // 展开后超长要拒绝（D-033）。
    let near_limit = format!("{} @alice", "字".repeat(1993));
    assert_eq!(near_limit.chars().count(), 2000);
    assert_eq!(
        h.try_comment(&op, post, None, &near_limit)
            .await
            .unwrap_err()
            .field_errors()[0]
            .code,
        "TOO_LONG"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn anonymous_comments_hide_author(db: PgPool) {
    let h = harness(db);
    let op = h.user(1).await;
    let me = h.user(2).await;
    let post = h.post(&op, false).await;
    let mut names = Vec::new();
    for _ in 0..2 {
        let view = service::create(
            &h.comments,
            &me,
            post,
            CreateCommentRequest {
                body: "匿名".into(),
                parent_id: None,
                is_anonymous: true,
            },
        )
        .await
        .unwrap();
        assert!(view.author.id.is_none());
        names.push(view.author.name);
    }
    assert_ne!(names[0], names[1], "每条匿名评论各自独立的匿名名");
    let page = service::list(&h.comments, Some(&op), post, first_page())
        .await
        .unwrap();
    assert!(
        page.items
            .iter()
            .all(|t| t.comment.author.anonymous && !t.comment.is_mine)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn pin_rules(db: PgPool) {
    let h = harness(db);
    let op = h.user(1).await;
    let me = h.user(2).await;
    let admin = h.user_named(3, "admin", Role::Admin, false).await;
    let post = h.post(&op, false).await;
    let a = h.comment(&me, post, None, "a").await;
    let b = h.comment(&me, post, None, "b").await;
    let reply = h.comment(&me, post, Some(a), "r").await;

    assert_eq!(
        service::pin(&h.comments, &me, a).await.unwrap_err().code(),
        "FORBIDDEN"
    );
    assert_eq!(
        service::pin(&h.comments, &op, reply)
            .await
            .unwrap_err()
            .code(),
        "COMMENT_NOT_TOP_LEVEL"
    );
    service::pin(&h.comments, &op, a).await.unwrap();
    service::pin(&h.comments, &op, a).await.unwrap();
    service::pin(&h.comments, &admin, b).await.unwrap();

    let page = service::list(&h.comments, None, post, first_page())
        .await
        .unwrap();
    assert_eq!(
        page.pinned.as_ref().map(|t| t.comment.id.get()),
        Some(b),
        "新置顶替换旧置顶"
    );
    assert_eq!(
        page.items
            .iter()
            .map(|t| t.comment.id.get())
            .collect::<Vec<_>>(),
        vec![a]
    );

    service::unpin(&h.comments, &op, b).await.unwrap();
    service::unpin(&h.comments, &op, b).await.unwrap();
    assert!(
        service::list(&h.comments, None, post, first_page())
            .await
            .unwrap()
            .pinned
            .is_none()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn accept_rules(db: PgPool) {
    let h = harness(db.clone());
    let op = h.user(1).await;
    let me = h.user(2).await;
    let admin = h.user_named(3, "admin", Role::Admin, false).await;

    let closed = h.post(&op, false).await;
    let c = h.comment(&me, closed, None, "答").await;
    assert_eq!(
        service::accept(&h.comments, &admin, c)
            .await
            .unwrap_err()
            .code(),
        "ACCEPT_DISABLED"
    );

    let qa = h.post(&op, true).await;
    let a = h.comment(&me, qa, None, "答案 A").await;
    let b = h.comment(&me, qa, None, "答案 B").await;
    assert_eq!(
        service::accept(&h.comments, &me, a)
            .await
            .unwrap_err()
            .code(),
        "FORBIDDEN"
    );
    service::accept(&h.comments, &op, a).await.unwrap();
    let first = sqlx::query_scalar!("SELECT accepted_at FROM comments WHERE id = $1", a)
        .fetch_one(&db)
        .await
        .unwrap();
    service::accept(&h.comments, &op, a).await.unwrap();
    let again = sqlx::query_scalar!("SELECT accepted_at FROM comments WHERE id = $1", a)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(first, again, "重复采纳是空操作（D-030）");
    assert_eq!(h.post_counts(qa).await.1, Some(a));

    service::accept(&h.comments, &op, b).await.unwrap();
    assert_eq!(h.post_counts(qa).await.1, Some(b), "采纳新的替换旧的");
    let page = service::list(&h.comments, None, qa, first_page())
        .await
        .unwrap();
    assert_eq!(page.accepted.as_ref().map(|t| t.comment.id.get()), Some(b));
    assert!(page.accepted.as_ref().unwrap().comment.is_accepted);
    assert_eq!(page.items.len(), 1);

    // 取消旧的（已不是采纳）不影响帖子当前的采纳。
    service::unaccept(&h.comments, &op, a).await.unwrap();
    assert_eq!(h.post_counts(qa).await.1, Some(b));
    service::unaccept(&h.comments, &op, b).await.unwrap();
    assert_eq!(h.post_counts(qa).await, (2, None, false));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn deleting_accepted_comment_clears_solved(db: PgPool) {
    let h = harness(db);
    let op = h.user(1).await;
    let me = h.user(2).await;
    let qa = h.post(&op, true).await;
    let a = h.comment(&me, qa, None, "答").await;
    h.comment(&op, qa, Some(a), "谢谢").await;
    service::accept(&h.comments, &op, a).await.unwrap();
    assert_eq!(
        service::delete(&h.comments, &op, a)
            .await
            .unwrap_err()
            .code(),
        "FORBIDDEN"
    );
    service::delete(&h.comments, &me, a).await.unwrap();
    assert_eq!(h.post_counts(qa).await, (0, None, false));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn inline_replies_are_per_thread_and_exclude_hidden(db: PgPool) {
    let h = harness(db.clone());
    let op = h.user(1).await;
    let me = h.user(2).await;
    let post = h.post(&op, false).await;
    let a = h.comment(&me, post, None, "a").await;
    let b = h.comment(&op, post, None, "b").await;
    let mut a_replies = Vec::new();
    for i in 0..5 {
        a_replies.push(h.comment(&op, post, Some(a), &format!("a{i}")).await);
    }
    h.comment(&me, post, Some(b), "b0").await;
    sqlx::query!(
        "UPDATE comments SET hidden_at = now() WHERE id = $1",
        a_replies[1]
    )
    .execute(&db)
    .await
    .unwrap();

    let page = service::list(&h.comments, None, post, first_page())
        .await
        .unwrap();
    let thread_a = page.items.iter().find(|t| t.comment.id.get() == a).unwrap();
    let thread_b = page.items.iter().find(|t| t.comment.id.get() == b).unwrap();
    assert_eq!(
        thread_a
            .replies
            .iter()
            .map(|r| r.id.get())
            .collect::<Vec<_>>(),
        vec![a_replies[0], a_replies[2], a_replies[3]]
    );
    assert_eq!((thread_a.reply_count, thread_a.replies_has_more), (4, true));
    assert_eq!(
        (
            thread_b.replies.len(),
            thread_b.reply_count,
            thread_b.replies_has_more
        ),
        (1, 1, false)
    );

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = service::replies(
            &h.comments,
            None,
            a,
            PageQuery {
                cursor,
                limit: Some(2),
            },
        )
        .await
        .unwrap();
        seen.extend(page.items.iter().map(|r| r.id.get()));
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(
        seen,
        vec![a_replies[0], a_replies[2], a_replies[3], a_replies[4]]
    );
    assert_eq!(
        service::replies(&h.comments, None, a_replies[0], first_page())
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND",
        "回复没有下一层"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn top_level_pagination_is_stable(db: PgPool) {
    let h = harness(db.clone());
    let op = h.user(1).await;
    let post = h.post(&op, false).await;
    let mut all = Vec::new();
    for i in 0..7 {
        let who = h.user(10 + i).await;
        all.push(h.comment(&who, post, None, "x").await);
    }
    sqlx::query!("UPDATE comments SET created_at = '2026-01-01T00:00:00Z'")
        .execute(&db)
        .await
        .unwrap();
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = service::list(
            &h.comments,
            None,
            post,
            PageQuery {
                cursor,
                limit: Some(3),
            },
        )
        .await
        .unwrap();
        seen.extend(page.items.iter().map(|t| t.comment.id.get()));
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    all.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(seen, all);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn deleting_post_cascades_comments(db: PgPool) {
    let h = harness(db.clone());
    let op = h.user(1).await;
    let post = h.post(&op, true).await;
    let a = h.comment(&op, post, None, "a").await;
    h.comment(&op, post, Some(a), "r").await;
    service::accept(&h.comments, &op, a).await.unwrap();
    ik_content::service::delete(&h.content, &op, post)
        .await
        .unwrap();
    let left = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM comments"#)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

async fn exp_of(db: &PgPool, id: i64) -> i64 {
    sqlx::query_scalar!("SELECT exp FROM user_economy WHERE user_id = $1", id)
        .fetch_optional(db)
        .await
        .unwrap()
        .unwrap_or(0)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn comment_and_accept_grant_exp(db: PgPool) {
    let h = harness(db);
    let owner = h.user(1).await;
    let answerer = h.user(2).await;
    let post = h.post(&owner, true).await;
    let owner_exp = exp_of(&h.db, 1).await;

    // 在自己的帖子下评论不给经验。
    h.comment(&owner, post, None, "自己顶一下").await;
    assert_eq!(exp_of(&h.db, 1).await, owner_exp);

    // 在别人的帖子下评论：每天第一条 +3。
    let answer = h.comment(&answerer, post, None, "答案").await;
    h.comment(&answerer, post, None, "补充").await;
    assert_eq!(exp_of(&h.db, 2).await, 3);

    // 被采纳 +10，反复采纳 / 取消不重复发放。
    service::accept(&h.comments, &owner, answer).await.unwrap();
    assert_eq!(exp_of(&h.db, 2).await, 13);
    service::unaccept(&h.comments, &owner, answer)
        .await
        .unwrap();
    service::accept(&h.comments, &owner, answer).await.unwrap();
    assert_eq!(exp_of(&h.db, 2).await, 13);

    // 采纳自己的评论不给经验。
    let own = h.comment(&owner, post, None, "我自己的答案").await;
    service::accept(&h.comments, &owner, own).await.unwrap();
    assert_eq!(exp_of(&h.db, 1).await, owner_exp);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn same_user_commenting_concurrently_does_not_deadlock(db: PgPool) {
    // 评论插入的外键检查会对用户行加 KEY SHARE，随后发经验要锁同一用户行；
    // 两个事务交错时不能互相等待（锁用 FOR NO KEY UPDATE）。
    let h = harness(db);
    let op = h.user(1).await;
    let me = h.user(2).await;
    let posts = [h.post(&op, false).await, h.post(&op, false).await];
    let bodies: Vec<String> = (0..6).map(|i| format!("并发 {i}")).collect();
    let results = futures_util::future::join_all(
        bodies
            .iter()
            .enumerate()
            .map(|(i, body)| h.try_comment(&me, posts[i % 2], None, body)),
    )
    .await;
    assert!(results.iter().all(Result::is_ok), "{results:?}");
    assert_eq!(exp_of(&h.db, 2).await, 3, "每天只有第一条评论给经验");
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
async fn blocked_users_cannot_comment_or_reply(db: PgPool) {
    let h = harness(db);
    let op = h.user(1).await;
    let alice = h.user(2).await;
    let carol = h.user(3).await;
    let post = h.post(&op, false).await;
    let top = h.comment(&alice, post, None, "一楼").await;

    // 帖主拉黑了 carol：carol 不能在帖下评论。
    block(&h.db, 1, 3).await;
    let err = h.try_comment(&carol, post, None, "评论").await.unwrap_err();
    assert_eq!(err.code(), "USER_BLOCKED");

    // carol 拉黑了 alice：alice 回复不了 carol，carol 也回复不了 alice。
    let op_post = h.post(&alice, false).await;
    sqlx::query!("DELETE FROM user_blocks")
        .execute(&h.db)
        .await
        .unwrap();
    block(&h.db, 3, 2).await;
    let err = h
        .try_comment(&carol, post, Some(top), "回复")
        .await
        .unwrap_err();
    assert_eq!(err.code(), "USER_BLOCKED");
    let err = h
        .try_comment(&carol, op_post, None, "评论")
        .await
        .unwrap_err();
    assert_eq!(err.code(), "USER_BLOCKED");
    assert_eq!(h.post_counts(post).await.0, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn lists_hide_blocked_authors_but_not_anonymous_comments(db: PgPool) {
    let h = harness(db);
    let op = h.user(1).await;
    let alice = h.user(2).await;
    let carol = h.user(3).await;
    let post = h.post(&op, false).await;
    let named = h.comment(&alice, post, None, "实名").await;
    let anonymous = service::create(
        &h.comments,
        &alice,
        post,
        CreateCommentRequest {
            body: "匿名".into(),
            parent_id: None,
            is_anonymous: false,
        },
    )
    .await
    .unwrap()
    .id
    .get();
    sqlx::query!(
        "UPDATE comments SET is_anonymous = TRUE, anonymous_seed = 'seed' WHERE id = $1",
        anonymous
    )
    .execute(&h.db)
    .await
    .unwrap();
    let mine = h.comment(&carol, post, None, "我的").await;
    h.comment(&alice, post, Some(mine), "回复 carol").await;
    h.comment(&op, post, Some(mine), "帖主回复").await;
    block(&h.db, 3, 2).await;

    let page = service::list(&h.comments, Some(&carol), post, first_page())
        .await
        .unwrap();
    let ids: Vec<i64> = page.items.iter().map(|t| t.comment.id.get()).collect();
    assert_eq!(ids, vec![mine, anonymous]);
    assert_eq!(page.items[0].reply_count, 1);
    assert_eq!(page.items[0].replies.len(), 1);

    let replies = service::replies(&h.comments, Some(&carol), mine, first_page())
        .await
        .unwrap();
    assert_eq!(replies.items.len(), 1);
    let err = service::replies(&h.comments, Some(&carol), named, first_page())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "NOT_FOUND");

    // 游客看到全部。
    let page = service::list(&h.comments, None, post, first_page())
        .await
        .unwrap();
    assert_eq!(page.items.len(), 3);
    assert_eq!(page.items[0].reply_count, 2);
}

// ---------- 评论点赞 ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn comment_likes_count_once_and_show_in_lists(db: PgPool) {
    let h = harness(db);
    let op = h.user(1).await;
    let fan = h.user(2).await;
    let post = h.post(&op, false).await;
    let top = h.comment(&op, post, None, "一楼").await;
    let reply = h.comment(&op, post, Some(top), "回复").await;

    let liked = service::like(&h.comments, &fan, top).await.unwrap();
    assert!(liked.liked);
    assert_eq!(liked.like_count, 1);
    assert_eq!(
        service::like(&h.comments, &fan, top)
            .await
            .unwrap()
            .like_count,
        1
    );
    service::like(&h.comments, &fan, reply).await.unwrap();
    // 帖子和评论的点赞经验共用「每日第一次」。
    ik_content::service::like(&h.content, &fan, post)
        .await
        .unwrap();
    assert_eq!(exp_of(&h.db, 2).await, 1);

    let page = service::list(&h.comments, Some(&fan), post, first_page())
        .await
        .unwrap();
    assert!(page.items[0].comment.liked);
    assert_eq!(page.items[0].comment.like_count, 1);
    assert!(page.items[0].replies[0].liked);
    let page = service::list(&h.comments, Some(&op), post, first_page())
        .await
        .unwrap();
    assert!(!page.items[0].comment.liked);

    let off = service::unlike(&h.comments, &fan, top).await.unwrap();
    assert_eq!((off.liked, off.like_count), (false, 0));
    assert_eq!(
        service::unlike(&h.comments, &fan, top)
            .await
            .unwrap()
            .like_count,
        0
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn comment_like_rules(db: PgPool) {
    let h = harness(db);
    let op = h.user(1).await;
    let fan = h.user(2).await;
    let newbie = h.user_named(3, "newbie", Role::User, false).await;
    let post = h.post(&op, false).await;
    let top = h.comment(&op, post, None, "一楼").await;
    let reply = h.comment(&op, post, Some(top), "回复").await;

    assert_eq!(
        service::like(&h.comments, &newbie, top)
            .await
            .unwrap_err()
            .code(),
        "EXAM_REQUIRED"
    );
    service::like(&h.comments, &fan, reply).await.unwrap();
    // 一级评论被隐藏后，它的回复也不能再赞，但可以取消。
    sqlx::query!("UPDATE comments SET hidden_at = now() WHERE id = $1", top)
        .execute(&h.db)
        .await
        .unwrap();
    assert_eq!(
        service::like(&h.comments, &fan, top)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
    assert_eq!(
        service::like(&h.comments, &op, reply)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
    assert_eq!(
        service::unlike(&h.comments, &fan, reply)
            .await
            .unwrap()
            .like_count,
        0
    );

    let second = h.comment(&op, post, None, "二楼").await;
    block(&h.db, 1, 2).await;
    assert_eq!(
        service::like(&h.comments, &fan, second)
            .await
            .unwrap_err()
            .code(),
        "USER_BLOCKED"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn deleting_a_comment_removes_its_likes(db: PgPool) {
    let h = harness(db);
    let op = h.user(1).await;
    let fan = h.user(2).await;
    let post = h.post(&op, false).await;
    let top = h.comment(&op, post, None, "一楼").await;
    let reply = h.comment(&op, post, Some(top), "回复").await;
    service::like(&h.comments, &fan, top).await.unwrap();
    service::like(&h.comments, &fan, reply).await.unwrap();
    service::delete(&h.comments, &op, top).await.unwrap();
    let left = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM comment_likes"#)
        .fetch_one(&h.db)
        .await
        .unwrap();
    assert_eq!(left, 0, "D-029：不留孤儿点赞行");
}
