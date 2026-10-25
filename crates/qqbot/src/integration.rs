//! Exercise ingestion, quota/accounting and delivery together with local fake providers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::{QqState, jobs, repo, webhook::Incoming};
use axum::{Json, Router, extract::State, routing::post};
use chrono::{Duration, Utc};
use ik_ai::{
    AiState,
    llm::{ChatProvider, ChatRequest, EventStream, LlmError, Providers, StreamEvent, Usage},
};
use ik_kernel::{
    Snowflake,
    bus::LocalBus,
    config::{Ai, Qqbot, QqbotApp, Secret},
    jobs::Jobs,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");
#[derive(Default)]
struct FakeAi {
    calls: AtomicUsize,
    fail: AtomicBool,
}
#[async_trait::async_trait]
impl ChatProvider for FakeAi {
    async fn stream(&self, _: &ChatRequest) -> Result<EventStream, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(LlmError::Upstream("test failure".into()));
        }
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(StreamEvent::Content("你好，今天去六分街逛逛吧。".into())),
            Ok(StreamEvent::Usage(Usage {
                prompt_tokens: 10,
                completion_tokens: 10,
            })),
        ])))
    }
}
#[derive(Default)]
struct FakeQq {
    sent: Mutex<Vec<Value>>,
    fail: AtomicBool,
    transient: AtomicBool,
    attempts: Mutex<Vec<Value>>,
}
async fn send(
    State(fake): State<Arc<FakeQq>>,
    Json(body): Json<Value>,
) -> (http::StatusCode, Json<Value>) {
    fake.attempts.lock().unwrap().push(body.clone());
    if fake.transient.swap(false, Ordering::SeqCst) {
        return (
            http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"code":500})),
        );
    }
    if fake.fail.load(Ordering::SeqCst) {
        return (http::StatusCode::BAD_REQUEST, Json(json!({"code":304023})));
    }
    fake.sent.lock().unwrap().push(body);
    (http::StatusCode::OK, Json(json!({"id":"out"})))
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn transient_send_retry_reuses_output_sequence_and_charge(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    qq.transient.store(true, Ordering::SeqCst);
    let turn = enqueue(&s, "retry", "alice", None, "你好", Duration::zero())
        .await
        .unwrap();
    assert!(jobs::run_reply(&s, turn).await.is_err());
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    jobs::run_reply(&s, turn).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(qq.sent.lock().unwrap().len(), 1);
    let attempts = qq.attempts.lock().unwrap().clone();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["msg_id"], attempts[1]["msg_id"]);
    assert_eq!(attempts[0]["msg_seq"], attempts[1]["msg_seq"]);
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        20
    );
    assert_eq!(
        count(&s, "SELECT sum(turns)::bigint FROM qq_guest_usage").await,
        1
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_messages WHERE direction='out'").await,
        1
    );
    server.abort();
}
async fn setup(
    db: PgPool,
) -> (
    QqState,
    Arc<FakeAi>,
    Arc<FakeQq>,
    tokio::task::JoinHandle<()>,
) {
    let fake = Arc::new(FakeQq::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new()
        .route(
            "/token",
            post(|| async { Json(json!({"access_token":"test-token","expires_in":7200})) }),
        )
        .route("/v2/groups/{openid}/messages", post(send))
        .route("/v2/users/{openid}/messages", post(send))
        .with_state(fake.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let ids = Arc::new(Snowflake::new(5).unwrap());
    let mut ai = AiState::for_tests(
        db.clone(),
        ids.clone(),
        Arc::new(LocalBus::new(16)),
        Ai::default(),
    )
    .unwrap();
    let llm = Arc::new(FakeAi::default());
    ai.providers = Providers::default().with("fake", llm.clone());
    ai.catalog = ik_ai::catalog::CatalogCache::new(db.clone(), ai.providers.clone());
    sqlx::query("INSERT INTO users(id,username,email,is_ai_agent) VALUES(100,'qq_fairy','qq@example.com',TRUE)").execute(&db).await.unwrap();
    ik_ai::service::import(&ai,serde_json::from_value(json!({
        "models":[{"key":"fake","displayName":"测试","provider":"fake","providerModel":"fake","isDefault":true}],
        "characters":[{"slug":"fairy","username":"qq_fairy","displayName":"Fairy","systemPrompt":"你是 Fairy。","enabled":true,"qqEnabled":true}]
    })).unwrap()).await.unwrap();
    let config = Qqbot {
        enabled: true,
        apps: std::collections::BTreeMap::from([(
            "test".into(),
            QqbotApp {
                app_id: "123".into(),
                secret: Secret::new("abc"),
            },
        )]),
        token_url: format!("{url}/token"),
        api_base: Some(url),
        ..Default::default()
    };
    let jobs = Jobs::new(db.clone());
    jobs.migrate().await.unwrap();
    let state = QqState::new(db.clone(), ids, jobs, ai, config).unwrap();
    state.initialize().await.unwrap();
    sqlx::query("UPDATE qq_bots SET enabled=TRUE,default_card_slug='fairy'")
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("UPDATE qq_settings SET enabled=TRUE")
        .execute(&db)
        .await
        .unwrap();
    (state, llm, fake, server)
}
fn message(id: &str, openid: &str, group: Option<&str>, content: &str, age: Duration) -> Incoming {
    serde_json::from_value(json!({"id":id,"content":content,"timestamp":Utc::now()-age,"group_openid":group,
        "author":{"user_openid":openid,"member_openid":openid,"username":"【Alice】"},"mentions":[]})).unwrap()
}
async fn enqueue(
    s: &QqState,
    id: &str,
    openid: &str,
    group: Option<&str>,
    content: &str,
    age: Duration,
) -> Option<i64> {
    repo::ingest(
        s,
        "123",
        if group.is_some() {
            "GROUP_MESSAGE_CREATE"
        } else {
            "C2C_MESSAGE_CREATE"
        },
        message(id, openid, group, content, age),
    )
    .await
    .unwrap();
    sqlx::query_scalar::<_, i64>(
        "SELECT t.id FROM qq_turns t JOIN qq_messages m ON m.id=t.message_id WHERE m.qq_msg_id=$1",
    )
    .bind(id)
    .fetch_optional(&s.db)
    .await
    .unwrap()
}
async fn count(s: &QqState, sql: &'static str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .fetch_one(&s.db)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_tenth_turn_succeeds_eleventh_notifies_once_and_events_deduplicate(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    assert!(
        enqueue(
            &s,
            "ambient",
            "other",
            Some("group"),
            "今天大家打本吗",
            Duration::zero()
        )
        .await
        .is_none()
    );
    for i in 0..12 {
        let id = format!("in{i}");
        let turn = enqueue(
            &s,
            &id,
            "alice",
            Some("group"),
            "<@123> 你好",
            Duration::zero(),
        )
        .await
        .unwrap();
        if i == 0 {
            assert_eq!(
                enqueue(
                    &s,
                    &id,
                    "alice",
                    Some("group"),
                    "<@123> 你好",
                    Duration::zero()
                )
                .await,
                Some(turn)
            );
        }
        jobs::run_reply(&s, turn).await.unwrap();
        jobs::run_reply(&s, turn).await.unwrap();
    }
    assert_eq!(llm.calls.load(Ordering::SeqCst), 10);
    assert_eq!(qq.sent.lock().unwrap().len(), 11);
    assert_eq!(
        count(&s, "SELECT sum(turns)::bigint FROM qq_guest_usage").await,
        10
    );
    assert_eq!(
        count(&s, "SELECT sum(used)::bigint FROM qq_guest_tokens").await,
        200
    );
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        200
    );
    assert_eq!(count(&s, "SELECT count(*) FROM qq_turns").await, 12);
    assert!(
        qq.sent
            .lock()
            .unwrap()
            .iter()
            .all(|body| body["msg_seq"] == 1)
    );
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failure_refunds_turns_but_retains_spend_and_expired_messages_never_generate(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    qq.fail.store(true, Ordering::SeqCst);
    let id = enqueue(&s, "sendfail", "alice", None, "你好", Duration::zero())
        .await
        .unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(
        count(&s, "SELECT sum(turns)::bigint FROM qq_guest_usage").await,
        0
    );
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        20
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_messages WHERE direction='out'").await,
        0
    );
    qq.fail.store(false, Ordering::SeqCst);
    let expired = enqueue(
        &s,
        "old",
        "alice",
        Some("group"),
        "<@123> 你好",
        Duration::minutes(4),
    )
    .await
    .unwrap();
    jobs::run_reply(&s, expired).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert!(qq.sent.lock().unwrap().is_empty());
    llm.fail.store(true, Ordering::SeqCst);
    let failed = enqueue(&s, "modelfail", "alice", None, "你好", Duration::zero())
        .await
        .unwrap();
    jobs::run_reply(&s, failed).await.unwrap();
    assert_eq!(
        count(&s, "SELECT sum(turns)::bigint FROM qq_guest_usage").await,
        0
    );
    assert_eq!(count(&s, "SELECT count(*) FROM ai_quota_holds").await, 0);
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn independent_guest_guards_and_context_privacy(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    sqlx::query("UPDATE qq_settings SET guest_group_daily_turns=1")
        .execute(&s.db)
        .await
        .unwrap();
    s.invalidate().await;
    let first = enqueue(
        &s,
        "a1",
        "alice",
        Some("group"),
        "<@123> 你好",
        Duration::zero(),
    )
    .await
    .unwrap();
    jobs::run_reply(&s, first).await.unwrap();
    let second = enqueue(
        &s,
        "b1",
        "bob",
        Some("group"),
        "<@123> 你好",
        Duration::zero(),
    )
    .await
    .unwrap();
    jobs::run_reply(&s, second).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    sqlx::query("UPDATE qq_settings SET guest_daily_tokens=20")
        .execute(&s.db)
        .await
        .unwrap();
    s.invalidate().await;
    let third = enqueue(&s, "c1", "charlie", None, "你好", Duration::zero())
        .await
        .unwrap();
    jobs::run_reply(&s, third).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    // Different groups and C2C are separate identities even if the upstream openid string coincides.
    enqueue(
        &s,
        "other-group",
        "alice",
        Some("other"),
        "普通群话",
        Duration::zero(),
    )
    .await;
    enqueue(&s, "c2c", "alice", None, "你好", Duration::zero()).await;
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_identities WHERE openid='alice'"
        )
        .await,
        3
    );
    assert_eq!(qq.sent.lock().unwrap().len(), 3);
    // Lifecycle retries cannot replay an old ADD over a subsequent DEL.
    let event = json!({"group_openid":"group"});
    repo::lifecycle(&s, "123", "GROUP_ADD_ROBOT", &event, "add-receipt")
        .await
        .unwrap();
    repo::lifecycle(&s, "123", "GROUP_DEL_ROBOT", &event, "del-receipt")
        .await
        .unwrap();
    repo::lifecycle(&s, "123", "GROUP_ADD_ROBOT", &event, "add-receipt")
        .await
        .unwrap();
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_groups WHERE group_openid='group' AND left_at IS NOT NULL"
        )
        .await,
        1
    );
    server.abort();
}
