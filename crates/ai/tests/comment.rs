//! AI 评论回复：触发判定、回复挂在一级评论下、上下文与记忆、复核、额度、沉浸守卫、重试、通知。
//! 上游用脚本化的假提供方（实现 `ChatProvider`），不连网络。

// clippy.toml 的测试豁免只覆盖 #[test] 函数本身，不覆盖集成测试里的辅助函数。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::Utc;
use ik_ai::AiState;
use ik_ai::comment;
use ik_ai::dm::RunOutcome;
use ik_ai::dto::ImportFile;
use ik_ai::llm::{ChatMessage, LlmError, Providers};
use ik_ai::{immersion, quota, service};
use ik_auth::Principal;
use ik_comments::dto::CreateCommentRequest;
use ik_content::ContentState;
use ik_content::dto::PostInput;
use ik_kernel::bus::LocalBus;
use ik_kernel::config::Ai;
use ik_kernel::{Id, Role, Snowflake};
use ik_messaging::MessagingState;
use sqlx::PgPool;

mod common;
use common::{Scripted, Step, content, usage};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

const ALICE: i64 = 101;
const BOB: i64 = 102;
const FAIRY: i64 = 200;
const QUIET: i64 = 201;

struct Harness {
    db: PgPool,
    state: AiState,
    content: ContentState,
    fake: Arc<Scripted>,
}

fn principal(id: i64, ai: bool) -> Principal {
    Principal {
        user_id: Id::new(id),
        session_id: Id::new(id),
        role: Role::User,
        exam_passed: true,
        is_ai_agent: ai,
        level: 1,
    }
}

async fn insert_user(db: &PgPool, id: i64, ai: bool) {
    sqlx::query!(
        "INSERT INTO users (id, username, email, role, level, is_ai_agent, exam_passed_at)
         VALUES ($1, $2, $3, 'user', 1, $4, now())",
        id,
        format!("user{id}"),
        format!("user{id}@example.com"),
        ai,
    )
    .execute(db)
    .await
    .unwrap();
}

async fn harness_with(db: PgPool, per_user: i64) -> Harness {
    ik_kernel::jobs::Jobs::new(db.clone())
        .migrate()
        .await
        .unwrap();
    let ids = Arc::new(Snowflake::new(1).unwrap());
    let bus = Arc::new(LocalBus::new(1024));
    let media = ik_media::MediaState::for_tests(db.clone(), ids.clone());
    let messaging = MessagingState::new(db.clone(), ids.clone(), bus.clone(), media.clone());
    let forum = ik_ai::Forum::for_tests(db.clone(), ids, bus, media).unwrap();
    let content = forum.content.clone();
    let fake = Arc::new(Scripted::default());
    let providers = Providers::default().with("fake", fake.clone());
    let state = AiState::new(messaging, forum, Ai::default(), providers);
    assert!(state.install_hooks());
    for (id, ai) in [(ALICE, false), (BOB, false), (FAIRY, true), (QUIET, true)] {
        insert_user(&db, id, ai).await;
    }
    let file: ImportFile = serde_json::from_value(serde_json::json!({
        "settings": { "globalEnabled": true, "dailyTokensPerUser": per_user, "dailyTokens": 0 },
        "models": [
            { "key": "base", "displayName": "基础", "provider": "fake",
              "providerModel": "fake-base", "isDefault": true, "sortOrder": 1 }
        ],
        "characters": [
            { "slug": "fairy", "displayName": "Fairy", "username": format!("user{FAIRY}"),
              "enabled": true, "systemPrompt": "你是 Fairy。" },
            { "slug": "quiet", "displayName": "Quiet", "username": format!("user{QUIET}"),
              "enabled": true, "commentEnabled": false, "systemPrompt": "你是 Quiet。" }
        ]
    }))
    .unwrap();
    service::import(&state, file).await.unwrap();
    Harness {
        db,
        state,
        content,
        fake,
    }
}

async fn harness(db: PgPool) -> Harness {
    harness_with(db, 1_000_000).await
}

impl Harness {
    async fn post(&self, author: i64, anonymous: bool) -> i64 {
        let input = PostInput {
            title: "六分街的传闻".into(),
            body: "昨晚有人在六分街看到了以骸。".into(),
            category_id: None,
            tags: Vec::new(),
            is_anonymous: anonymous,
            external_videos: Vec::new(),
            editor_state: None,
            cover_ids: Vec::new(),
        };
        let who = principal(author, author == FAIRY || author == QUIET);
        ik_content::service::create(&self.content, &who, input, true)
            .await
            .unwrap()
            .id
            .get()
    }

    async fn comment_as(
        &self,
        who: i64,
        post: i64,
        parent: Option<i64>,
        body: &str,
        anonymous: bool,
    ) -> i64 {
        ik_comments::service::create(
            &self.state.comments,
            &principal(who, who == FAIRY || who == QUIET),
            post,
            CreateCommentRequest {
                body: body.into(),
                image_ids: Vec::new(),
                reply_to_id: parent.map(Id::new),
                is_anonymous: anonymous,
            },
        )
        .await
        .unwrap()
        .id
        .get()
    }

    async fn comment(&self, who: i64, post: i64, parent: Option<i64>, body: &str) -> i64 {
        self.comment_as(who, post, parent, body, false).await
    }

    async fn job_for(&self, trigger: i64) -> Option<i64> {
        sqlx::query_scalar!(
            "SELECT id FROM ai_jobs WHERE execution_key = $1",
            format!("comment:{trigger}")
        )
        .fetch_optional(&self.db)
        .await
        .unwrap()
    }

    async fn job_count(&self) -> i64 {
        sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM ai_jobs"#)
            .fetch_one(&self.db)
            .await
            .unwrap()
    }

    async fn run(&self, trigger: i64) -> RunOutcome {
        let job = self.job_for(trigger).await.expect("没有建任务");
        comment::run_job(&self.state, job, 1, false).await.unwrap()
    }

    /// (回复 id, 回复了哪条, 正文)
    async fn reply(&self, trigger: i64) -> Option<(i64, Option<i64>, String)> {
        sqlx::query!(
            "SELECT c.id, c.reply_to_id, c.body, c.author_id, c.is_anonymous FROM ai_jobs j
             JOIN comments c ON c.id = j.reply_comment_id WHERE j.execution_key = $1",
            format!("comment:{trigger}")
        )
        .fetch_optional(&self.db)
        .await
        .unwrap()
        .map(|r| {
            assert_eq!(r.author_id, FAIRY);
            assert!(!r.is_anonymous);
            (r.id, r.reply_to_id, r.body)
        })
    }

    async fn job_state(&self, trigger: i64) -> (String, Option<String>) {
        let row = sqlx::query!(
            "SELECT status, reason FROM ai_jobs WHERE execution_key = $1",
            format!("comment:{trigger}")
        )
        .fetch_one(&self.db)
        .await
        .unwrap();
        (row.status, row.reason)
    }

    async fn used(&self, user: i64) -> i64 {
        let mut conn = self.db.acquire().await.unwrap();
        quota::read(&mut conn, user, Utc::now()).await.unwrap().used
    }

    fn answer(&self, text: &str) {
        self.fake.push(vec![content(text), usage(100, 20)]);
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn mention_triggers_a_reply_under_the_comment(db: PgPool) {
    let h = harness(db).await;
    let post = h.post(ALICE, false).await;
    // 裸 @ 用户名会展开成提及 token（TRIG-011）。
    let trigger = h.comment(BOB, post, None, "@user200 这是真的吗？").await;
    let job = h.job_for(trigger).await.unwrap();
    // 任务与评论同一事务入队（TRIG-010）。
    let queued: i64 = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM graphile_worker.jobs WHERE key = $1"#,
        format!("ai:{job}")
    )
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_eq!(queued, 1);

    h.answer("**暂时**无法确认，建议先观望。");
    assert_eq!(h.run(trigger).await, RunOutcome::Completed);
    let (_, parent, body) = h.reply(trigger).await.unwrap();
    assert_eq!(parent, Some(trigger));
    assert_eq!(
        body,
        format!("@[user{BOB}]({BOB})\n暂时无法确认，建议先观望。")
    );

    // 上下文（CTX-001/002）：system 里有人格、评论回复要求与帖子；最后一条是本次评论。
    let request = h.fake.requests().pop().unwrap();
    assert_eq!(request.model, "fake-base");
    let system = request.messages.first().unwrap().text().to_owned();
    assert!(system.starts_with("你是 Fairy。"));
    assert!(system.contains("【评论回复】"));
    assert!(system.contains("【当前帖子】六分街的传闻\n昨晚有人在六分街看到了以骸。"));
    let last = request.messages.last().unwrap().text().to_owned();
    assert_eq!(
        last,
        format!(
            "当前帖子：六分街的传闻（document_id: {post}）\nuser{BOB} 说：@user200 这是真的吗？"
        )
    );

    // 计费给评论者（CTX-001），审计记完成。
    assert_eq!(h.used(BOB).await, 120);
    assert_eq!(h.job_state(trigger).await.0, "completed");
    // 评论数含 AI 的回复。
    let count: i32 = sqlx::query_scalar!("SELECT comment_count FROM posts WHERE id = $1", post)
        .fetch_one(&h.db)
        .await
        .unwrap();
    assert_eq!(count, 2);
    // AI 的回复照常通知被回复的人，自成一组（D-091 方案 A）；回复里 @ 到的也是他，不重复。
    let from_ai: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT recipient_id, kind, bucket_key FROM notifications WHERE sender_id = $1",
    )
    .bind(FAIRY)
    .fetch_all(&h.db)
    .await
    .unwrap();
    assert_eq!(
        from_ai,
        vec![(BOB, "reply".to_owned(), format!("user-{FAIRY}"))]
    );

    // 重放：已有回复，不再调用模型（TRIG-037）。
    let calls = h.fake.requests().len();
    assert_eq!(
        comment::run_job(&h.state, job, 2, false).await.unwrap(),
        RunOutcome::AlreadyDone
    );
    assert_eq!(h.fake.requests().len(), calls);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn replies_answer_the_trigger_with_its_target_as_context(db: PgPool) {
    let h = harness(db).await;
    let post = h.post(ALICE, false).await;
    let top = h.comment(ALICE, post, None, "我觉得是假的").await;
    assert!(h.job_for(top).await.is_none());
    // 回复里 @AI（D-004 方案 A）：AI 回复触发评论本身，被回复的评论作为父评论进上下文。
    let trigger = h.comment(BOB, post, Some(top), "@user200 你怎么看").await;
    h.answer("我也倾向于不信。");
    assert_eq!(h.run(trigger).await, RunOutcome::Completed);
    let (ai_reply, parent, _) = h.reply(trigger).await.unwrap();
    assert_eq!(parent, Some(trigger));
    let last = h.fake.requests().pop().unwrap();
    let last = last.messages.last().unwrap().text().to_owned();
    assert!(last.contains("\n父评论：我觉得是假的\n"));

    // AI 自己的回复不再触发（TRIG-012）。
    assert!(h.job_for(ai_reply).await.is_none());
    // 被回复的是 AI：回复 AI 的评论触发（TRIG-013 ②），包括 AI 刚发的那条回复。
    let ai_top = h.comment(FAIRY, post, None, "大家好").await;
    assert!(h.job_for(ai_top).await.is_none());
    let under_ai = h.comment(BOB, post, Some(ai_top), "你好呀").await;
    assert!(h.job_for(under_ai).await.is_some());
    let follow_up = h.comment(BOB, post, Some(ai_reply), "为什么？").await;
    assert!(h.job_for(follow_up).await.is_some());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn trigger_rules(db: PgPool) {
    let h = harness(db).await;
    let human_post = h.post(ALICE, false).await;
    // 没有提及 AI、帖子也不是 AI 发的：不触发。
    let plain = h.comment(BOB, human_post, None, "路过").await;
    assert!(h.job_for(plain).await.is_none());
    // 匿名评论不触发（TRIG-012）。
    let anonymous = h
        .comment_as(BOB, human_post, None, "@user200 在吗", true)
        .await;
    assert!(h.job_for(anonymous).await.is_none());
    // 角色关闭了评论回复（TRIG-014）。
    let quiet = h.comment(BOB, human_post, None, "@user201 在吗").await;
    assert!(h.job_for(quiet).await.is_none());

    // AI 发的帖下，任何实名评论都触发（TRIG-013 ③）。
    let ai_post = h.post(FAIRY, false).await;
    let any = h.comment(BOB, ai_post, None, "沙发").await;
    assert!(h.job_for(any).await.is_some());
    // 匿名帖下不触发。
    let anonymous_post = h.post(ALICE, true).await;
    let under_anonymous = h.comment(BOB, anonymous_post, None, "@user200 看看").await;
    assert!(h.job_for(under_anonymous).await.is_none());

    // 全局关闭：不入队（TRIG-001）。
    sqlx::query!("UPDATE ai_settings SET global_enabled = FALSE")
        .execute(&h.db)
        .await
        .unwrap();
    h.state.catalog.invalidate().await;
    let before = h.job_count().await;
    h.comment(BOB, ai_post, None, "还在吗").await;
    assert_eq!(h.job_count().await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn memory_is_per_post_commenter_and_ai(db: PgPool) {
    let h = harness(db).await;
    let post = h.post(FAIRY, false).await;
    let first = h.comment(BOB, post, None, "第一个问题").await;
    h.answer("第一个回答。");
    assert_eq!(h.run(first).await, RunOutcome::Completed);
    // 别人的评论不共享记忆（CTX-003）。
    let other = h.comment(ALICE, post, None, "我也问").await;
    h.answer("好的。");
    h.run(other).await;
    assert_eq!(h.fake.requests().pop().unwrap().messages.len(), 2);

    let second = h.comment(BOB, post, None, "第二个问题").await;
    h.answer("第二个回答。");
    h.run(second).await;
    let messages = h.fake.requests().pop().unwrap().messages;
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[1].text(), format!("user{BOB} 说：第一个问题"));
    // AI 自己的话不带系统加的提及。
    assert_eq!(messages[2].text(), "第一个回答。");

    // 换一篇帖子从头开始。
    let other_post = h.post(FAIRY, false).await;
    let fresh = h.comment(BOB, other_post, None, "新帖").await;
    h.answer("好。");
    h.run(fresh).await;
    assert_eq!(h.fake.requests().pop().unwrap().messages.len(), 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn removed_trigger_is_skipped_without_charging(db: PgPool) {
    let h = harness(db).await;
    let post = h.post(ALICE, false).await;
    let trigger = h.comment(BOB, post, None, "@user200 在吗").await;
    ik_comments::service::delete(&h.state.comments, &principal(BOB, false), trigger)
        .await
        .unwrap();
    // 评论删除后任务行保留（审计），引用置空；执行时复核跳过（TRIG-016）。
    let job: i64 = sqlx::query_scalar!(
        "SELECT id FROM ai_jobs WHERE execution_key = $1",
        format!("comment:{trigger}")
    )
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_eq!(
        comment::run_job(&h.state, job, 1, false).await.unwrap(),
        RunOutcome::Skipped("trigger_removed")
    );
    assert!(h.fake.requests().is_empty());
    assert_eq!(h.used(BOB).await, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn quota_denial_skips_silently(db: PgPool) {
    let h = harness_with(db, 1).await;
    let post = h.post(FAIRY, false).await;
    let first = h.comment(BOB, post, None, "一").await;
    h.answer("回答。");
    assert_eq!(h.run(first).await, RunOutcome::Completed);
    let second = h.comment(BOB, post, None, "二").await;
    assert_eq!(
        h.run(second).await,
        RunOutcome::Skipped("daily_token_budget_exceeded")
    );
    assert!(h.reply(second).await.is_none());
    // 评论区不发提示（只有私信有系统提示位）。
    let comments: i64 = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM comments WHERE author_id = $1"#,
        FAIRY
    )
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_eq!(comments, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn immersion_guard_replaces_meta_replies(db: PgPool) {
    let h = harness(db).await;
    let post = h.post(FAIRY, false).await;
    let trigger = h.comment(BOB, post, None, "这是真的吗").await;
    h.answer("这只是基于游戏的同人创作。");
    assert_eq!(h.run(trigger).await, RunOutcome::Completed);
    let (_, _, body) = h.reply(trigger).await.unwrap();
    assert_eq!(
        body,
        format!("@[user{BOB}]({BOB})\n{}", immersion::RUMOR_REPLY)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failures_retry_then_fail_without_a_reply(db: PgPool) {
    let h = harness(db).await;
    let post = h.post(FAIRY, false).await;
    let trigger = h.comment(BOB, post, None, "在吗").await;
    let job = h.job_for(trigger).await.unwrap();
    h.fake.push(vec![Step::Fail(LlmError::Rejected(
        "400 bad request".into(),
    ))]);
    assert!(matches!(
        comment::run_job(&h.state, job, 1, false).await.unwrap(),
        RunOutcome::Retry(_)
    ));
    assert_eq!(h.job_state(trigger).await.0, "processing");
    // 上游的错误文本不作为回复（GEN-024）；最后一次失败记最终失败。
    h.fake.push(vec![content("   ")]);
    assert!(matches!(
        comment::run_job(&h.state, job, 3, true).await.unwrap(),
        RunOutcome::Failed(_)
    ));
    let (status, reason) = h.job_state(trigger).await;
    assert_eq!(status, "failed");
    assert_eq!(reason.as_deref(), Some("empty reply after sanitize"));
    assert!(h.reply(trigger).await.is_none());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn comment_replies_can_use_tools_but_keep_only_the_final_answer(db: PgPool) {
    let h = harness(db).await;
    let post = h.post(FAIRY, false).await;
    let trigger = h.comment(BOB, post, None, "这帖子热度怎么样").await;
    h.fake.push(vec![
        content("我查一下。"),
        Step::Emit(ik_ai::llm::StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_1".into()),
            name: Some("get_post_content".into()),
            arguments: Some(format!(r#"{{"document_id":"{post}"}}"#)),
        }),
        usage(50, 5),
    ]);
    h.answer("目前阅读还不多，欢迎大家来看。");
    assert_eq!(h.run(trigger).await, RunOutcome::Completed);
    let requests = h.fake.requests();
    assert_eq!(requests[0].tools.len(), 7);
    let (_, _, body) = h.reply(trigger).await.unwrap();
    // 只用最后一轮（不把「我查一下」发到评论区）。
    assert_eq!(
        body,
        format!("@[user{BOB}]({BOB})\n目前阅读还不多，欢迎大家来看。")
    );
    assert_eq!(h.used(BOB).await, 175);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn vision_models_see_post_and_comment_images(db: PgPool) {
    let h = harness(db).await;
    // 默认模型改成识图（LLM-015）。
    let file: ImportFile = serde_json::from_value(serde_json::json!({
        "models": [
            { "key": "base", "displayName": "基础", "provider": "fake",
              "providerModel": "fake-base", "isDefault": true, "sortOrder": 1, "vision": true }
        ]
    }))
    .unwrap();
    service::import(&h.state, file).await.unwrap();
    for (id, owner) in [(900, FAIRY), (901, BOB)] {
        sqlx::query!(
            "INSERT INTO media_files (id, object_key, mime, size_bytes, sha256, receipt)
             VALUES ($1, $2, 'image/png', 100, $3, $4)",
            id,
            format!("m/{id}.png"),
            "b".repeat(64),
            format!("r{id}"),
        )
        .execute(&h.db)
        .await
        .unwrap();
        sqlx::query!(
            "INSERT INTO media_uploaders (media_id, user_id) VALUES ($1, $2)",
            id,
            owner
        )
        .execute(&h.db)
        .await
        .unwrap();
    }
    let post = ik_content::service::create(
        &h.content,
        &principal(FAIRY, true),
        PostInput {
            title: "看图".into(),
            body: "正文配图：![图](https://cdn.test/body.png)".into(),
            category_id: None,
            tags: Vec::new(),
            is_anonymous: false,
            external_videos: Vec::new(),
            editor_state: None,
            cover_ids: vec![Id::new(900)],
        },
        true,
    )
    .await
    .unwrap()
    .id
    .get();
    let trigger = ik_comments::service::create(
        &h.state.comments,
        &principal(BOB, false),
        post,
        CreateCommentRequest {
            body: "这是什么".into(),
            image_ids: vec![Id::new(901)],
            reply_to_id: None,
            is_anonymous: false,
        },
    )
    .await
    .unwrap()
    .id
    .get();
    h.fake.push(vec![content("是一张图。"), usage(3000, 10)]);
    assert_eq!(h.run(trigger).await, RunOutcome::Completed);
    let request = h.fake.requests().pop().unwrap();
    assert_eq!(
        request.messages.last().unwrap(),
        &ChatMessage::user_with_images(
            format!(
                "当前帖子：看图（document_id: {post}）\n帖子图片：[图片×2]\nuser102 说：这是什么 [图片]"
            ),
            vec![
                "https://cdn.test/m/900.png".into(),
                "https://cdn.test/body.png".into(),
                "https://cdn.test/m/901.png".into(),
            ],
        )
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn comments_only_use_public_memories(db: PgPool) {
    let h = harness(db).await;
    for (id, content, sensitivity) in [
        (1, "对方最喜欢的角色是艾莲", "public"),
        (2, "对方最近在准备考研", "private"),
    ] {
        sqlx::query!(
            "INSERT INTO ai_memories (id, user_id, kind, content, origin, sensitivity)
             VALUES ($1, $2, 'profile', $3, 'extracted', $4)",
            id,
            BOB,
            content,
            sensitivity,
        )
        .execute(&h.db)
        .await
        .unwrap();
    }
    let post = h.post(ALICE, false).await;
    let trigger = h.comment(BOB, post, None, "@user200 你觉得呢").await;
    h.answer("我觉得挺好。");
    assert_eq!(h.run(trigger).await, RunOutcome::Completed);
    // 评论区是公开的：只带公开的记忆（MEM-061），也不提供记忆工具。
    let request = h.fake.requests().pop().unwrap();
    let system = request.messages.first().unwrap().text().to_owned();
    assert!(system.contains("这里是公开的评论区"), "{system}");
    assert!(system.contains("- 对方最喜欢的角色是艾莲"), "{system}");
    assert!(!system.contains("考研"), "{system}");
    assert_eq!(request.tools.len(), 7);
}
