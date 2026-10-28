//! 连接池只有 1 个连接时，常用接口照样能完成。
//!
//! 一个请求拿着连接再去池里要第二个，并发一高所有请求都卡在等第二个连接上，直到超时一起 503
//! （CPU 却是空的）。池里只有 1 个连接时这种请求必然超时，所以这里能把它抓出来。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use ik_kernel::config::{Config, Secret};
use ik_server::{AppState, build_app};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

const ORIGIN: &str = "https://interk.net";
const ADMIN: &str = "iks_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

async fn setup(db: &PgPool) -> Router {
    ik_kernel::jobs::Jobs::new(db.clone())
        .migrate()
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, email, role, level, exam_passed_at)
         VALUES (1, 'boss', 'boss@example.com', 'admin', 7, now())",
    )
    .execute(db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO sessions (id, user_id, token_hash, login_method, expires_at, max_expires_at)
         VALUES (1, 1, sha256(convert_to($1, 'UTF8')), 'password',
                 now() + interval '1 day', now() + interval '1 day')",
    )
    .bind(ADMIN)
    .execute(db)
    .await
    .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(2))
        .connect_with((*db.connect_options()).clone())
        .await
        .unwrap();
    let mut config = Config::default();
    config.database.url = Secret::new("postgres://unused");
    config.http.allowed_origins = vec![ORIGIN.to_owned()];
    config.hot.enabled = false;
    // 关掉限流：这里一个令牌要连着调几十个接口。
    config.rate_limit.enabled = false;
    build_app(AppState::new(config, pool).unwrap()).unwrap()
}

async fn call(
    app: &Router,
    token: Option<&str>,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("user-agent", "pool-test/1.0");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request
        .body(if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn common_endpoints_work_with_a_single_connection(db: PgPool) {
    let app = setup(&db).await;

    let (status, post) = call(
        &app,
        Some(ADMIN),
        "POST",
        "/v1/posts?publish=true",
        json!({ "title": "连接池", "body": "正文" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{post}");
    let post_id = post["id"].as_str().unwrap().to_owned();
    let (status, comment) = call(
        &app,
        Some(ADMIN),
        "POST",
        &format!("/v1/posts/{post_id}/comments"),
        json!({ "body": "沙发" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{comment}");
    let comment_id = comment["id"].as_str().unwrap().to_owned();

    let public = [
        "/v1/site".to_owned(),
        "/v1/posts?limit=20".to_owned(),
        "/v1/feed".to_owned(),
        "/v1/feed/hot".to_owned(),
        "/v1/categories".to_owned(),
        "/v1/emotes".to_owned(),
        "/v1/presence".to_owned(),
        format!("/v1/posts/{post_id}"),
        format!("/v1/posts/{post_id}/comments"),
        format!("/v1/posts/{post_id}/related"),
        format!("/v1/comments/{comment_id}/replies"),
        "/v1/users/1".to_owned(),
        "/v1/users/1/posts".to_owned(),
        "/v1/users/1/comments".to_owned(),
        "/v1/ai/characters".to_owned(),
    ];
    let signed_in = [
        "/v1/me",
        "/v1/me/profile",
        "/v1/me/posts",
        "/v1/me/likes",
        "/v1/me/favorites",
        "/v1/me/following",
        "/v1/me/avatars",
        "/v1/me/cards",
        "/v1/me/wallet",
        "/v1/feed/following",
        "/v1/feed/unread",
        "/v1/inbox",
        "/v1/inbox/unread-count",
        "/v1/notifications",
        "/v1/notifications/buckets",
        "/v1/users/search?q=bo",
        "/v1/users/mention-candidates?q=bo",
        "/v1/admin/dashboard",
        "/v1/admin/dashboard/trends",
        "/v1/admin/system",
        "/v1/admin/settings",
        "/v1/admin/users",
        "/v1/admin/users/1",
        "/v1/admin/posts",
        "/v1/admin/comments",
        "/v1/admin/reports",
        "/v1/admin/audit",
        "/v1/admin/ai/characters",
        "/v1/admin/cosmetics/avatars",
        "/v1/admin/cosmetics/cards",
        "/v1/admin/emotes",
    ];
    let requests = public
        .iter()
        .map(|uri| (None, uri.as_str()))
        .chain(public.iter().map(|uri| (Some(ADMIN), uri.as_str())))
        .chain(signed_in.iter().map(|uri| (Some(ADMIN), *uri)));
    for (token, uri) in requests {
        let (status, body) = call(&app, token, "GET", uri, Value::Null).await;
        assert!(
            status.is_success(),
            "GET {uri}（{}）返回 {status}：{body}",
            if token.is_some() { "登录" } else { "游客" }
        );
    }
}
