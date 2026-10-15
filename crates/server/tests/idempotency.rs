//! `Idempotency-Key` 中间件：经过完整中间件栈、连真实数据库。

#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use ik_kernel::config::{Config, Secret};
use ik_server::{AppState, build_app};
use sqlx::PgPool;
use tower::ServiceExt;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

const TOKEN: &str = "iks_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const KEY: &str = "0190f3e2-7a1b-7c3d-8e4f-0123456789ab";
const ORIGIN: &str = "https://interk.net";

async fn setup(db: &PgPool) -> Router {
    sqlx::query!(
        "INSERT INTO users (id, username, email, role, level, exam_passed_at)
         VALUES (1, 'writer', 'writer@example.com', 'user', 1, now())"
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
    build_app(AppState::new(config, db.clone()).unwrap()).unwrap()
}

fn create(key: Option<&str>, title: &str, auth: bool) -> Request<Body> {
    let mut builder = Request::post("/v1/posts")
        .header("content-type", "application/json")
        .header("origin", ORIGIN);
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    if auth {
        builder = builder.header("authorization", format!("Bearer {TOKEN}"));
    }
    builder
        .body(Body::from(
            serde_json::json!({ "title": title, "body": "正文" }).to_string(),
        ))
        .unwrap()
}

async fn send(
    app: &Router,
    request: Request<Body>,
) -> (StatusCode, http::HeaderMap, serde_json::Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, serde_json::from_slice(&bytes).unwrap())
}

async fn post_count(db: &PgPool) -> i64 {
    sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM posts"#)
        .fetch_one(db)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn replays_the_first_success(db: PgPool) {
    let app = setup(&db).await;
    let (status, headers, first) = send(&app, create(Some(KEY), "第一次", true)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(headers.get("idempotent-replayed").is_none());

    let (status, headers, again) = send(&app, create(Some(KEY), "第一次", true)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(headers["idempotent-replayed"], "true");
    assert_eq!(headers["content-type"], "application/json");
    assert_eq!(headers["etag"], "\"rev:1\"");
    assert_eq!(again["id"], first["id"]);
    assert_eq!(post_count(&db).await, 1);

    // 同一个键、不同的请求体。
    let (status, _, body) = send(&app, create(Some(KEY), "换了标题", true)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "IDEMPOTENCY_KEY_REUSED");
    // 不带键照常处理。
    let (status, _, _) = send(&app, create(None, "第一次", true)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(post_count(&db).await, 2);

    // 过期之后同一个键可以再用。
    sqlx::query!("UPDATE idempotency_keys SET created_at = now() - interval '25 hours'")
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(
        ik_server::idempotency::prune(&db, chrono::Utc::now())
            .await
            .unwrap(),
        1
    );
    let (status, headers, _) = send(&app, create(Some(KEY), "换了标题", true)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(headers.get("idempotent-replayed").is_none());
    assert_eq!(post_count(&db).await, 3);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failures_release_the_key_and_bad_keys_are_rejected(db: PgPool) {
    let app = setup(&db).await;
    let (status, _, body) = send(&app, create(Some(KEY), "", true)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "VALIDATION_FAILED");
    // 失败没有占用：改正后用同一个键成功。
    let (status, _, _) = send(&app, create(Some(KEY), "改好了", true)).await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _, body) = send(&app, create(Some("not-a-uuid"), "t", true)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "IDEMPOTENCY_KEY_INVALID");
    // 未登录：不做幂等处理，交给接口自己返回 401。
    let (status, _, body) = send(&app, create(Some(KEY), "t", false)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "UNAUTHORIZED");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn in_progress_requests_conflict_until_the_claim_expires(db: PgPool) {
    let app = setup(&db).await;
    let body = serde_json::json!({ "title": "并发", "body": "正文" }).to_string();
    // 模拟另一个实例正在处理同一个请求。
    // 指纹与中间件的算法一致：方法 \0 路径与查询 \0 请求体。
    let insert = |lock_secs: f64| {
        let db = db.clone();
        let body = body.clone();
        async move {
            sqlx::query!(
                r"INSERT INTO idempotency_keys (user_id, key, fingerprint, locked_until)
                  VALUES (1, $1::text::uuid,
                          sha256(convert_to('POST', 'UTF8') || '\x00'::bytea
                                 || convert_to('/v1/posts', 'UTF8') || '\x00'::bytea
                                 || convert_to($2, 'UTF8')),
                          now() + make_interval(secs => $3))
                  ON CONFLICT (user_id, key) DO UPDATE SET locked_until = EXCLUDED.locked_until",
                KEY,
                body,
                lock_secs,
            )
            .execute(&db)
            .await
            .unwrap();
        }
    };
    insert(60.0).await;
    let (status, headers, problem) = send(&app, create(Some(KEY), "并发", true)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(problem["code"], "IDEMPOTENCY_IN_PROGRESS");
    assert_eq!(headers["retry-after"], "1");
    assert_eq!(post_count(&db).await, 0);

    // 占用失效（那个实例崩溃了）：接手处理。
    insert(-1.0).await;
    let (status, _, _) = send(&app, create(Some(KEY), "并发", true)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(post_count(&db).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_duplicates_create_once(db: PgPool) {
    let app = setup(&db).await;
    let requests = (0..8).map(|_| send(&app, create(Some(KEY), "同时", true)));
    let results = futures_util::future::join_all(requests).await;
    let created = results
        .iter()
        .filter(|(s, h, _)| *s == StatusCode::CREATED && h.get("idempotent-replayed").is_none())
        .count();
    assert_eq!(created, 1);
    for (status, _, body) in &results {
        assert!(
            *status == StatusCode::CREATED || body["code"] == "IDEMPOTENCY_IN_PROGRESS",
            "{status} {body}"
        );
    }
    assert_eq!(post_count(&db).await, 1);
}
