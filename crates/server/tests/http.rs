//! 不依赖真实数据库的 HTTP 层测试：中间件栈、错误格式、探针、OpenAPI、
//! 认证接口在触达数据库之前的行为（参数校验、限流、未登录）。

// clippy.toml 的测试豁免只覆盖 #[test] 函数本身，不覆盖集成测试里的辅助函数。
#![allow(clippy::unwrap_used)]

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use ik_kernel::config::{Config, Secret};
use ik_server::{AppState, build_app};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

fn app() -> Router {
    let mut config = Config::default();
    // 指向一个必然连不上的地址：这些测试只验证 HTTP 层，/readyz 应据此返回 503。
    config.database.url = Secret::new("postgres://test:test@127.0.0.1:1/test");
    config.http.allowed_origins = vec!["https://interk.net".to_owned()];
    let db = PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(500))
        .connect_lazy(config.database.url.expose())
        .unwrap();
    build_app(AppState::new(config, db).unwrap()).unwrap()
}

async fn send(request: Request<Body>) -> (StatusCode, http::HeaderMap, serde_json::Value) {
    send_to(&app(), request).await
}

async fn send_to(
    app: &Router,
    request: Request<Body>,
) -> (StatusCode, http::HeaderMap, serde_json::Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, headers, json)
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn healthz_ok_and_generates_request_id() {
    let (status, headers, body) = send(get("/healthz")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(headers.get("x-request-id").unwrap().len(), 32);
}

#[tokio::test]
async fn valid_incoming_request_id_is_echoed_and_invalid_replaced() {
    let request = Request::get("/healthz")
        .header("x-request-id", "trace-abc_123")
        .body(Body::empty())
        .unwrap();
    let (_, headers, _) = send(request).await;
    assert_eq!(headers.get("x-request-id").unwrap(), "trace-abc_123");

    let request = Request::get("/healthz")
        .header("x-request-id", "bad id with spaces")
        .body(Body::empty())
        .unwrap();
    let (_, headers, _) = send(request).await;
    assert_ne!(headers.get("x-request-id").unwrap(), "bad id with spaces");
}

#[tokio::test]
async fn unknown_route_is_problem_json_with_request_id() {
    let (status, headers, body) = send(get("/v1/nope")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    assert_eq!(body["code"], "NOT_FOUND");
    assert_eq!(body["status"], 404);
    assert_eq!(
        body["requestId"],
        headers.get("x-request-id").unwrap().to_str().unwrap()
    );
}

#[tokio::test]
async fn readyz_reports_503_when_database_unreachable() {
    let (status, _, body) = send(get("/readyz")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "SERVICE_UNAVAILABLE");
}

#[tokio::test]
async fn cross_site_post_is_rejected_by_csrf() {
    let request = Request::post("/v1/anything")
        .header(header::HOST, "api.example.com")
        .header(header::ORIGIN, "https://evil.example")
        .header("sec-fetch-site", "cross-site")
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "CSRF_REJECTED");
}

#[tokio::test]
async fn trusted_origin_post_passes_csrf() {
    let request = Request::post("/v1/anything")
        .header(header::HOST, "api.example.com")
        .header(header::ORIGIN, "https://interk.net")
        .header("sec-fetch-site", "same-site")
        .body(Body::empty())
        .unwrap();
    let (status, _, _) = send(request).await;
    // 通过 CSRF 后落到 404 兜底，说明没有被拦截。
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn openapi_document_is_served() {
    let (status, _, body) = send(get("/v1/openapi.json")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["openapi"].as_str().unwrap().starts_with("3."));
    assert!(body["paths"]["/healthz"].is_object());
    assert!(body["components"]["schemas"]["ProblemBody"].is_object());
    assert!(body["paths"]["/v1/auth/login"]["post"].is_object());
    assert!(body["paths"]["/v1/me/sessions/{id}"]["delete"].is_object());
    assert!(body["paths"]["/v1/me/check-in"]["post"].is_object());
    assert!(body["paths"]["/v1/posts/{id}/denny"]["post"].is_object());
    assert!(body["paths"]["/v1/exam/attempts/{id}/submit"]["post"].is_object());
    assert!(body["paths"]["/v1/users/{id}/follow"]["put"].is_object());
    assert!(body["paths"]["/v1/users/{id}/block"]["delete"].is_object());
    assert!(body["paths"]["/v1/me/relations"]["get"].is_object());
    assert!(body["paths"]["/v1/posts/{id}/like"]["put"].is_object());
    assert!(body["paths"]["/v1/posts/{id}/triple"]["post"].is_object());
    assert!(body["paths"]["/v1/comments/{id}/like"]["delete"].is_object());
    assert!(body["paths"]["/v1/me/favorites"]["get"].is_object());
    assert!(body["paths"]["/v1/notifications/buckets/{key}"]["get"].is_object());
    assert!(body["paths"]["/v1/notifications/{id}/read"]["put"].is_object());
    assert!(body["paths"]["/v1/realtime"]["get"].is_object());
    assert!(body["paths"]["/v1/users/{id}"]["get"].is_object());
    assert!(body["paths"]["/v1/users/search"]["get"].is_object());
    assert!(body["paths"]["/v1/me/username"]["put"].is_object());
    assert!(body["paths"]["/v1/feed"]["get"].is_object());
    assert!(body["paths"]["/v1/feed/hot"]["get"].is_object());
    assert!(body["paths"]["/v1/me/reads"]["post"].is_object());
    assert!(body["components"]["schemas"]["ServerFrame"].is_object());
    let schemes = &body["components"]["securitySchemes"];
    assert_eq!(schemes["session_cookie"]["in"], "cookie");
    assert_eq!(schemes["bearer"]["scheme"], "bearer");
}

fn post_json(uri: &str, body: &str) -> Request<Body> {
    Request::post(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

#[tokio::test]
async fn benefits_are_public() {
    let (status, _, body) = send(get("/v1/benefits")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["level"], 0);
    assert_eq!(body["benefits"]["postMaxBodyChars"], 0);
    assert_eq!(body["nextBenefits"]["postMaxBodyChars"], 3000);
}

#[tokio::test]
async fn exam_requires_login() {
    let (status, _, body) = send(get("/v1/exam")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "UNAUTHORIZED");
}

#[tokio::test]
async fn realtime_requires_websocket_handshake() {
    let (status, _, body) = send(get("/v1/realtime")).await;
    assert_eq!(status, StatusCode::UPGRADE_REQUIRED);
    assert_eq!(body["code"], "WEBSOCKET_REQUIRED");

    let request = Request::get("/v1/realtime")
        .header(header::ORIGIN, "https://evil.example")
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "CSRF_REJECTED");
}

#[tokio::test]
async fn me_without_credentials_is_401() {
    let (status, _, body) = send(get("/v1/me")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "UNAUTHORIZED");
}

#[tokio::test]
async fn malformed_json_is_400_invalid_json() {
    let (status, _, body) = send(post_json("/v1/auth/login", "{not json")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INVALID_JSON");
}

#[tokio::test]
async fn missing_content_type_is_415() {
    let request = Request::post("/v1/auth/login")
        .body(Body::from(r#"{"identifier":"a","password":"b"}"#))
        .unwrap();
    let (status, _, body) = send(request).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(body["code"], "UNSUPPORTED_MEDIA_TYPE");
}

#[tokio::test]
async fn unknown_fields_are_rejected() {
    let (status, _, body) = send(post_json(
        "/v1/auth/email-codes",
        r#"{"email":"a@example.com","purpose":"login","extra":1}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "VALIDATION_FAILED");
}

#[tokio::test]
async fn invalid_email_is_field_error() {
    let (status, _, body) = send(post_json(
        "/v1/auth/email-codes",
        r#"{"email":"not-an-email","purpose":"register"}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "VALIDATION_FAILED");
    assert_eq!(body["errors"][0]["field"], "email");
}

#[tokio::test]
async fn auth_routes_are_rate_limited_per_route() {
    let app = app();
    for _ in 0..20 {
        let (status, _, _) = send_to(&app, post_json("/v1/auth/login", "{")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, headers, body) = send_to(&app, post_json("/v1/auth/login", "{")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["code"], "RATE_LIMITED");
    let retry: u64 = headers
        .get(header::RETRY_AFTER)
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry), "{retry}");

    // 另一条路由有独立配额。
    let (status, _, _) = send_to(&app, post_json("/v1/auth/login/email-code", "{")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_pages_are_served_under_admin() {
    // 后台有没有打包都成立（CI 不构建后台）：目录入口跳转，缺失的资源是 404 而不是回退成页面。
    let (status, headers, _) = send(get("/admin")).await;
    assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(headers.get(header::LOCATION).unwrap(), "/admin/");

    let (status, _, body) = send(get("/admin/_nuxt/missing.js")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["status"], 404);
}

/// 剧情库 MCP 端点穿过完整中间件栈的行为：CSRF、全站限流（按读计）、未启用、方法限制。
/// 语料来自 ik-story 的测试样例（小型提取器输出目录）。放在本文件里而不是单独的测试文件：
/// 每个集成测试文件都会链接出一个完整的 ik-server，CI runner 的磁盘放不下更多。
mod story {
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
        // 只在用本地语料时加载：这里的数据库连不上，数据库里的语料版本由 admin.rs 的用例覆盖。
        if !state.config.story.source.is_empty() {
            state.story.load().await.unwrap();
        }
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
        let app = build(|c| {
            c.story.source = fixture();
            c.story.public = true;
        })
        .await;
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}});
        let (status, headers, body) =
            send(&app, mcp().body(Body::from(init.to_string())).unwrap()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(headers[header::CONTENT_TYPE], "application/json");
        assert!(headers.contains_key("x-request-id"));
        assert_eq!(body["result"]["serverInfo"]["name"], "绳网档案");

        let call = json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"ik_search","arguments":{"keyword":"狡兔屋 妮可"}}});
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
            c.story.public = true;
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
}
