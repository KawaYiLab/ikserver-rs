//! AI 私聊的端到端接线：HTTP 建会话、发消息 → 后台任务执行器 → 本地 SSE 桩上游 → 定稿。
//! 上游分别来自部署配置与管理后台（`ai_providers`）。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use ik_kernel::config::{AiProvider, Config, Secret};
use ik_server::{AppState, build_app};
use sqlx::PgPool;
use tower::ServiceExt;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

const TOKEN: &str = "iks_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ORIGIN: &str = "https://interk.net";

/// OpenAI 兼容的流式桩。
async fn stub_upstream() -> String {
    let body = [
        r#"{"choices":[{"delta":{"content":"你好，"}}]}"#,
        r#"{"choices":[{"delta":{"content":"我是 Fairy。"},"finish_reason":"stop"}]}"#,
        r#"{"choices":[],"usage":{"prompt_tokens":40,"completion_tokens":10}}"#,
    ]
    .iter()
    .map(|f| format!("data: {f}\n\n"))
    .collect::<String>()
        + "data: [DONE]\n\n";
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move || {
            let body = body.clone();
            async move { ([(http::header::CONTENT_TYPE, "text/event-stream")], body) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/v1")
}

/// 上游提供方放在哪。
#[derive(Clone, Copy)]
enum ProviderFrom {
    Config,
    Admin,
}

async fn setup(db: &PgPool, from: ProviderFrom) -> (Router, AppState) {
    // 只入队、不执行的进程要先建好任务队列的表（`ik-server migrate` 会做）。
    ik_kernel::jobs::Jobs::new(db.clone())
        .migrate()
        .await
        .unwrap();
    sqlx::query!(
        "INSERT INTO users (id, username, email, role, level, exam_passed_at, is_ai_agent)
         VALUES (1, 'writer', 'writer@example.com', 'user', 1, now(), FALSE),
                (2, 'fairy', 'fairy@example.com', 'user', 1, now(), TRUE)"
    )
    .execute(db)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO sessions (id, user_id, token_hash, login_method, expires_at, max_expires_at)
         VALUES (1, 1, sha256(convert_to($1, 'UTF8')), 'password',
                 now() + interval '1 day', now() + interval '1 day')",
        TOKEN
    )
    .execute(db)
    .await
    .unwrap();
    let mut config = Config::default();
    config.database.url = Secret::new("postgres://unused");
    config.http.allowed_origins = vec![ORIGIN.to_owned()];
    config.hot.enabled = false;
    let base_url = stub_upstream().await;
    match from {
        ProviderFrom::Config => {
            config.ai.providers.insert(
                "stub".to_owned(),
                AiProvider {
                    base_url,
                    api_key: Secret::new("sk-test"),
                },
            );
        }
        ProviderFrom::Admin => {
            sqlx::query!(
                "INSERT INTO ai_providers (id, name, display_name, base_url, api_key)
                 VALUES (1, 'stub', '桩', $1, 'sk-test')",
                base_url
            )
            .execute(db)
            .await
            .unwrap();
        }
    }
    let state = AppState::new(config, db.clone()).unwrap();
    let file = serde_json::from_value(serde_json::json!({
        "models": [{ "key": "base", "displayName": "基础", "provider": "stub",
                     "providerModel": "stub-chat", "isDefault": true }],
        "characters": [{ "slug": "fairy", "displayName": "Fairy", "username": "fairy",
                         "enabled": true, "systemPrompt": "你是 Fairy。" }]
    }))
    .unwrap();
    ik_ai::service::import(&state.ai, file).await.unwrap();
    (build_app(state.clone()).unwrap(), state)
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, json)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn ai_session_reply_end_to_end(db: PgPool) {
    let (app, state) = setup(&db, ProviderFrom::Config).await;
    converse(&app, &state).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn ai_reply_through_admin_managed_provider(db: PgPool) {
    let (app, state) = setup(&db, ProviderFrom::Admin).await;
    converse(&app, &state).await;
}

/// 建会话、发一条消息、跑一次任务，回复按桩上游定稿。
async fn converse(app: &Router, state: &AppState) {
    let app = app.clone();
    let (status, conversation) = call(
        &app,
        "POST",
        "/v1/ai/sessions",
        serde_json::json!({ "character": "fairy", "title": "测试" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{conversation}");
    assert_eq!(conversation["kind"], "ai");
    assert_eq!(conversation["title"], "测试");
    let id = conversation["id"].as_str().unwrap().to_owned();

    let (status, sent) = call(
        &app,
        "POST",
        &format!("/v1/dm/conversations/{id}/messages"),
        serde_json::json!({ "content": "你好" }),
    )
    .await;
    assert!(status.is_success(), "{status} {sent}");

    ik_server::jobs::worker_options(state)
        .unwrap()
        .concurrency(1)
        .init()
        .await
        .unwrap()
        .run_once()
        .await
        .unwrap();

    let (status, page) = call(
        &app,
        "GET",
        &format!("/v1/dm/conversations/{id}/messages"),
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let reply = &page["items"][0];
    assert_eq!(reply["content"], "你好，我是 Fairy。", "{page}");
    assert_eq!(reply["ai"]["state"], "done");
    assert_eq!(reply["ai"]["modelKey"], "base");
    assert_eq!(reply["senderId"], "2");

    let (status, quota) = call(&app, "GET", "/v1/ai/quota", serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(quota["available"], true);
}

/// 同一角色多个会话：列表按最近活动排序、分页；首条消息给没标题的会话自动起名。
#[sqlx::test(migrator = "MIGRATOR")]
async fn ai_sessions_list_and_auto_title(db: PgPool) {
    let (app, _state) = setup(&db, ProviderFrom::Config).await;
    let mut ids = Vec::new();
    for _ in 0..2 {
        let (status, conversation) = call(
            &app,
            "POST",
            "/v1/ai/sessions",
            serde_json::json!({ "character": "fairy" }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{conversation}");
        assert!(conversation.get("title").is_none(), "{conversation}");
        ids.push(conversation["id"].as_str().unwrap().to_owned());
    }

    // 给先建的那条发消息：它变成最近活动的一条，并用消息起标题。
    let (status, sent) = call(
        &app,
        "POST",
        &format!("/v1/dm/conversations/{}/messages", ids[0]),
        serde_json::json!({ "content": "  帮我查查\n艾莲的技能" }),
    )
    .await;
    assert!(status.is_success(), "{status} {sent}");

    let (status, page) = call(
        &app,
        "GET",
        "/v1/ai/sessions?character=fairy",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{page}");
    assert_eq!(items[0]["id"], ids[0].as_str());
    assert_eq!(items[0]["title"], "帮我查查");
    assert_eq!(items[1]["id"], ids[1].as_str());
    assert!(items[1].get("title").is_none());
    assert!(page.get("nextCursor").is_none());

    // 已有标题不会被后面的消息覆盖。
    let (status, _) = call(
        &app,
        "POST",
        &format!("/v1/dm/conversations/{}/messages", ids[0]),
        serde_json::json!({ "content": "再来一条" }),
    )
    .await;
    assert!(status.is_success());

    let (status, first) = call(
        &app,
        "GET",
        "/v1/ai/sessions?character=fairy&limit=1",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["items"][0]["title"], "帮我查查");
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let (status, second) = call(
        &app,
        "GET",
        &format!("/v1/ai/sessions?character=fairy&limit=1&cursor={cursor}"),
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["items"][0]["id"], ids[1].as_str());
    assert!(second.get("nextCursor").is_none());

    let (status, _) = call(
        &app,
        "GET",
        "/v1/ai/sessions?character=nobody",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
