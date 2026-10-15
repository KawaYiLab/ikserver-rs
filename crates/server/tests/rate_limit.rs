//! 全站默认限流：经过完整中间件栈。

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

const TOKEN: &str = "iks_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const FAKE: &str = "iks_ccccccccccccccccccccccccccccccccccccccccccc";

fn app(db: PgPool, configure: impl FnOnce(&mut Config)) -> Router {
    let mut config = Config::default();
    config.database.url = Secret::new("postgres://unused");
    config.hot.enabled = false;
    // 一层反代：按 X-Forwarded-For 最右边一个区分客户端。
    config.http.trusted_proxy_hops = 1;
    configure(&mut config);
    build_app(AppState::new(config, db).unwrap()).unwrap()
}

fn get(ip: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::get("/v1/benefits").header("x-forwarded-for", ip);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::empty()).unwrap()
}

async fn send(
    app: &Router,
    request: Request<Body>,
) -> (StatusCode, http::HeaderMap, serde_json::Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or_default(),
    )
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn anonymous_reads_are_limited_per_ip_with_headers(db: PgPool) {
    let app = app(db, |c| c.rate_limit.anon_read_per_min = 2);
    let (status, headers, _) = send(&app, get("10.0.0.1", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["ratelimit-limit"], "2");
    assert_eq!(headers["ratelimit-remaining"], "1");
    assert_eq!(headers["ratelimit-reset"], "30");
    let (status, headers, _) = send(&app, get("10.0.0.1", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["ratelimit-remaining"], "0");
    assert_eq!(headers["ratelimit-reset"], "60");

    let (status, headers, body) = send(&app, get("10.0.0.1", None)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["code"], "RATE_LIMITED");
    assert_eq!(headers["ratelimit-remaining"], "0");
    let retry: u64 = headers["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=31).contains(&retry), "{retry}");

    // 别的 IP 不受影响；/v1 之外（探针）不限流。
    let (status, _, _) = send(&app, get("10.0.0.2", None)).await;
    assert_eq!(status, StatusCode::OK);
    let probe = Request::get("/healthz")
        .header("x-forwarded-for", "10.0.0.1")
        .body(Body::empty())
        .unwrap();
    let (status, headers, _) = send(&app, probe).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("ratelimit-limit").is_none());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn users_have_their_own_bucket_and_fake_tokens_stay_on_the_ip(db: PgPool) {
    sqlx::query!(
        "INSERT INTO users (id, username, email, role, level) VALUES (1, 'u1', 'u1@example.com', 'user', 1)"
    )
    .execute(&db)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO sessions (id, user_id, token_hash, login_method, expires_at, max_expires_at)
         VALUES (1, 1, sha256(convert_to($1, 'UTF8')), 'password',
                 now() + interval '1 day', now() + interval '1 day')",
        TOKEN
    )
    .execute(&db)
    .await
    .unwrap();
    let app = app(db, |c| {
        c.rate_limit.anon_read_per_min = 1;
        c.rate_limit.user_read_per_min = 3;
    });

    // 用尽 IP 的游客额度。
    assert_eq!(send(&app, get("10.0.0.9", None)).await.0, StatusCode::OK);
    assert_eq!(
        send(&app, get("10.0.0.9", None)).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    // 编造的令牌仍算这个 IP 的游客。
    assert_eq!(
        send(&app, get("10.0.0.9", Some(FAKE))).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    // 登录用户按用户计：同一个 IP 也有自己的额度，换 IP 不换桶。
    let (status, headers, _) = send(&app, get("10.0.0.9", Some(TOKEN))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["ratelimit-limit"], "3");
    assert_eq!(
        send(&app, get("10.0.0.10", Some(TOKEN))).await.0,
        StatusCode::OK
    );
    assert_eq!(
        send(&app, get("10.0.0.11", Some(TOKEN))).await.0,
        StatusCode::OK
    );
    assert_eq!(
        send(&app, get("10.0.0.12", Some(TOKEN))).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn writes_have_a_separate_budget_and_limiting_can_be_disabled(db: PgPool) {
    let app_limited = app(db.clone(), |c| {
        c.rate_limit.anon_read_per_min = 1;
        c.rate_limit.anon_write_per_min = 1;
    });
    let write = || {
        Request::post("/v1/auth/logout")
            .header("x-forwarded-for", "10.0.0.20")
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        send(&app_limited, get("10.0.0.20", None)).await.0,
        StatusCode::OK
    );
    // 读的额度用完了，写还有。
    assert_ne!(
        send(&app_limited, write()).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        send(&app_limited, write()).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );

    let open = app(db, |c| {
        c.rate_limit.enabled = false;
        c.rate_limit.anon_read_per_min = 1;
    });
    for _ in 0..3 {
        let (status, headers, _) = send(&open, get("10.0.0.21", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(headers.get("ratelimit-limit").is_none());
    }
}
