//! QQ callbacks pass through the real middleware stack without using the anonymous IP bucket.
#![allow(clippy::unwrap_used)]

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use ik_kernel::config::{Config, QqbotApp, Secret};
use ik_server::{AppState, build_app};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

#[tokio::test]
async fn qq_validation_passes_csrf_and_has_its_own_rate_limit() {
    let mut config = Config::default();
    config.hot.enabled = false;
    config.rate_limit.anon_write_per_min = 1;
    config.qqbot.enabled = true;
    config.qqbot.allow_validation = true;
    config.qqbot.webhook_rate_per_second = 50;
    config.qqbot.apps.insert("test".to_owned(), QqbotApp {
        app_id: "123".to_owned(), secret: Secret::new("test-secret"),
    });
    let pool = PgPoolOptions::new().connect_lazy("postgres://test:test@127.0.0.1:1/test").unwrap();
    let app = build_app(AppState::new(config, pool).unwrap()).unwrap();
    let payload = r#"{"op":13,"d":{"event_ts":"123456","plain_token":"ABC123"}}"#;
    // No Origin / Sec-Fetch-Site: server-to-server callbacks must reach signature validation.
    for _ in 0..3 {
        let response = app.clone().oneshot(Request::post("/v1/qq/webhook")
            .header("x-bot-appid", "123").header("content-type", "application/json")
            .body(Body::from(payload)).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key("ratelimit-limit"));
        let body: serde_json::Value = serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(body["plain_token"], "ABC123");
        assert_eq!(body["signature"].as_str().unwrap().len(), 128);
    }
    let oversized = app.clone().oneshot(Request::post("/v1/qq/webhook")
        .header("x-bot-appid", "123").body(Body::from("x".repeat(65 * 1024))).unwrap()).await.unwrap();
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let unknown = app.oneshot(Request::post("/v1/qq/webhook")
        .header("x-bot-appid", "999").body(Body::from(payload)).unwrap()).await.unwrap();
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
}
