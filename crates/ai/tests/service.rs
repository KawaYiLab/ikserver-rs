//! AI 第一阶段：额度账本（并发、拒绝退回、结算、回收）、上游流式客户端、配置导入与公开读取。

// clippy.toml 的测试豁免只覆盖 #[test] 函数本身，不覆盖集成测试里的辅助函数。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::{Duration, Utc};
use futures_util::StreamExt;
use ik_ai::AiState;
use ik_ai::dto::ImportFile;
use ik_ai::llm::{ChatMessage, ChatProvider, ChatRequest, LlmError, OpenAiCompatible, StreamEvent};
use ik_ai::quota::{self, Denial, Hold, Limits, Settled};
use ik_ai::service;
use ik_auth::Principal;
use ik_kernel::bus::LocalBus;
use ik_kernel::config::{Ai, AiProvider};
use ik_kernel::{Id, Role, Snowflake};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

const HOLD_TTL: Duration = Duration::minutes(15);

fn state(db: PgPool) -> AiState {
    let ids = Arc::new(Snowflake::new(1).unwrap());
    AiState::for_tests(db, ids, Arc::new(LocalBus::new(16)), Ai::default())
}

async fn user(db: &PgPool, id: i64) -> Principal {
    sqlx::query!(
        "INSERT INTO users (id, username, email, role, level) VALUES ($1, $2, $3, 'user', 1)",
        id,
        format!("user{id}"),
        format!("user{id}@example.com"),
    )
    .execute(db)
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

async fn used(db: &PgPool, user_id: i64) -> i64 {
    let mut conn = db.acquire().await.unwrap();
    quota::read(&mut conn, user_id, Utc::now())
        .await
        .unwrap()
        .used
}

async fn reserve(
    db: &PgPool,
    ids: &Snowflake,
    limits: Limits,
    user_id: i64,
    amount: i64,
) -> Result<Hold, Denial> {
    let mut tx = db.begin().await.unwrap();
    let outcome = quota::reserve(&mut tx, ids, limits, user_id, amount, Utc::now(), HOLD_TTL)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    outcome
}

async fn settle(db: &PgPool, hold: &Hold, key: &str, actual: i64) -> Settled {
    let mut tx = db.begin().await.unwrap();
    let settled = quota::settle(&mut tx, hold, key, actual, Utc::now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    settled
}

fn limits(per_user: i64, site: i64) -> Limits {
    Limits {
        global_enabled: true,
        per_user,
        site,
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_reservations_admit_only_what_the_budget_allows(db: PgPool) {
    let ids = Arc::new(Snowflake::new(1).unwrap());
    // 上限 10000、每份 3000：进来时用量 0、3000、6000、9000 的放行，之后的都拒（QUOTA-005）。
    let tasks: Vec<_> = (0..10)
        .map(|_| {
            let db = db.clone();
            let ids = ids.clone();
            tokio::spawn(async move { reserve(&db, &ids, limits(10_000, 0), 42, 3000).await })
        })
        .collect();
    let mut admitted = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => admitted += 1,
            Err(denial) => assert_eq!(denial, Denial::UserBudget),
        }
    }
    assert_eq!(admitted, 4);
    // 被拒的就地退回：计数只含放行的 4 份；全站计数同量纲。
    assert_eq!(used(&db, 42).await, 12_000);
    assert_eq!(used(&db, quota::SITE).await, 12_000);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn denials_refund_both_counters(db: PgPool) {
    let ids = Snowflake::new(1).unwrap();
    // 全站闸门：个人通过但全站超限，两个计数器上的预占都退回（QUOTA-009）。
    let first = reserve(&db, &ids, limits(0, 100), 1, 100).await.unwrap();
    assert_eq!(first.amount, 100);
    assert_eq!(
        reserve(&db, &ids, limits(0, 100), 2, 50).await,
        Err(Denial::SiteBudget)
    );
    assert_eq!(used(&db, 2).await, 0);
    assert_eq!(used(&db, quota::SITE).await, 100);
    // 两道都超时报个人。
    assert_eq!(
        reserve(&db, &ids, limits(100, 100), 1, 10).await,
        Err(Denial::UserBudget)
    );
    // 全局关闭直接拒，不动账。
    let off = Limits {
        global_enabled: false,
        per_user: 0,
        site: 0,
    };
    assert_eq!(
        reserve(&db, &ids, off, 3, 10).await,
        Err(Denial::GlobalDisabled)
    );
    // 都没配上限：不预占、不登记（QUOTA-006）。
    let free = reserve(&db, &ids, limits(0, 0), 3, 10).await.unwrap();
    assert_eq!((free.id, free.amount), (None, 0));
    assert_eq!(used(&db, 3).await, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn settlement_adjusts_once_and_survives_reclaim(db: PgPool) {
    let ids = Snowflake::new(1).unwrap();
    let hold = reserve(&db, &ids, limits(1_000_000, 0), 7, 5000)
        .await
        .unwrap();
    assert_eq!(used(&db, 7).await, 5000);
    // 多退少补：实际 1200 → 退 3800。
    assert_eq!(
        settle(&db, &hold, "job:1", 1200).await,
        Settled::Applied { delta: -3800 }
    );
    assert_eq!(used(&db, 7).await, 1200);
    assert_eq!(used(&db, quota::SITE).await, 1200);
    // 同一份预占只结算一次（QUOTA-014/015）。
    assert_eq!(settle(&db, &hold, "job:1", 1200).await, Settled::Duplicate);
    assert_eq!(used(&db, 7).await, 1200);

    // 过期未结算的预占被回收整份退回；迟到的真实结算按实际完整扣一次（QUOTA-016）。
    let late = reserve(&db, &ids, limits(1_000_000, 0), 7, 5000)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let reclaimed = quota::reclaim_expired(&mut tx, Utc::now() + Duration::hours(1), 100)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(reclaimed, 1);
    assert_eq!(used(&db, 7).await, 1200);
    assert_eq!(
        settle(&db, &late, "job:2", 300).await,
        Settled::Applied { delta: 300 }
    );
    assert_eq!(used(&db, 7).await, 1500);

    // 认领者与登记不符：报错、不动账（QUOTA-014）。
    let other = reserve(&db, &ids, limits(1_000_000, 0), 7, 100)
        .await
        .unwrap();
    let forged = Hold {
        user_id: 8,
        ..other
    };
    let mut tx = db.begin().await.unwrap();
    assert!(
        quota::settle(&mut tx, &forged, "job:3", 0, Utc::now())
            .await
            .is_err()
    );
    drop(tx);
    assert_eq!(used(&db, 7).await, 1600);

    // 无上限的结算按执行 id 去重，照实累加。
    let free = reserve(&db, &ids, limits(0, 0), 9, 10).await.unwrap();
    assert_eq!(
        settle(&db, &free, "job:4:1", 70).await,
        Settled::Applied { delta: 70 }
    );
    assert_eq!(settle(&db, &free, "job:4:1", 70).await, Settled::Duplicate);
    assert_eq!(used(&db, 9).await, 70);
}

// ---------- 上游流式客户端 ----------

/// 本地 SSE 桩：按顺序回放给定的 data 帧；`status` 非 200 时回错误体。
async fn stub(status: u16, body: String) -> String {
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |axum::Json(request): axum::Json<serde_json::Value>| {
            let body = body.clone();
            async move {
                assert_eq!(request["stream"], true);
                assert_eq!(request["stream_options"]["include_usage"], true);
                let status = StatusCode::from_u16(status).unwrap();
                let content_type = if status.is_success() {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                (status, [(header::CONTENT_TYPE, content_type)], body).into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/v1")
}

fn sse(frames: &[&str]) -> String {
    let mut body: String = frames.iter().map(|f| format!("data: {f}\n\n")).collect();
    body.push_str("data: [DONE]\n\n");
    body
}

fn provider(base_url: String) -> OpenAiCompatible {
    OpenAiCompatible::new(&AiProvider {
        base_url,
        api_key: ik_kernel::config::Secret::new("sk-test"),
    })
    .unwrap()
}

fn request() -> ChatRequest {
    ChatRequest {
        model: "m".into(),
        messages: vec![ChatMessage::system("s"), ChatMessage::user("hi")],
        tools: Vec::new(),
    }
}

#[tokio::test]
async fn streams_reasoning_content_tool_calls_and_usage() {
    let base = stub(
        200,
        sse(&[
            r#"{"choices":[{"delta":{"reasoning_content":"想一想"}}]}"#,
            r#"{"choices":[{"delta":{"content":"你好"}}]}"#,
            // 重复的增量文本也是合法内容，不能被当成快照去重。
            r#"{"choices":[{"delta":{"content":"你好"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"get_hot_posts","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":120,"completion_tokens":30}}"#,
        ]),
    )
    .await;
    let events: Vec<StreamEvent> = provider(base)
        .stream(&request())
        .await
        .unwrap()
        .map(Result::unwrap)
        .collect()
        .await;
    assert_eq!(
        events,
        vec![
            StreamEvent::Reasoning("想一想".into()),
            StreamEvent::Content("你好".into()),
            StreamEvent::Content("你好".into()),
            StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("call_1".into()),
                name: Some("get_hot_posts".into()),
                arguments: Some("{}".into()),
            },
            StreamEvent::Finish("tool_calls".into()),
            StreamEvent::Usage(ik_ai::llm::Usage {
                prompt_tokens: 120,
                completion_tokens: 30,
            }),
        ]
    );
}

#[tokio::test]
async fn upstream_errors_are_classified() {
    let base = stub(
        429,
        r#"{"error":{"message":"Rate limit reached","type":"requests"}}"#.to_owned(),
    )
    .await;
    let error = provider(base).stream(&request()).await.err().unwrap();
    assert!(error.is_rate_limited(), "{error:?}");

    let base = stub(
        400,
        r#"{"error":{"message":"invalid model","type":"invalid_request_error"}}"#.to_owned(),
    )
    .await;
    let error = provider(base).stream(&request()).await.err().unwrap();
    assert!(matches!(error, LlmError::Upstream(_)), "{error:?}");

    // 200 的流里以错误对象结束。
    let base = stub(
        200,
        sse(&[
            r#"{"choices":[{"delta":{"content":"半"}}]}"#,
            r#"{"error":{"message":"upstream overloaded"}}"#,
        ]),
    )
    .await;
    let items: Vec<_> = provider(base)
        .stream(&request())
        .await
        .unwrap()
        .collect()
        .await;
    assert!(matches!(items[0], Ok(StreamEvent::Content(_))));
    assert!(matches!(items[1], Err(LlmError::Upstream(_))));
}

// ---------- 导入与公开读取 ----------

fn import_file(username: &str) -> ImportFile {
    serde_json::from_value(serde_json::json!({
        "settings": { "globalEnabled": true, "dailyTokensPerUser": 2000, "dailyTokens": 0 },
        "models": [
            { "key": "base", "displayName": "基础", "provider": "deepseek",
              "providerModel": "deepseek-chat", "isDefault": true, "sortOrder": 1 },
            { "key": "pro", "displayName": "专业", "provider": "deepseek",
              "providerModel": "deepseek-reasoner", "costWeight": 3, "sortOrder": 2 },
            { "key": "off", "displayName": "停用", "provider": "deepseek",
              "providerModel": "x", "enabled": false }
        ],
        "characters": [
            { "slug": "fairy", "displayName": "Fairy", "username": username, "enabled": true,
              "systemPrompt": "你是 Fairy。", "defaultModelKey": "pro",
              "allowedModelKeys": ["pro", "off"],
              "suggestedQuestions": ["最近有什么热帖？", "  "],
              "examples": [{ "user": "你好", "assistant": "你好，绳匠。" }] },
            { "slug": "hidden", "displayName": "隐藏", "username": username, "enabled": true,
              "knockVisible": false }
        ]
    }))
    .unwrap()
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn import_then_public_reads(db: PgPool) {
    let state = state(db.clone());
    let reader = user(&db, 100).await;
    user(&db, 200).await;
    let report = service::import(&state, import_file("user200"))
        .await
        .unwrap();
    assert_eq!((report.models, report.characters), (3, 2));
    let is_ai = sqlx::query_scalar!("SELECT is_ai_agent FROM users WHERE id = 200")
        .fetch_one(&db)
        .await
        .unwrap();
    assert!(is_ai);

    // 模型：只列启用的，不含上游标识与倍率。
    let models = service::models(&state).await.unwrap();
    let keys: Vec<&str> = models.items.iter().map(|m| m.key.as_str()).collect();
    assert_eq!(keys, ["base", "pro"]);
    let json = serde_json::to_value(&models).unwrap().to_string();
    assert!(!json.contains("deepseek") && !json.contains("costWeight"));

    // 角色：只列敲敲可见的；白名单只剩启用的模型；空建议问题被丢弃。
    let characters = service::characters(&state).await.unwrap();
    assert_eq!(characters.items.len(), 1);
    let fairy = &characters.items[0];
    assert_eq!(fairy.slug, "fairy");
    assert_eq!(fairy.default_model_key.as_deref(), Some("pro"));
    assert_eq!(fairy.allowed_model_keys, Some(vec!["pro".to_owned()]));
    assert_eq!(fairy.suggested_questions, ["最近有什么热帖？"]);
    assert_eq!(fairy.bio, None);
    assert_eq!(fairy.user.id.get(), 200);

    // 模型决议与预占倍率。
    let catalog = state.catalog.get().await.unwrap();
    let card = catalog.card_for_user(200).unwrap();
    assert_eq!(card.slug, "fairy");
    assert_eq!(card.examples.len(), 1);
    let resolved = catalog.resolve_model(card, Some("base"), None).unwrap();
    assert_eq!(
        resolved.key, "pro",
        "base 不在白名单内，静默回落到角色卡默认"
    );
    assert!((catalog.max_weight() - 3.0).abs() < f64::EPSILON);

    // 额度：只给百分比。
    let view = service::quota(&state, &reader).await.unwrap();
    assert!(view.available && !view.unlimited && !view.exhausted);
    assert_eq!(view.percent, 0);
    let ids = Snowflake::new(2).unwrap();
    reserve(&db, &ids, catalog.limits, 100, 500).await.unwrap();
    let view = service::quota(&state, &reader).await.unwrap();
    assert_eq!(view.percent, 25);

    // 再导入：按 key / slug 更新，默认模型可以换。
    let mut again = import_file("user200");
    again.models[0].is_default = false;
    again.models[1].is_default = true;
    service::import(&state, again).await.unwrap();
    let catalog = state.catalog.get().await.unwrap();
    assert_eq!(catalog.default_model().unwrap().key, "pro");
    assert_eq!(catalog.models.len(), 3);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn import_rejects_bad_input_without_writing(db: PgPool) {
    let state = state(db.clone());
    user(&db, 200).await;
    let mut bad = import_file("user200");
    bad.characters[0].allowed_model_keys = vec!["nope".into()];
    let problem = service::import(&state, bad).await.unwrap_err();
    assert_eq!(problem.code(), "VALIDATION_FAILED");
    let count = sqlx::query_scalar!("SELECT count(*) FROM ai_models")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, Some(0));

    let missing = import_file("ghost");
    assert!(service::import(&state, missing).await.is_err());

    let mut weird = import_file("user200");
    weird.models[0].key = "Bad Key".into();
    assert!(service::import(&state, weird).await.is_err());
}
