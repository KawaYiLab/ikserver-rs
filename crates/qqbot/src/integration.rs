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
    /// 每次请求的全部消息正文（按顺序拼起来），用来断言记忆段有没有进提示词。
    prompts: Mutex<Vec<String>>,
}
const PERSONAL_EXTRACT: &str = "你负责维护 AI 角色对用户的长期记忆";
const SHARED_EXTRACT: &str = "你负责维护 AI 角色对一个 QQ 群";
#[async_trait::async_trait]
impl ChatProvider for FakeAi {
    async fn stream(&self, request: &ChatRequest) -> Result<EventStream, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(LlmError::Upstream("test failure".into()));
        }
        let all: Vec<&str> = request.messages.iter().map(|m| m.text()).collect();
        self.prompts.lock().unwrap().push(all.join("\n"));
        let system = all.first().copied().unwrap_or_default();
        let reply = if system.starts_with(PERSONAL_EXTRACT) {
            r#"{"ops":[{"op":"add","kind":"profile","content":"对方喜欢下雨天"}]}"#
        } else if system.starts_with(SHARED_EXTRACT) {
            r#"{"ops":[{"op":"add","content":"这个群周五晚上一起打本"}]}"#
        } else {
            "你好，今天去六分街逛逛吧。"
        };
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(StreamEvent::Content(reply.into())),
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
    let state = QqState::new(db.clone(), ids, jobs, ai, config, b"test-key".to_vec()).unwrap();
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

// ---------- 第二、三期：绑定、个人额度、记忆、群共享记忆 ----------

const SITE_USER: i64 = 200;

async fn site_user(s: &QqState, id: i64, name: &str) {
    sqlx::query("INSERT INTO users(id,username,email) VALUES($1,$2,$3)")
        .bind(id)
        .bind(name)
        .bind(format!("{name}@example.com"))
        .execute(&s.db)
        .await
        .unwrap();
}

async fn bind_code(s: &QqState, user_id: i64, code: &str) {
    sqlx::query("INSERT INTO qq_bind_codes(code_hash,user_id,expires_at) VALUES($1,$2,$3)")
        .bind(crate::me::code_hash(&s.code_key, code))
        .bind(user_id)
        .bind(Utc::now() + Duration::minutes(10))
        .execute(&s.db)
        .await
        .unwrap();
}

fn last_sent(qq: &FakeQq) -> String {
    qq.sent.lock().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn say(s: &QqState, id: &str, openid: &str, group: Option<&str>, content: &str) {
    let turn = enqueue(s, id, openid, group, content, Duration::zero())
        .await
        .unwrap();
    jobs::run_reply(s, turn).await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn binding_switches_to_personal_quota_and_notifies_the_account(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    site_user(&s, SITE_USER, "alice_site").await;
    site_user(&s, 201, "mallory").await;
    bind_code(&s, SITE_USER, "ABCD2345").await;
    // 码不区分大小写，可以带空格。
    say(&s, "bind", "alice", None, "/绑定 abcd 2345").await;
    assert!(last_sent(&qq).contains("绑定成功"));
    assert_eq!(llm.calls.load(Ordering::SeqCst), 0, "指令不调用模型");
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_identities WHERE openid='alice' AND user_id=200"
        )
        .await,
        1
    );
    // 站内收到一条系统通知（万一不是本人操作能及时发现）。
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM notifications WHERE recipient_id=200 AND kind='system'"
        )
        .await,
        1
    );
    // 用过的码不能再用（哪怕被旁观者看到）。
    say(&s, "steal", "mallory_qq", None, "/绑定 ABCD2345").await;
    assert!(last_sent(&qq).contains("无效"));
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_identities WHERE openid='mallory_qq' AND user_id IS NOT NULL"
        )
        .await,
        0
    );
    // 绑定后不受游客次数限制，按个人额度计。
    sqlx::query("UPDATE qq_settings SET guest_daily_turns=0")
        .execute(&s.db)
        .await
        .unwrap();
    s.invalidate().await;
    say(&s, "chat", "alice", None, "你好").await;
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_turns WHERE payer_user_id=200 AND status='completed'"
        )
        .await,
        1
    );
    assert!(
        count(
            &s,
            "SELECT COALESCE(sum(used),0)::bigint FROM ai_quota_counters WHERE user_id=200"
        )
        .await
            > 0
    );
    assert_eq!(
        count(
            &s,
            "SELECT COALESCE(sum(turns),0)::bigint FROM qq_guest_usage"
        )
        .await,
        0,
        "绑定用户不占游客次数"
    );
    assert_eq!(
        count(
            &s,
            "SELECT COALESCE(sum(used),0)::bigint FROM qq_guest_tokens"
        )
        .await,
        0,
        "绑定用户不计入游客 token"
    );
    // 个人额度用完：提示一次，不调用模型。
    sqlx::query("UPDATE ai_settings SET daily_tokens_per_user=1")
        .execute(&s.db)
        .await
        .unwrap();
    s.ai.catalog.invalidate().await;
    say(&s, "chat2", "alice", None, "再聊").await;
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert!(last_sent(&qq).contains("额度已用完"));
    // 解绑后回到游客。
    say(&s, "unbind", "alice", None, "/解绑").await;
    assert!(last_sent(&qq).contains("已解除绑定"));
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_identities WHERE openid='alice' AND user_id IS NOT NULL"
        )
        .await,
        0
    );
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_code_sniffed_from_the_group_cannot_win_the_race(db: PgPool) {
    let (s, _llm, qq, server) = setup(db).await;
    site_user(&s, SITE_USER, "alice_site").await;
    bind_code(&s, SITE_USER, "ABCD2345").await;
    // Alice 在群里先发了码，但她所在的群队列还没处理；旁观者随后在单聊里发同一个码，先被处理。
    let alice = enqueue(
        &s,
        "alice-bind",
        "alice",
        Some("group"),
        "<@123> /绑定 ABCD2345",
        Duration::seconds(5),
    )
    .await
    .unwrap();
    say(&s, "mallory-bind", "mallory", None, "/绑定 abcd-2345").await;
    assert!(last_sent(&qq).contains("作废"));
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_identities WHERE user_id IS NOT NULL"
        )
        .await,
        0,
        "抢先的旁观者绑不上"
    );
    // 码已作废，Alice 也要重新生成（不会被绑到旁观者那边）。
    jobs::run_reply(&s, alice).await.unwrap();
    assert!(last_sent(&qq).contains("无效"));
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_identities WHERE user_id IS NOT NULL"
        )
        .await,
        0
    );
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn wrong_codes_are_limited_and_blocked_accounts_are_not_downgraded(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    site_user(&s, SITE_USER, "alice_site").await;
    for i in 0..10 {
        say(&s, &format!("wrong{i}"), "eve", None, "/绑定 ZZZZ9999").await;
    }
    bind_code(&s, SITE_USER, "WXYZ2345").await;
    say(&s, "right", "eve", None, "/绑定 WXYZ2345").await;
    assert!(
        last_sent(&qq).contains("次数太多"),
        "连错 10 次后当天不能再试"
    );
    // 别人先发过的码作废（防抢先），所以 Alice 用一个新码。
    bind_code(&s, SITE_USER, "WXYZ2346").await;
    // 绑定的账号被封禁：不回复，也不降级成游客。
    say(&s, "bind", "alice", None, "/绑定 WXYZ2346").await;
    assert!(last_sent(&qq).contains("绑定成功"));
    sqlx::query("UPDATE users SET status='blocked' WHERE id=200")
        .execute(&s.db)
        .await
        .unwrap();
    let sent = qq.sent.lock().unwrap().len();
    say(&s, "blocked", "alice", None, "你好").await;
    assert_eq!(qq.sent.lock().unwrap().len(), sent);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
    // 注销的账号：解绑，按游客处理。
    sqlx::query("UPDATE users SET status='deleted' WHERE id=200")
        .execute(&s.db)
        .await
        .unwrap();
    say(&s, "gone", "alice", None, "你好").await;
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_identities WHERE openid='alice' AND user_id IS NOT NULL"
        )
        .await,
        0
    );
    server.abort();
}

async fn memory(s: &QqState, id: i64, content: &str, sensitivity: &str, scope: Option<String>) {
    sqlx::query("INSERT INTO ai_memories(id,user_id,kind,content,origin,sensitivity,source_scope) VALUES($1,200,'profile',$2,'told',$3,$4)")
        .bind(id)
        .bind(content)
        .bind(sensitivity)
        .bind(scope)
        .execute(&s.db)
        .await
        .unwrap();
}

fn last_prompt(llm: &FakeAi) -> String {
    llm.prompts.lock().unwrap().last().unwrap().clone()
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn memories_follow_the_place_and_group_memory_is_shared(db: PgPool) {
    let (s, llm, _qq, server) = setup(db).await;
    site_user(&s, SITE_USER, "alice_site").await;
    // 先在 A 群、B 群各出现一次（建群与身份），再在 A 群和单聊各绑一次。
    say(&s, "a0", "alice", Some("group-a"), "<@123> 在吗").await;
    say(&s, "b0", "alice", Some("group-b"), "<@123> 在吗").await;
    for (i, group) in [Some("group-a"), None].into_iter().enumerate() {
        let code = format!("ABCD234{}", i + 5);
        bind_code(&s, SITE_USER, &code).await;
        say(
            &s,
            &format!("bind{i}"),
            "alice",
            group,
            &format!("<@123> /绑定 {code}"),
        )
        .await;
    }
    let group_a: i64 = sqlx::query_scalar("SELECT id FROM qq_groups WHERE group_openid='group-a'")
        .fetch_one(&s.db)
        .await
        .unwrap();
    let group_b: i64 = sqlx::query_scalar("SELECT id FROM qq_groups WHERE group_openid='group-b'")
        .fetch_one(&s.db)
        .await
        .unwrap();
    memory(&s, 1, "对方住在上海", "private", None).await;
    memory(&s, 2, "对方喜欢猫", "public", None).await;
    memory(
        &s,
        3,
        "对方在 A 群说过要考研",
        "private",
        Some(repo::group_scope(group_a)),
    )
    .await;
    memory(
        &s,
        4,
        "对方在 B 群说过换了工作",
        "private",
        Some(repo::group_scope(group_b)),
    )
    .await;
    sqlx::query("INSERT INTO ai_shared_memories(id,scope,ai_user_id,content,origin) VALUES(9,$1,100,'这个群的群规是不刷屏','manual')")
        .bind(repo::group_scope(group_a))
        .execute(&s.db)
        .await
        .unwrap();

    // 单聊（私下场合）：全部个人记忆可用，不带任何群的共享记忆。
    say(&s, "c2c", "alice", None, "最近怎么样").await;
    let prompt = last_prompt(&llm);
    for text in ["对方住在上海", "对方喜欢猫", "要考研", "换了工作"] {
        assert!(prompt.contains(text), "单聊应带：{text}");
    }
    assert!(!prompt.contains("不刷屏"));

    // A 群（公开场合）：只带公开的和在 A 群说的，加 A 群的共享记忆；私事和 B 群的事不带。
    say(&s, "a1", "alice", Some("group-a"), "<@123> 最近怎么样").await;
    let prompt = last_prompt(&llm);
    assert!(prompt.contains("对方喜欢猫"));
    assert!(prompt.contains("要考研"));
    assert!(prompt.contains("不刷屏"));
    assert!(!prompt.contains("上海"), "私事不能进群");
    assert!(!prompt.contains("换了工作"), "B 群的事不能进 A 群");

    // B 群里还没绑定的身份是游客：不带任何个人记忆；共享记忆默认只给绑定用户。
    say(&s, "b1", "alice", Some("group-b"), "<@123> 最近怎么样").await;
    let prompt = last_prompt(&llm);
    for text in ["喜欢猫", "上海", "换了工作", "不刷屏"] {
        assert!(!prompt.contains(text), "游客不应带：{text}");
    }
    // 游客在 A 群：设置允许时能读 A 群的共享记忆，但仍没有个人记忆。
    sqlx::query("UPDATE qq_settings SET guest_reads_group_memory=TRUE")
        .execute(&s.db)
        .await
        .unwrap();
    s.invalidate().await;
    say(&s, "a2", "bob", Some("group-a"), "<@123> 群规是什么").await;
    let prompt = last_prompt(&llm);
    assert!(prompt.contains("不刷屏"));
    assert!(!prompt.contains("喜欢猫"));
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn extraction_scopes_personal_memories_and_feeds_group_memory_only_from_bound_users(
    db: PgPool,
) {
    let (s, llm, _qq, server) = setup(db).await;
    site_user(&s, SITE_USER, "alice_site").await;
    say(&s, "a0", "alice", Some("group-a"), "<@123> 在吗").await;
    bind_code(&s, SITE_USER, "ABCD2345").await;
    say(
        &s,
        "bind",
        "alice",
        Some("group-a"),
        "<@123> /绑定 ABCD2345",
    )
    .await;
    say(
        &s,
        "a1",
        "alice",
        Some("group-a"),
        "<@123> 我们群周五晚上一起打本",
    )
    .await;
    say(
        &s,
        "g1",
        "guest",
        Some("group-a"),
        "<@123> 游客的秘密口令是 swordfish",
    )
    .await;
    // 提炼任务已排上（对话停下来以后才执行）。
    assert_eq!(
        count(&s, "SELECT count(*) FROM graphile_worker.jobs WHERE task_identifier IN ('qq.extract_memories','qq.extract_group_memories')").await,
        2
    );
    let group_a: i64 = sqlx::query_scalar("SELECT id FROM qq_groups WHERE group_openid='group-a'")
        .fetch_one(&s.db)
        .await
        .unwrap();
    crate::memory_jobs::extract_personal(&s, SITE_USER, 100)
        .await
        .unwrap();
    // 群里提炼出的个人记忆记作在这个群说的（只在这个群用）。
    let scope: Option<String> = sqlx::query_scalar(
        "SELECT source_scope FROM ai_memories WHERE user_id=200 AND content='对方喜欢下雨天'",
    )
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(scope, Some(repo::group_scope(group_a)));
    let prompt = last_prompt(&llm);
    assert!(prompt.contains("打本"));
    assert!(!prompt.contains("swordfish"), "别人的话不进他的记忆");
    assert!(!prompt.contains("/绑定"), "指令不进提炼");

    crate::memory_jobs::extract_group(&s, group_a, 100)
        .await
        .unwrap();
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM ai_shared_memories WHERE content='这个群周五晚上一起打本'"
        )
        .await,
        1
    );
    let prompt = last_prompt(&llm);
    assert!(prompt.contains("打本"));
    assert!(
        !prompt.contains("swordfish"),
        "游客的话不沉淀进群的共同记忆"
    );
    // 进度只前进：再跑一次没有新消息，不再调模型。
    let calls = llm.calls.load(Ordering::SeqCst);
    crate::memory_jobs::extract_group(&s, group_a, 100)
        .await
        .unwrap();
    crate::memory_jobs::extract_personal(&s, SITE_USER, 100)
        .await
        .unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), calls);
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn role_and_reset_commands_work_in_private_chat_only(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    say(&s, "r0", "alice", None, "/角色").await;
    assert!(last_sent(&qq).contains("Fairy"));
    say(&s, "r1", "alice", None, "/角色 不存在的人").await;
    assert!(last_sent(&qq).contains("没有叫"));
    say(&s, "r2", "alice", None, "/角色 Fairy").await;
    assert!(last_sent(&qq).contains("已切换"));
    say(&s, "c1", "alice", None, "记住这句话：橘子").await;
    say(&s, "reset", "alice", None, "/重置").await;
    assert!(last_sent(&qq).contains("从头开始"));
    say(&s, "c2", "alice", None, "我刚才说了什么").await;
    assert!(
        !last_prompt(&llm).contains("橘子"),
        "重置后不带之前的上下文"
    );
    say(&s, "g", "alice", Some("group"), "<@123> /重置").await;
    assert!(last_sent(&qq).contains("管理员"));
    server.abort();
}

/// 带签名的真实格式群事件走完整的 webhook：验签、宽松解析、入库、建回复任务；解析不了的事件也 ACK。
#[sqlx::test(migrator = "MIGRATOR")]
async fn signed_group_event_goes_through_the_webhook(db: PgPool) {
    use crate::webhook::tests_support::{call, sign};
    let (s, llm, qq, server) = setup(db).await;
    let body = serde_json::to_vec(&json!({
        "op": 0, "id": "GROUP_AT_MESSAGE_CREATE:abc", "t": "GROUP_AT_MESSAGE_CREATE",
        "d": {
            "id": "ROBOT1.0_real", "content": " 你好呀", "timestamp": "2026-10-26T05:18:29+08:00",
            "group_openid": "REALGROUP", "group_id": "REALGROUP",
            "author": {"id": "REALMEMBER", "member_openid": "REALMEMBER", "union_openid": "U"},
            "mentions": [{"scope": "single", "is_you": true, "bot": true}], "attachments": null
        }
    }))
    .unwrap();
    let (status, reply) = call(
        &s,
        "123",
        "abc",
        &sign("abc", "1761427109", &body),
        "1761427109",
        body,
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(reply, json!({"op": 12, "d": 0}));
    let turn: i64 = sqlx::query_scalar("SELECT id FROM qq_turns")
        .fetch_one(&s.db)
        .await
        .unwrap();
    // 消息时间在过去（测试数据是固定时刻），被动窗口已过：不调用模型，也不发送。
    jobs::run_reply(&s, turn).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
    assert!(qq.sent.lock().unwrap().is_empty());
    // 字段对不上的事件：ACK，不入库，也不报 4xx（否则平台反复重推）。
    let broken =
        serde_json::to_vec(&json!({"op": 0, "t": "C2C_MESSAGE_CREATE", "d": {"content": "x"}}))
            .unwrap();
    let (status, _) = call(&s, "123", "abc", &sign("abc", "1", &broken), "1", broken).await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(count(&s, "SELECT count(*) FROM qq_messages").await, 1);
    server.abort();
}
