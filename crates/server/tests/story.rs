//! 剧情库 MCP 端点穿过完整中间件栈的行为：CSRF、全站限流（按读计）、未启用、方法限制。
//! 不连真实数据库：语料来自 ik-story 的测试样例（小型提取器输出目录）。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use ik_kernel::config::{Config, Secret};
use ik_server::{AppState, build_app};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

fn fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../story/tests/fixtures/sample"
    )
    .to_owned()
}

async fn build(configure: impl FnOnce(&mut Config)) -> Router {
    let mut config = Config::default();
    config.database.url = Secret::new("postgres://test:test@127.0.0.1:1/test");
    config.http.allowed_origins = vec!["https://interk.net".to_owned()];
    configure(&mut config);
    let db = PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(500))
        .connect_lazy(config.database.url.expose())
        .unwrap();
    let state = AppState::new(config, db).unwrap();
    state.story.load().await.unwrap();
    build_app(state).unwrap()
}

fn mcp() -> http::request::Builder {
    Request::post("/v1/story/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, http::HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, headers, json)
}

#[tokio::test]
async fn mcp_client_works_through_the_full_stack() {
    let app = build(|c| c.story.source = fixture()).await;
    let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}});
    let (status, headers, body) =
        send(&app, mcp().body(Body::from(init.to_string())).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(headers[header::CONTENT_TYPE], "application/json");
    assert!(headers.contains_key("x-request-id"));
    assert_eq!(body["result"]["serverInfo"]["name"], "zzz-story-mcp");

    let call = json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"zzz_search","arguments":{"keyword":"狡兔屋 妮可"}}});
    let (status, _, body) = send(&app, mcp().body(Body::from(call.to_string())).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["isError"], false);
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("妮可：安比，交给你了！"), "{text}");

    let note = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    let (status, _, _) = send(&app, mcp().body(Body::from(note.to_string())).unwrap()).await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let (status, _, _) = send(
        &app,
        Request::get("/v1/story/mcp").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

    // 浏览器里跨站发起的请求照常被 CSRF 层拦下；非浏览器的 MCP 客户端不带 Origin，不受影响。
    let (status, _, body) = send(
        &app,
        mcp()
            .header(header::ORIGIN, "https://evil.example")
            .header("sec-fetch-site", "cross-site")
            .body(Body::from(init.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn mcp_posts_count_as_reads_in_the_global_limiter() {
    let app = build(|c| {
        c.story.source = fixture();
        c.rate_limit.anon_read_per_min = 3;
        c.rate_limit.anon_write_per_min = 1;
    })
    .await;
    let ping = json!({"jsonrpc":"2.0","id":1,"method":"ping"});
    for _ in 0..3 {
        let (status, headers, _) =
            send(&app, mcp().body(Body::from(ping.to_string())).unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["ratelimit-limit"], "3");
    }
    let (status, _, body) = send(&app, mcp().body(Body::from(ping.to_string())).unwrap()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["code"], "RATE_LIMITED");
}

#[tokio::test]
async fn access_key_and_disabled() {
    let app = build(|c| {
        c.story.source = fixture();
        c.story.access_key = Secret::new("k3y");
    })
    .await;
    let ping = json!({"jsonrpc":"2.0","id":1,"method":"ping"});
    let (status, headers, body) =
        send(&app, mcp().body(Body::from(ping.to_string())).unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "STORY_ACCESS_DENIED");
    assert_eq!(headers[header::WWW_AUTHENTICATE], "Bearer");
    let (status, _, _) = send(
        &app,
        mcp()
            .header(header::AUTHORIZATION, "Bearer k3y")
            .body(Body::from(ping.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let disabled = build(|_| {}).await;
    let (status, headers, body) =
        send(&disabled, mcp().body(Body::from(ping.to_string())).unwrap()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "STORY_DISABLED");
    assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
}
