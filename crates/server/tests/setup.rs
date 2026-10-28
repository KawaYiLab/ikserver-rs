//! 首次安装：`GET/POST /v1/setup`，经过完整中间件栈、连真实数据库。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use ik_kernel::config::{Config, Secret};
use ik_server::{AppState, build_app};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

const ORIGIN: &str = "https://forum.example.com";

fn app(db: &PgPool) -> Router {
    let mut config = Config::default();
    config.database.url = Secret::new("postgres://unused");
    config.http.allowed_origins = vec![ORIGIN.to_owned()];
    config.hot.enabled = false;
    build_app(AppState::new(config, db.clone()).unwrap()).unwrap()
}

async fn send(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .body(if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

fn input(email: &str) -> Value {
    json!({ "email": email, "username": "论坛站长", "password": "correct-horse-1" })
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn creates_the_first_admin_once(db: PgPool) {
    let app = app(&db);
    let (status, body) = send(&app, "GET", "/v1/setup", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["needed"], true);

    let (status, body) = send(&app, "POST", "/v1/setup", input("Admin@Example.com")).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let role = sqlx::query_scalar!("SELECT role FROM users WHERE email = 'admin@example.com'")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(role, "admin");

    let (_, body) = send(&app, "GET", "/v1/setup", Value::Null).await;
    assert_eq!(body["needed"], false);
    let (status, body) = send(&app, "POST", "/v1/setup", input("b@example.com")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "ALREADY_SET_UP");

    let (status, body) = send(
        &app,
        "POST",
        "/v1/auth/login",
        json!({ "identifier": "admin@example.com", "password": "correct-horse-1", "client": "web" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn existing_admin_means_no_setup(db: PgPool) {
    sqlx::query!(
        "INSERT INTO users (id, username, email, role) VALUES (1, 'boss', 'boss@example.com', 'admin')"
    )
    .execute(&db)
    .await
    .unwrap();
    let app = app(&db);
    let (_, body) = send(&app, "GET", "/v1/setup", Value::Null).await;
    assert_eq!(body["needed"], false);
    let (status, body) = send(&app, "POST", "/v1/setup", input("a@example.com")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}
