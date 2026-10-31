//! AI 私聊：会话、触发、流式生成、停止、重置、重新生成、额度提示、首帧超时退回默认模型。
//! 上游用脚本化的假提供方（实现 `ChatProvider`），不连网络。

// clippy.toml 的测试豁免只覆盖 #[test] 函数本身，不覆盖集成测试里的辅助函数。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use ik_ai::AiState;
use ik_ai::dm::{self, RunOutcome};
use ik_ai::dto::{CreateSessionRequest, ImportFile, RegenerateRequest, UpdateSessionRequest};
use ik_ai::llm::{ChatMessage, LlmError, Providers, StreamEvent};
use ik_ai::quota;
use ik_ai::{service, sessions};
use ik_auth::Principal;
use ik_kernel::bus::{Event, LocalBus};
use ik_kernel::config::Ai;
use ik_kernel::{Id, Role, Snowflake};
use ik_messaging::MessagingState;
use ik_messaging::dto::{OpenDirectRequest, SendMessageRequest};
use sqlx::PgPool;

mod common;
use common::{Scripted, Step, content, usage};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

const HUMAN: i64 = 100;
const FAIRY: i64 = 200;

struct Harness {
    db: PgPool,
    state: AiState,
    fake: Arc<Scripted>,
    bus: Arc<LocalBus>,
    me: Principal,
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

async fn harness_with(db: PgPool, config: Ai, per_user: i64) -> Harness {
    ik_kernel::jobs::Jobs::new(db.clone())
        .migrate()
        .await
        .unwrap();
    let ids = Arc::new(Snowflake::new(1).unwrap());
    let bus = Arc::new(LocalBus::new(1024));
    let media = ik_media::MediaState::for_tests(db.clone(), ids.clone());
    let messaging = MessagingState::new(db.clone(), ids.clone(), bus.clone(), media.clone());
    let forum = ik_ai::Forum::for_tests(db.clone(), ids, bus.clone(), media).unwrap();
    let fake = Arc::new(Scripted::default());
    let providers = Providers::default().with("fake", fake.clone());
    let state = AiState::new(messaging, forum, config, providers);
    assert!(state.install_hooks());
    insert_user(&db, HUMAN, false).await;
    insert_user(&db, FAIRY, true).await;
    let file: ImportFile = serde_json::from_value(serde_json::json!({
        "settings": { "globalEnabled": true, "dailyTokensPerUser": per_user, "dailyTokens": 0 },
        "models": [
            { "key": "base", "displayName": "基础", "provider": "fake",
              "providerModel": "fake-base", "isDefault": true, "sortOrder": 1 },
            { "key": "pro", "displayName": "专业", "provider": "fake",
              "providerModel": "fake-pro", "costWeight": 2, "sortOrder": 2 },
            { "key": "lost", "displayName": "没配提供方", "provider": "nowhere",
              "providerModel": "x", "sortOrder": 3 },
            { "key": "eye", "displayName": "识图", "provider": "fake",
              "providerModel": "fake-eye", "vision": true, "sortOrder": 4 }
        ],
        "characters": [
            { "slug": "fairy", "displayName": "Fairy", "username": format!("user{FAIRY}"),
              "enabled": true, "systemPrompt": "你是 Fairy。", "allowedModelKeys": ["base", "pro", "lost", "eye"] }
        ]
    }))
    .unwrap();
    service::import(&state, file).await.unwrap();
    Harness {
        db,
        state,
        fake,
        bus,
        me: principal(HUMAN, false),
    }
}

async fn harness(db: PgPool) -> Harness {
    harness_with(db, Ai::default(), 1_000_000).await
}

impl Harness {
    async fn session(&self) -> i64 {
        self.session_as(&self.me).await
    }

    async fn session_as(&self, me: &Principal) -> i64 {
        sessions::create(
            &self.state,
            me,
            CreateSessionRequest {
                character: "fairy".into(),
                title: Some("  第一次  ".into()),
            },
        )
        .await
        .unwrap()
        .id
        .get()
    }

    async fn send(&self, conversation_id: i64, text: &str) -> i64 {
        self.send_as(&self.me, conversation_id, text).await
    }

    async fn send_as(&self, me: &Principal, conversation_id: i64, text: &str) -> i64 {
        let (_, view) = ik_messaging::service::send(
            &self.state.messaging,
            me,
            conversation_id,
            SendMessageRequest {
                content: text.into(),
                reply_to_id: None,
                client_id: None,
                image_ids: Vec::new(),
            },
        )
        .await
        .unwrap();
        view.id.get()
    }

    async fn job_for(&self, key: &str) -> i64 {
        sqlx::query_scalar!("SELECT id FROM ai_jobs WHERE execution_key = $1", key)
            .fetch_one(&self.db)
            .await
            .unwrap()
    }

    async fn job_for_trigger(&self, trigger: i64) -> i64 {
        self.job_for(&format!("dm:{trigger}")).await
    }

    async fn reply(&self, job_id: i64) -> (i64, String, Option<String>, Option<String>) {
        let row = sqlx::query!(
            "SELECT m.id, m.content, m.ai_state, m.ai_model_key FROM ai_jobs j
             JOIN messages m ON m.id = j.reply_message_id WHERE j.id = $1",
            job_id
        )
        .fetch_one(&self.db)
        .await
        .unwrap();
        (row.id, row.content, row.ai_state, row.ai_model_key)
    }

    async fn used(&self) -> i64 {
        let mut conn = self.db.acquire().await.unwrap();
        quota::read(&mut conn, HUMAN, Utc::now())
            .await
            .unwrap()
            .used
    }

    fn drain(&self, rx: &mut tokio::sync::broadcast::Receiver<Event>) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn message_triggers_a_streamed_reply(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session().await;
    let view = sessions::get(&h.state, &h.me, conversation).await.unwrap();
    assert_eq!(view.title.as_deref(), Some("第一次"));
    assert_eq!(view.character.as_deref(), Some("fairy"));

    let trigger = h.send(conversation, "你好").await;
    let job = h.job_for_trigger(trigger).await;
    // 任务与消息同一事务入队（TRIG-031）。
    let queued: i64 = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM graphile_worker.jobs WHERE key = $1"#,
        format!("ai:{job}")
    )
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_eq!(queued, 1);

    h.fake.push(vec![
        Step::Emit(StreamEvent::Reasoning("想想".into())),
        content("**你好**，"),
        content("我是 Fairy，有什么可以帮你？"),
        usage(100, 20),
    ]);
    let mut rx = h.bus.subscribe();
    assert_eq!(
        dm::run_job(&h.state, job, false).await.unwrap(),
        RunOutcome::Completed
    );
    let (_, text, state, model) = h.reply(job).await;
    assert_eq!(
        text, "你好，我是 Fairy，有什么可以帮你？",
        "plain 模式去掉 Markdown"
    );
    assert_eq!(state.as_deref(), Some("done"));
    assert_eq!(model.as_deref(), Some("base"));
    // 用量按上游报告 × 倍率结算（预占多出的部分退回）。
    assert_eq!(h.used().await, 120);

    let kinds: Vec<&str> = h.drain(&mut rx).iter().map(|e| e.kind).collect();
    assert_eq!(kinds.first(), Some(&"dm.message.created"), "{kinds:?}");
    assert!(kinds.contains(&"dm.message.delta"), "{kinds:?}");
    assert!(kinds.contains(&"dm.message.append"), "{kinds:?}");
    assert_eq!(kinds.last(), Some(&"dm.message.edited"), "{kinds:?}");

    // 请求：system + 带昵称的用户消息。
    let request = &h.fake.requests()[0];
    assert_eq!(request.model, "fake-base");
    assert!(
        matches!(&request.messages[0], ChatMessage::System { content } if content.starts_with("你是 Fairy。"))
    );
    assert_eq!(request.messages[1], ChatMessage::user("你好"));

    // 同一任务重放：已有结果，不再调用上游（TRIG-037）。
    assert_eq!(
        dm::run_job(&h.state, job, false).await.unwrap(),
        RunOutcome::AlreadyDone
    );
    assert_eq!(h.fake.requests().len(), 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn history_carries_prior_turns_until_reset(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session().await;
    let first = h.send(conversation, "第一句").await;
    h.fake.push(vec![content("收到第一句"), usage(10, 5)]);
    dm::run_job(&h.state, h.job_for_trigger(first).await, false)
        .await
        .unwrap();

    let second = h.send(conversation, "第二句").await;
    h.fake.push(vec![content("收到第二句"), usage(10, 5)]);
    dm::run_job(&h.state, h.job_for_trigger(second).await, false)
        .await
        .unwrap();
    let messages = &h.fake.requests()[1].messages;
    assert_eq!(
        &messages[1..],
        &[
            ChatMessage::user("第一句"),
            ChatMessage::assistant("收到第一句"),
            ChatMessage::user("第二句"),
        ]
    );

    // 重置：分界消息不删历史，AI 只读分界之后的（CTX-021）。
    let marker = sessions::reset(&h.state, &h.me, conversation)
        .await
        .unwrap();
    assert_eq!(marker.kind, "system");
    let third = h.send(conversation, "第三句").await;
    h.fake.push(vec![content("收到第三句"), usage(10, 5)]);
    dm::run_job(&h.state, h.job_for_trigger(third).await, false)
        .await
        .unwrap();
    let messages = &h.fake.requests()[2].messages;
    assert_eq!(&messages[1..], &[ChatMessage::user("第三句")]);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn popular_questions_count_people_not_messages(db: PgPool) {
    let h = harness(db).await;
    insert_user(&h.db, 101, false).await;
    insert_user(&h.db, 102, false).await;
    let popular = |h: &Harness| {
        let state = h.state.clone();
        async move {
            state.popular.invalidate().await;
            let list = service::characters(&state).await.unwrap();
            list.items[0].popular_questions.clone()
        }
    };
    // 两个人问过：还不够展示。同一个人换个会话、换个写法再问不重复计数，追问不算。
    let me = h.me.clone();
    let first = h.session().await;
    let opener = h.send(first, "最近有什么热帖？").await;
    h.send(first, "那第二个呢？").await;
    let again = h.session().await;
    h.send(again, "最近有什么 热帖").await;
    let other = principal(101, false);
    let conversation = h.session_as(&other).await;
    h.send_as(&other, conversation, "最近有什么热帖").await;
    h.send_as(&other, conversation, "新角色是谁？").await;
    assert!(popular(&h).await.is_empty());

    let third = principal(102, false);
    let conversation = h.session_as(&third).await;
    h.send_as(&third, conversation, "最近有什么热帖").await;
    assert_eq!(popular(&h).await, ["最近有什么热帖"]);

    // 重置之后的第一句重新算开场。
    sessions::reset(&h.state, &third, conversation)
        .await
        .unwrap();
    h.send_as(&third, conversation, "新角色是谁").await;
    h.send_as(&me, first, "新角色是谁").await;
    assert_eq!(popular(&h).await, ["最近有什么热帖"]);

    // 撤回开场的那条后不再计入（第二个会话里的那次已把记录换成了它，先撤回那一条）。
    let latest: i64 = sqlx::query_scalar(
        "SELECT message_id FROM ai_question_asks WHERE user_id = $1 AND question_key = '最近有什么热帖'",
    )
    .bind(HUMAN)
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_ne!(latest, opener);
    ik_messaging::service::withdraw(&h.state.messaging, &me, latest)
        .await
        .unwrap();
    assert!(popular(&h).await.is_empty());

    // 超过 30 天的清理掉。
    let pruned = ik_ai::popular::prune(&h.db, Utc::now() + chrono::Duration::days(31))
        .await
        .unwrap();
    assert!(pruned > 0);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_question_asks")
        .fetch_one(&h.db)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stop_keeps_partial_answer(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session().await;
    let trigger = h.send(conversation, "讲个长故事").await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![content("从前有座山，"), Step::Hang]);
    let state = h.state.clone();
    let running = tokio::spawn(async move { dm::run_job(&state, job, false).await });
    // 等占位出现后停止。
    let reply = loop {
        let row = sqlx::query_scalar!("SELECT reply_message_id FROM ai_jobs WHERE id = $1", job)
            .fetch_one(&h.db)
            .await
            .unwrap();
        if let Some(reply) = row {
            break reply;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    sessions::stop(&h.state, &h.me, reply).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(outcome, RunOutcome::Stopped);
    let (_, text, state, _) = h.reply(job).await;
    assert_eq!(text, "从前有座山，\n\n（已停止）");
    assert_eq!(state.as_deref(), Some("stopped"));
    // 调用已开始、上游没报用量：按估算计费（D-083 方案 C），不是整份预占。
    let used = h.used().await;
    assert!(used > 0 && used < 6000, "{used}");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn withdrawn_trigger_cancels_before_generation(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session().await;
    let trigger = h.send(conversation, "算了").await;
    ik_messaging::service::withdraw(&h.state.messaging, &h.me, trigger)
        .await
        .unwrap();
    let job = h.job_for_trigger(trigger).await;
    assert_eq!(
        dm::run_job(&h.state, job, false).await.unwrap(),
        RunOutcome::Skipped("cancelled")
    );
    assert!(h.fake.requests().is_empty());
    assert_eq!(h.used().await, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failures_retry_on_the_same_placeholder(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session().await;
    let trigger = h.send(conversation, "在吗").await;
    let job = h.job_for_trigger(trigger).await;

    // 上游拒绝（4xx）不在轮内重发，整个任务稍后重试。
    h.fake.push(vec![Step::Fail(LlmError::Rejected(
        "400 bad request".into(),
    ))]);
    let outcome = dm::run_job(&h.state, job, false).await.unwrap();
    assert!(matches!(outcome, RunOutcome::Retry(_)), "{outcome:?}");
    let (first_reply, text, state, _) = h.reply(job).await;
    assert_eq!(text, "（生成失败，稍后自动重试）");
    assert_eq!(state.as_deref(), Some("failed"));

    h.fake.push(vec![content("在的"), usage(30, 3)]);
    assert_eq!(
        dm::run_job(&h.state, job, false).await.unwrap(),
        RunOutcome::Completed
    );
    let (reply, text, state, _) = h.reply(job).await;
    assert_eq!(reply, first_reply, "重试复用同一条占位（GEN-002）");
    assert_eq!(text, "在的");
    assert_eq!(state.as_deref(), Some("done"));

    // 空回复按失败；最后一次尝试记为最终失败（TRIG-034）。
    let again = h.send(conversation, "再来").await;
    let job = h.job_for_trigger(again).await;
    h.fake.push(vec![content("<br>"), usage(1, 1)]);
    let outcome = dm::run_job(&h.state, job, true).await.unwrap();
    assert!(matches!(outcome, RunOutcome::Failed(_)), "{outcome:?}");
    let status = sqlx::query_scalar!("SELECT status FROM ai_jobs WHERE id = $1", job)
        .fetch_one(&h.db)
        .await
        .unwrap();
    assert_eq!(status, "failed");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_newer_message_ends_a_pending_retry(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session().await;
    let first = h.send(conversation, "在吗").await;
    let job = h.job_for_trigger(first).await;
    h.fake.push(vec![Step::Fail(LlmError::Rejected(
        "400 bad request".into(),
    ))]);
    let outcome = dm::run_job(&h.state, job, false).await.unwrap();
    assert!(matches!(outcome, RunOutcome::Retry(_)), "{outcome:?}");

    // 等重试的时候对方又发了一条：交给新的那条一起回，旧的不再隔很久补一条（GEN-027）。
    h.send(conversation, "人呢").await;
    let calls = h.fake.requests().len();
    assert_eq!(
        dm::run_job(&h.state, job, false).await.unwrap(),
        RunOutcome::Skipped("merged")
    );
    assert_eq!(h.fake.requests().len(), calls);
    let (_, text, state, _) = h.reply(job).await;
    assert_eq!(text, "（已停止）");
    assert_eq!(state.as_deref(), Some("stopped"));
    let attempts: i16 = sqlx::query_scalar("SELECT attempts FROM ai_jobs WHERE id = $1")
        .bind(job)
        .fetch_one(&h.db)
        .await
        .unwrap();
    assert_eq!(attempts, 2, "尝试次数一直往上加");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn first_delta_timeout_falls_back_to_default_model(db: PgPool) {
    let config = Ai {
        first_delta_timeout_secs: 1,
        reply_timeout_secs: 60,
        ..Ai::default()
    };
    let h = harness_with(db, config, 1_000_000).await;
    let conversation = h.session().await;
    sessions::update(
        &h.state,
        &h.me,
        conversation,
        UpdateSessionRequest {
            title: None,
            model_key: Some(Some("pro".into())),
        },
    )
    .await
    .unwrap();
    let trigger = h.send(conversation, "你好").await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![Step::Hang]);
    h.fake.push(vec![content("默认模型来了"), usage(10, 5)]);
    assert_eq!(
        dm::run_job(&h.state, job, false).await.unwrap(),
        RunOutcome::Completed
    );
    let requests = h.fake.requests();
    assert_eq!(requests[0].model, "fake-pro");
    assert_eq!(requests[1].model, "fake-base");
    let (_, text, _, model) = h.reply(job).await;
    assert_eq!(text, "默认模型来了");
    assert_eq!(model.as_deref(), Some("base"));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn model_choice_rules(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session().await;
    let set = |key: Option<&str>| UpdateSessionRequest {
        title: Some(Some("新标题".into())),
        model_key: Some(key.map(str::to_owned)),
    };
    let problem = sessions::update(&h.state, &h.me, conversation, set(Some("nope")))
        .await
        .unwrap_err();
    assert_eq!(problem.code(), "AI_MODEL_UNAVAILABLE");
    let view = sessions::update(&h.state, &h.me, conversation, set(Some("pro")))
        .await
        .unwrap();
    assert_eq!(view.model_key.as_deref(), Some("pro"));
    assert_eq!(view.title.as_deref(), Some("新标题"));
    let view = sessions::update(&h.state, &h.me, conversation, set(Some("")))
        .await
        .unwrap();
    assert_eq!(view.model_key, None);

    // 指向没有配置的提供方的模型：静默退回默认模型（LLM-011）。
    sessions::update(&h.state, &h.me, conversation, set(Some("lost")))
        .await
        .unwrap();
    let trigger = h.send(conversation, "hi").await;
    h.fake.push(vec![content("ok"), usage(1, 1)]);
    dm::run_job(&h.state, h.job_for_trigger(trigger).await, false)
        .await
        .unwrap();
    assert_eq!(h.fake.requests()[0].model, "fake-base");

    // 普通私聊不能开给 AI；AI 会话的设置接口不接受普通私聊。
    let problem = ik_messaging::service::open_direct(
        &h.state.messaging,
        &h.me,
        OpenDirectRequest {
            user_id: Id::new(FAIRY),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(problem.code(), "DM_AI_SESSION_REQUIRED");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn images_reach_vision_models_only(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session().await;
    sqlx::query!(
        "INSERT INTO media_files (id, object_key, mime, size_bytes, sha256, receipt)
         VALUES (900, 'm/900.png', 'image/png', 100, $1, 'r900')",
        "b".repeat(64),
    )
    .execute(&h.db)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO media_uploaders (media_id, user_id) VALUES (900, $1)",
        HUMAN
    )
    .execute(&h.db)
    .await
    .unwrap();
    let set = |key: &str| UpdateSessionRequest {
        title: None,
        model_key: Some(Some(key.to_owned())),
    };

    // 识图模型：只发图（正文为空）也触发，图片作为 image_url 发给上游（MSG-030、LLM-015）。
    sessions::update(&h.state, &h.me, conversation, set("eye"))
        .await
        .unwrap();
    let (_, view) = ik_messaging::service::send(
        &h.state.messaging,
        &h.me,
        conversation,
        SendMessageRequest {
            content: "  ".into(),
            reply_to_id: None,
            client_id: None,
            image_ids: vec![Id::new(900)],
        },
    )
    .await
    .unwrap();
    assert_eq!(view.content.as_deref(), Some(""));
    assert_eq!(view.images.len(), 1);
    h.fake.push(vec![content("是一张图。"), usage(1000, 5)]);
    dm::run_job(&h.state, h.job_for_trigger(view.id.get()).await, false)
        .await
        .unwrap();
    let request = &h.fake.requests()[0];
    assert_eq!(request.model, "fake-eye");
    assert_eq!(
        request.messages[1],
        ChatMessage::user_with_images("[图片]", vec!["https://cdn.test/m/900.png".into()])
    );

    // 换成不识图的模型：历史里的图片只剩占位。
    sessions::update(&h.state, &h.me, conversation, set("base"))
        .await
        .unwrap();
    let trigger = h.send(conversation, "再看看").await;
    h.fake.push(vec![content("我看不到图片。"), usage(10, 5)]);
    dm::run_job(&h.state, h.job_for_trigger(trigger).await, false)
        .await
        .unwrap();
    let request = &h.fake.requests()[1];
    assert_eq!(request.model, "fake-base");
    assert_eq!(request.messages[1], ChatMessage::user("[图片]"));
    assert_eq!(request.messages[3], ChatMessage::user("再看看"));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn quota_denial_posts_a_free_notice(db: PgPool) {
    let h = harness_with(db, Ai::default(), 100).await;
    // 先把今天的个人额度用满。
    let mut tx = h.db.begin().await.unwrap();
    let catalog = h.state.catalog.get().await.unwrap();
    let hold = quota::reserve(
        &mut tx,
        &h.state.ids,
        catalog.limits,
        HUMAN,
        150,
        Utc::now(),
        chrono::Duration::minutes(15),
    )
    .await
    .unwrap()
    .unwrap();
    quota::settle(&mut tx, &hold, "x", 150, Utc::now())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let conversation = h.session().await;
    let trigger = h.send(conversation, "还能聊吗").await;
    assert_eq!(
        dm::run_job(&h.state, h.job_for_trigger(trigger).await, false)
            .await
            .unwrap(),
        RunOutcome::Skipped("daily_token_budget_exceeded")
    );
    assert!(h.fake.requests().is_empty());
    let notice = sqlx::query!(
        "SELECT kind, content, sender_id FROM messages
         WHERE conversation_id = $1 ORDER BY created_at DESC, id DESC LIMIT 1",
        conversation
    )
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_eq!(notice.kind, "system");
    assert_eq!(notice.sender_id, FAIRY);
    assert!(notice.content.contains("04:00"));
    assert_eq!(h.used().await, 150, "提示不计费");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn regenerate_replaces_the_latest_reply_once(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session().await;
    let trigger = h.send(conversation, "写首诗").await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![content("第一版"), usage(5, 5)]);
    dm::run_job(&h.state, job, false).await.unwrap();
    let (old_reply, _, _, _) = h.reply(job).await;

    let request = || RegenerateRequest {
        model_key: Some("pro".into()),
    };
    sessions::regenerate(&h.state, &h.me, old_reply, request())
        .await
        .unwrap();
    // 重复点击：旧回复已撤回，不再受理。
    assert!(
        sessions::regenerate(&h.state, &h.me, old_reply, request())
            .await
            .is_err()
    );
    let withdrawn =
        sqlx::query_scalar!("SELECT withdrawn_at FROM messages WHERE id = $1", old_reply)
            .fetch_one(&h.db)
            .await
            .unwrap();
    assert!(withdrawn.is_some());

    let new_job = h.job_for(&format!("regenerate:{old_reply}")).await;
    h.fake.push(vec![content("第二版"), usage(5, 5)]);
    assert_eq!(
        dm::run_job(&h.state, new_job, false).await.unwrap(),
        RunOutcome::Completed
    );
    let (_, text, _, model) = h.reply(new_job).await;
    assert_eq!(text, "第二版");
    assert_eq!(model.as_deref(), Some("pro"), "单次覆盖的模型");
    assert_eq!(h.fake.requests()[1].model, "fake-pro");
    // 会话级选择不受影响。
    let view = sessions::get(&h.state, &h.me, conversation).await.unwrap();
    assert_eq!(view.model_key, None);

    // 不是最后一条 AI 回复不能重新生成。
    let (new_reply, _, _, _) = h.reply(new_job).await;
    let later = h.send(conversation, "再写一首").await;
    h.fake.push(vec![content("第三首"), usage(5, 5)]);
    dm::run_job(&h.state, h.job_for_trigger(later).await, false)
        .await
        .unwrap();
    let problem = sessions::regenerate(&h.state, &h.me, new_reply, RegenerateRequest::default())
        .await
        .unwrap_err();
    assert_eq!(problem.code(), "AI_NOT_LATEST_REPLY");
}

async fn job_metrics(db: &PgPool, job: i64) -> (i64, i16, i16) {
    let row = sqlx::query!(
        "SELECT cached_tokens, retries, tool_rounds FROM ai_jobs WHERE id = $1",
        job
    )
    .fetch_one(db)
    .await
    .unwrap();
    (row.cached_tokens, row.retries, row.tool_rounds)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_dropped_stream_is_resumed_within_the_round(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session().await;
    let trigger = h.send(conversation, "讲个故事").await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![
        content("从前有座山，"),
        Step::Fail(LlmError::Upstream("connection reset".into())),
    ]);
    h.fake.push(vec![content("山里有座庙。"), usage(80, 10)]);
    assert_eq!(
        dm::run_job(&h.state, job, false).await.unwrap(),
        RunOutcome::Completed
    );
    let (_, text, _, _) = h.reply(job).await;
    assert_eq!(text, "从前有座山，山里有座庙。");
    // 重发时带上已经发出去的部分，让模型接着写（GEN-025）。
    let requests = h.fake.requests();
    assert_eq!(requests.len(), 2);
    let tail = &requests[1].messages[requests[1].messages.len() - 2..];
    assert_eq!(tail[0], ChatMessage::assistant("从前有座山，"));
    assert!(tail[1].text().contains("接着写"), "{:?}", tail[1]);
    assert_eq!(job_metrics(&h.db, job).await.1, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_reply_cut_by_the_output_limit_is_continued_once(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session().await;
    let trigger = h.send(conversation, "写长一点").await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![
        content("第一段，"),
        Step::Emit(StreamEvent::Finish("length".into())),
    ]);
    h.fake.push(vec![
        content("第二段，"),
        Step::Emit(StreamEvent::Finish("length".into())),
    ]);
    assert_eq!(
        dm::run_job(&h.state, job, false).await.unwrap(),
        RunOutcome::Completed
    );
    let (_, text, _, _) = h.reply(job).await;
    assert_eq!(text, "第一段，第二段，");
    // 只续写一次（GEN-026）。
    let requests = h.fake.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .messages
            .last()
            .unwrap()
            .text()
            .contains("单次输出长度"),
        "{:?}",
        requests[1].messages.last()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn burst_messages_are_answered_once(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session().await;
    let first = h.send(conversation, "在吗").await;
    let second = h.send(conversation, "问个事").await;
    let (first_job, second_job) = (
        h.job_for_trigger(first).await,
        h.job_for_trigger(second).await,
    );
    // 同一会话排在同一个队列里（GEN-027）。
    let queues: Vec<Option<String>> = sqlx::query_scalar!(
        "SELECT queue_name FROM graphile_worker.jobs WHERE key = ANY($1) ORDER BY id",
        &[format!("ai:{first_job}"), format!("ai:{second_job}")]
    )
    .fetch_all(&h.db)
    .await
    .unwrap();
    let queue = format!("ai:dm:{conversation}");
    assert_eq!(queues, vec![Some(queue.clone()), Some(queue)]);

    assert_eq!(
        dm::run_job(&h.state, first_job, false).await.unwrap(),
        RunOutcome::Skipped("merged")
    );
    h.fake.push(vec![content("在的，你说。"), usage(50, 5)]);
    assert_eq!(
        dm::run_job(&h.state, second_job, false).await.unwrap(),
        RunOutcome::Completed
    );
    let requests = h.fake.requests();
    assert_eq!(requests.len(), 1);
    let texts: Vec<&str> = requests[0].messages.iter().map(ChatMessage::text).collect();
    assert_eq!(texts[1..], ["在吗", "问个事"]);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn traces_and_cache_hits_are_recorded(db: PgPool) {
    let h = harness(db).await;
    sqlx::query("UPDATE ai_settings SET trace_enabled = TRUE WHERE id = 1")
        .execute(&h.db)
        .await
        .unwrap();
    h.state.catalog.invalidate().await;
    let conversation = h.session().await;
    let trigger = h.send(conversation, "你好").await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![
        content("你好呀"),
        Step::Emit(StreamEvent::Usage(ik_ai::llm::Usage {
            prompt_tokens: 100,
            completion_tokens: 20,
            cached_tokens: 50,
        })),
    ]);
    assert_eq!(
        dm::run_job(&h.state, job, false).await.unwrap(),
        RunOutcome::Completed
    );
    // 命中缓存的 50 个输入 token 按 0.2 计：50 + 10 + 20。
    assert_eq!(h.used().await, 80);
    assert_eq!(job_metrics(&h.db, job).await, (50, 0, 0));
    let trace = sqlx::query!(
        "SELECT attempt, model_key, request, reply, outcome FROM ai_job_traces WHERE job_id = $1",
        job
    )
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_eq!((trace.attempt, trace.model_key.as_str()), (1, "base"));
    assert_eq!(trace.reply, "你好呀");
    assert_eq!(trace.outcome, "completed");
    assert_eq!(trace.request["model"], "fake-base");
    assert_eq!(trace.request["messages"][1]["content"], "你好");
}

fn tool_call(id: &str, name: &str, arguments: &str) -> Step {
    Step::Emit(StreamEvent::ToolCallDelta {
        index: 0,
        id: Some(id.into()),
        name: Some(name.into()),
        arguments: Some(arguments.into()),
    })
}

impl Harness {
    async fn guide_post(&self) -> i64 {
        ik_content::service::create(
            &self.state.forum.content,
            &self.me,
            ik_content::dto::PostInput {
                title: "零号空洞攻略".into(),
                body: "先清怪，再开箱。".into(),
                category_id: None,
                tags: Vec::new(),
                is_anonymous: false,
                external_videos: Vec::new(),
                editor_state: None,
                cover_ids: Vec::new(),
            },
            true,
        )
        .await
        .unwrap()
        .id
        .get()
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn tool_calls_loop_and_the_timeline_is_saved(db: PgPool) {
    let h = harness(db).await;
    let post = h.guide_post().await;
    let conversation = h.session().await;
    let trigger = h.send(conversation, "帮我找找零号空洞的攻略").await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![
        Step::Emit(StreamEvent::Reasoning(
            "用户想找攻略，先在站内搜一下。".into(),
        )),
        content("正在检索相关帖子。"),
        tool_call("call_1", "search_posts", r#"{"query":"零号空洞、攻略"}"#),
        Step::Emit(StreamEvent::Finish("tool_calls".into())),
        usage(100, 10),
    ]);
    h.fake.push(vec![
        content(&format!(
            "检索完毕。\n\n## 结论\n推荐阅读 [零号空洞攻略](/post/{post})。"
        )),
        usage(200, 20),
    ]);
    let mut rx = h.bus.subscribe();
    assert_eq!(
        dm::run_job(&h.state, job, false).await.unwrap(),
        RunOutcome::Completed
    );

    // 第一轮带 7 个论坛工具与 3 个记忆工具（ADR 0007）；第二轮回灌了工具调用与结果（LLM-005）。
    let requests = h.fake.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].tools.len(), 10);
    let second = &requests[1].messages;
    // 思考模式：带工具调用的那一轮把思考原文回传给上游（LLM-013），否则 DeepSeek 返回 400。
    assert!(matches!(
        &second[second.len() - 2],
        ChatMessage::Assistant { tool_calls, reasoning_content, .. }
            if tool_calls[0].name == "search_posts"
                && reasoning_content.as_deref() == Some("用户想找攻略，先在站内搜一下。")
    ));
    let ChatMessage::Tool {
        tool_call_id,
        content: result,
    } = &second[second.len() - 1]
    else {
        panic!("{second:?}");
    };
    assert_eq!(tool_call_id, "call_1");
    assert!(
        result.contains(&format!("[零号空洞攻略](/post/{post})")),
        "{result}"
    );

    // 正文是两轮拼起来的；用量逐轮累加（LLM-006）。
    let (reply, text, _, _) = h.reply(job).await;
    assert_eq!(
        text,
        format!(
            "正在检索相关帖子。\n\n检索完毕。\n\n结论\n推荐阅读 [零号空洞攻略](/post/{post})。"
        )
    );
    assert_eq!(h.used().await, 330);

    // 时间线落库（WF-003），并随消息下发。
    let page = ik_messaging::service::messages(
        &h.state.messaging,
        &h.me,
        conversation,
        ik_messaging::dto::MessagesQuery {
            cursor: None,
            limit: None,
        },
    )
    .await
    .unwrap();
    let shown = page.items.iter().find(|m| m.id.get() == reply).unwrap();
    let timeline = shown.ai.as_ref().unwrap().workflow.as_ref().unwrap();
    assert_eq!(timeline.last().unwrap().kind, "answer.finish");
    let stored: serde_json::Value =
        sqlx::query_scalar!("SELECT ai_workflow FROM messages WHERE id = $1", reply)
            .fetch_one(&h.db)
            .await
            .unwrap()
            .unwrap();
    let events = stored.as_array().unwrap();
    let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(
        types,
        vec![
            "reasoning.start",
            "reasoning.finish",
            "answer.start",
            "answer.delta",
            "forum.search.start",
            "forum.search.finish",
            "answer.delta",
            "answer.delta",
            "citation.add",
            "answer.finish",
        ]
    );
    assert_eq!(events[3]["data"]["text"], "正在检索相关帖子。");
    assert_eq!(events[4]["data"]["query"], "零号空洞、攻略");
    assert_eq!(
        events[5]["data"]["items"][0]["documentId"],
        post.to_string()
    );
    assert_eq!(
        events[6]["data"],
        serde_json::json!({ "partIndex": 1, "text": "检索完毕。" })
    );
    assert_eq!(events[7]["data"]["partIndex"], 2);
    assert_eq!(
        events[8]["data"],
        serde_json::json!({ "documentId": post.to_string(), "title": "零号空洞攻略" })
    );
    assert_eq!(
        events[9]["usage"],
        serde_json::json!({ "promptTokens": 300, "completionTokens": 30 })
    );
    // 思考文本、工具参数与结果不进入时间线。
    assert!(!stored.to_string().contains("用户想找攻略"));
    assert!(!stored.to_string().contains("先清怪"));

    // 实时推送：工作流事件在定稿之前。
    let kinds: Vec<&str> = h.drain(&mut rx).iter().map(|e| e.kind).collect();
    let workflow_events = kinds
        .iter()
        .filter(|k| **k == "dm.message.workflow")
        .count();
    assert_eq!(workflow_events, events.len());
    assert_eq!(kinds.last(), Some(&"dm.message.edited"));

    // 追问：上一条回复前附上当时的工具记录（CTX-012），模型知道之前读过哪篇。
    let follow = h.send(conversation, "第一篇再详细说说").await;
    let follow_job = h.job_for_trigger(follow).await;
    h.fake.push(vec![content("好的。"), usage(10, 1)]);
    assert_eq!(
        dm::run_job(&h.state, follow_job, false).await.unwrap(),
        RunOutcome::Completed
    );
    let requests = h.fake.requests();
    let history = &requests[2].messages;
    let previous = history
        .iter()
        .find_map(|m| match m {
            ChatMessage::Assistant { content, .. } if content.contains("推荐阅读") => {
                Some(content)
            }
            _ => None,
        })
        .unwrap();
    assert!(
        previous.starts_with(&format!(
            "【工具记录】检索「零号空洞、攻略」1 条：《零号空洞攻略》(/post/{post})"
        )),
        "{previous}"
    );
    assert!(history[0].text().contains("系统自动记下"));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn tools_in_one_round_run_together_and_reply_in_call_order(db: PgPool) {
    let h = harness(db).await;
    let post = h.guide_post().await;
    let conversation = h.session().await;
    let trigger = h.send(conversation, "热门和攻略都看看").await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![
        tool_call("call_1", "get_hot_posts", "{}"),
        Step::Emit(StreamEvent::ToolCallDelta {
            index: 1,
            id: Some("call_2".into()),
            name: Some("get_post_content".into()),
            arguments: Some(format!(r#"{{"post_id":"{post}"}}"#)),
        }),
        usage(10, 1),
    ]);
    h.fake.push(vec![content("看完了。"), usage(10, 1)]);
    assert_eq!(
        dm::run_job(&h.state, job, false).await.unwrap(),
        RunOutcome::Completed
    );
    let requests = h.fake.requests();
    let second = &requests[1].messages;
    let ids: Vec<&str> = second
        .iter()
        .filter_map(|m| match m {
            ChatMessage::Tool { tool_call_id, .. } => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["call_1", "call_2"]);
    assert!(matches!(
        second.last(),
        Some(ChatMessage::Tool { content, .. }) if content.contains("先清怪")
    ));
    // 两个开始事件都在任何一个结束事件之前（并发执行）。
    let (reply, _, _, _) = h.reply(job).await;
    let stored: serde_json::Value =
        sqlx::query_scalar!("SELECT ai_workflow FROM messages WHERE id = $1", reply)
            .fetch_one(&h.db)
            .await
            .unwrap()
            .unwrap();
    let types: Vec<&str> = stored
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    let last_start = types
        .iter()
        .rposition(|t| t.ends_with(".start") && *t != "answer.start")
        .unwrap();
    let first_finish = types
        .iter()
        .position(|t| t.ends_with(".finish") && *t != "answer.finish")
        .unwrap();
    assert!(last_start < first_finish, "{types:?}");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn tool_rounds_are_capped(db: PgPool) {
    let config = Ai {
        max_tool_rounds: 1,
        ..Ai::default()
    };
    let h = harness_with(db, config, 1_000_000).await;
    let conversation = h.session().await;
    let trigger = h.send(conversation, "今天热门是什么").await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![
        tool_call("call_1", "get_hot_posts", "{}"),
        usage(10, 1),
    ]);
    h.fake.push(vec![content("目前没有热帖。"), usage(10, 1)]);
    assert_eq!(
        dm::run_job(&h.state, job, false).await.unwrap(),
        RunOutcome::Completed
    );
    let requests = h.fake.requests();
    // 轮数用完：不再提供工具，并要求基于已有结果作答。
    assert!(requests[1].tools.is_empty());
    assert!(matches!(
        requests[1].messages.last(),
        Some(ChatMessage::User { content }) if content.text.contains("工具调用次数已达上限")
    ));
    assert!(matches!(
        &requests[1].messages[requests[1].messages.len() - 2],
        ChatMessage::Tool { content, .. } if content == "当前没有可推荐的热帖。"
    ));
    // 时间线上留一步「已达上限」。
    let (reply, _, _, _) = h.reply(job).await;
    let stored: serde_json::Value =
        sqlx::query_scalar!("SELECT ai_workflow FROM messages WHERE id = $1", reply)
            .fetch_one(&h.db)
            .await
            .unwrap()
            .unwrap();
    let limit = stored
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "tool.limit")
        .unwrap();
    assert_eq!(limit["data"], serde_json::json!({ "rounds": 1 }));
}
