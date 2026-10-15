//! AI 私聊的端到端接线：HTTP 建会话、发消息 → 后台任务执行器 → 本地 SSE 桩上游 → 定稿。

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

async fn setup(db: &PgPool) -> (Router, AppState) {
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
    config.ai.providers.insert(
        "stub".to_owned(),
        AiProvider {
            base_url: stub_upstream().await,
            api_key: Secret::new("sk-test"),
        },
    );
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
    let (app, state) = setup(&db).await;
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

    ik_server::jobs::worker_options(&state)
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
