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
    let comments = ik_comments::CommentsState::new(db.clone(), ids, bus.clone(), media);
    let fake = Arc::new(Scripted::default());
    let providers = Providers::default().with("fake", fake.clone());
    let state = AiState::new(messaging, comments, config, providers);
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
              "providerModel": "x", "sortOrder": 3 }
        ],
        "characters": [
            { "slug": "fairy", "displayName": "Fairy", "username": format!("user{FAIRY}"),
              "enabled": true, "systemPrompt": "你是 Fairy。", "allowedModelKeys": ["base", "pro", "lost"] }
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
        sessions::create(
            &self.state,
            &self.me,
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
        let (_, view) = ik_messaging::service::send(
            &self.state.messaging,
            &self.me,
            conversation_id,
            SendMessageRequest {
                content: text.into(),
                reply_to_id: None,
                client_id: None,
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
        dm::run_job(&h.state, job, 1, false).await.unwrap(),
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
    assert_eq!(kinds.last(), Some(&"dm.message.edited"), "{kinds:?}");

    // 请求：system + 带昵称的用户消息。
    let request = &h.fake.requests()[0];
    assert_eq!(request.model, "fake-base");
    assert!(
        matches!(&request.messages[0], ChatMessage::System { content } if content.starts_with("你是 Fairy。"))
    );
    assert_eq!(request.messages[1], ChatMessage::user("user100 说：你好"));

    // 同一任务重放：已有结果，不再调用上游（TRIG-037）。
    assert_eq!(
        dm::run_job(&h.state, job, 2, false).await.unwrap(),
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
    dm::run_job(&h.state, h.job_for_trigger(first).await, 1, false)
        .await
        .unwrap();

    let second = h.send(conversation, "第二句").await;
    h.fake.push(vec![content("收到第二句"), usage(10, 5)]);
    dm::run_job(&h.state, h.job_for_trigger(second).await, 1, false)
        .await
        .unwrap();
    let messages = &h.fake.requests()[1].messages;
    assert_eq!(
        &messages[1..],
        &[
            ChatMessage::user("user100 说：第一句"),
            ChatMessage::assistant("收到第一句"),
            ChatMessage::user("user100 说：第二句"),
        ]
    );

    // 重置：分界消息不删历史，AI 只读分界之后的（CTX-021）。
    let marker = sessions::reset(&h.state, &h.me, conversation)
        .await
        .unwrap();
    assert_eq!(marker.kind, "system");
    let third = h.send(conversation, "第三句").await;
    h.fake.push(vec![content("收到第三句"), usage(10, 5)]);
    dm::run_job(&h.state, h.job_for_trigger(third).await, 1, false)
        .await
        .unwrap();
    let messages = &h.fake.requests()[2].messages;
    assert_eq!(&messages[1..], &[ChatMessage::user("user100 说：第三句")]);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stop_keeps_partial_answer(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session().await;
    let trigger = h.send(conversation, "讲个长故事").await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![content("从前有座山，"), Step::Hang]);
    let state = h.state.clone();
    let running = tokio::spawn(async move { dm::run_job(&state, job, 1, false).await });
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
        dm::run_job(&h.state, job, 1, false).await.unwrap(),
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

    h.fake
        .push(vec![Step::Fail(LlmError::Upstream("boom".into()))]);
    let outcome = dm::run_job(&h.state, job, 1, false).await.unwrap();
    assert!(matches!(outcome, RunOutcome::Retry(_)), "{outcome:?}");
    let (first_reply, text, state, _) = h.reply(job).await;
    assert_eq!(text, "（生成失败，请重试）");
    assert_eq!(state.as_deref(), Some("failed"));

    h.fake.push(vec![content("在的"), usage(30, 3)]);
    assert_eq!(
        dm::run_job(&h.state, job, 2, false).await.unwrap(),
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
    let outcome = dm::run_job(&h.state, job, 3, true).await.unwrap();
    assert!(matches!(outcome, RunOutcome::Failed(_)), "{outcome:?}");
    let status = sqlx::query_scalar!("SELECT status FROM ai_jobs WHERE id = $1", job)
        .fetch_one(&h.db)
        .await
        .unwrap();
    assert_eq!(status, "failed");
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
        dm::run_job(&h.state, job, 1, false).await.unwrap(),
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
    dm::run_job(&h.state, h.job_for_trigger(trigger).await, 1, false)
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
        dm::run_job(&h.state, h.job_for_trigger(trigger).await, 1, false)
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
    dm::run_job(&h.state, job, 1, false).await.unwrap();
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
        dm::run_job(&h.state, new_job, 1, false).await.unwrap(),
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
    dm::run_job(&h.state, h.job_for_trigger(later).await, 1, false)
        .await
        .unwrap();
    let problem = sessions::regenerate(&h.state, &h.me, new_reply, RegenerateRequest::default())
        .await
        .unwrap_err();
    assert_eq!(problem.code(), "AI_NOT_LATEST_REPLY");
}
