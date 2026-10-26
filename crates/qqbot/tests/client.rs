#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use ik_kernel::config::{Qqbot, QqbotApp, Secret};
use ik_qqbot::client::{ApiClient, ApiEndpoint, ErrorClass, SendError, SendOutcome};
use serde_json::{Value, json};
use tokio::{
    net::TcpListener,
    sync::{Mutex, Notify},
    task::JoinHandle,
};

const APP: &str = "102123456";

#[derive(Default)]
struct Calls {
    tokens: Vec<Value>,
    messages: Vec<(String, String, String, Value)>,
    token_responses: VecDeque<(StatusCode, Value)>,
    message_responses: VecDeque<(StatusCode, Value)>,
    token_response_headers: HeaderMap,
    raw_message_responses: VecDeque<(StatusCode, HeaderMap, String)>,
    token_wait: Option<Arc<Notify>>,
}

struct FakeApi {
    config: Qqbot,
    calls: Arc<Mutex<Calls>>,
    task: JoinHandle<()>,
}

impl Drop for FakeApi {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn token(
    State(state): State<Arc<Mutex<Calls>>>,
    Json(body): Json<Value>,
) -> (StatusCode, HeaderMap, Json<Value>) {
    let mut calls = state.lock().await;
    calls.tokens.push(body);
    let (status, body) = calls.token_responses.pop_front().unwrap_or_else(|| {
        (
            StatusCode::OK,
            json!({"access_token":"cached-token","expires_in":7200}),
        )
    });
    let result = (status, calls.token_response_headers.clone(), Json(body));
    let wait = calls.token_wait.take();
    drop(calls);
    if let Some(wait) = wait {
        wait.notified().await;
    }
    result
}

async fn message(
    State(state): State<Arc<Mutex<Calls>>>,
    Path((collection, openid)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert_eq!(headers.get("x-union-appid").unwrap(), APP);
    let authorization = headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let mut calls = state.lock().await;
    calls
        .messages
        .push((collection, openid, authorization, body));
    if let Some((status, headers, body)) = calls.raw_message_responses.pop_front() {
        return (status, headers, body).into_response();
    }
    let (status, body) = calls
        .message_responses
        .pop_front()
        .unwrap_or_else(|| (StatusCode::OK, json!({"id":"sent"})));
    (status, Json(body)).into_response()
}

impl FakeApi {
    async fn start(calls: Calls) -> Self {
        let calls = Arc::new(Mutex::new(calls));
        let router = Router::new()
            .route("/token", post(token))
            .route("/v2/{collection}/{openid}/messages", post(message))
            .with_state(calls.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let config = Qqbot {
            enabled: true,
            token_url: format!("http://{address}/token"),
            api_base: Some(format!("http://{address}")),
            apps: BTreeMap::from([(
                "test".to_owned(),
                QqbotApp {
                    app_id: APP.to_owned(),
                    secret: Secret::new("test-secret"),
                },
            )]),
            ..Qqbot::default()
        };
        Self {
            config,
            calls,
            task,
        }
    }
}

#[tokio::test]
async fn token_is_cached_for_concurrent_sends_and_both_scenes() {
    let server = FakeApi::start(Calls::default()).await;
    let client = ApiClient::new(&server.config).unwrap();
    let (first, second) = tokio::join!(
        client.send(APP, "group", "group-openid", "incoming-1", 1, "第一段"),
        client.send(APP, "c2c", "user-openid", "incoming-2", 1, "单聊"),
    );
    first.unwrap();
    second.unwrap();
    client
        .send(APP, "group", "group-openid", "incoming-1", 2, "第二段")
        .await
        .unwrap();
    let calls = server.calls.lock().await;
    assert_eq!(
        calls.tokens,
        vec![json!({"appId":APP,"clientSecret":"test-secret"})]
    );
    assert_eq!(calls.messages.len(), 3);
    assert!(
        calls
            .messages
            .iter()
            .all(|(_, _, auth, body)| auth == "QQBot cached-token" && body["msg_type"] == 0)
    );
    assert!(
        calls
            .messages
            .iter()
            .any(|(collection, openid, _, body)| collection == "users"
                && openid == "user-openid"
                && body["msg_id"] == "incoming-2")
    );
    let group: Vec<_> = calls
        .messages
        .iter()
        .filter(|(collection, _, _, _)| collection == "groups")
        .collect();
    assert_eq!(group[0].3["msg_seq"], 1);
    assert_eq!(group[1].3["msg_seq"], 2);
    assert_eq!(group[1].3["msg_id"], "incoming-1");
}

#[tokio::test]
async fn token_http_200_business_error_never_sends_a_message() {
    let server = FakeApi::start(Calls {
        token_responses: VecDeque::from([(
            StatusCode::OK,
            json!({"code":40001,"message":"invalid secret"}),
        )]),
        ..Calls::default()
    })
    .await;
    let client = ApiClient::new(&server.config).unwrap();
    let error = client
        .send(APP, "group", "group", "incoming", 1, "hello")
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        SendError::Api {
            status: 200,
            code: 40001
        }
    ));
    assert!(server.calls.lock().await.messages.is_empty());
}

#[tokio::test]
async fn rejected_token_refreshes_once_and_keeps_message_identity() {
    let server = FakeApi::start(Calls {
        token_responses: VecDeque::from([
            (
                StatusCode::OK,
                json!({"access_token":"old","expires_in":"7200"}),
            ),
            (
                StatusCode::OK,
                json!({"access_token":"new","expires_in":7200}),
            ),
        ]),
        message_responses: VecDeque::from([(StatusCode::UNAUTHORIZED, json!({"code":40014}))]),
        ..Calls::default()
    })
    .await;
    let client = ApiClient::new(&server.config).unwrap();
    client
        .send(APP, "group", "group", "incoming", 3, "hello")
        .await
        .unwrap();
    let calls = server.calls.lock().await;
    assert_eq!(calls.tokens.len(), 2);
    assert_eq!(calls.messages.len(), 2);
    assert_eq!(calls.messages[0].2, "QQBot old");
    assert_eq!(calls.messages[1].2, "QQBot new");
    assert_eq!(calls.messages[0].3, calls.messages[1].3);
    assert_eq!(calls.messages[1].3["msg_seq"], 3);
}

#[tokio::test]
async fn near_expiry_refresh_is_throttled_and_accepts_the_same_platform_token() {
    let response = (
        StatusCode::OK,
        json!({"access_token":"same","expires_in":60}),
    );
    let server = FakeApi::start(Calls {
        token_responses: VecDeque::from([response.clone(), response]),
        ..Calls::default()
    })
    .await;
    let client = ApiClient::new(&server.config).unwrap();
    client
        .send(APP, "c2c", "user", "incoming", 1, "first")
        .await
        .unwrap();
    client
        .send(APP, "c2c", "user", "incoming", 2, "second")
        .await
        .unwrap();
    // 刚取过：即使离过期不到 5 分钟，也不为每条消息再打一次 token 接口（会被限频）。
    let calls = server.calls.lock().await;
    assert_eq!(calls.tokens.len(), 1);
    assert!(
        calls
            .messages
            .iter()
            .all(|(_, _, auth, _)| auth == "QQBot same")
    );
}

#[tokio::test]
async fn url_rejection_strips_links_and_reuses_the_same_sequence() {
    for code in [304003, 40034028, 40054010] {
        let server = FakeApi::start(Calls {
            message_responses: VecDeque::from([(StatusCode::BAD_REQUEST, json!({"code":code}))]),
            ..Calls::default()
        })
        .await;
        let client = ApiClient::new(&server.config).unwrap();
        let rendered = client
            .send_rendered(
                APP,
                "group",
                "group",
                "incoming",
                2,
                "攻略 https://interk.net/post/123",
            )
            .await
            .unwrap();
        assert_eq!(rendered, "攻略");
        let calls = server.calls.lock().await;
        assert_eq!(calls.messages.len(), 2);
        assert_eq!(calls.tokens.len(), 1);
        assert_eq!(calls.messages[1].3["content"], "攻略");
        for sent in &calls.messages {
            assert_eq!(sent.3["msg_id"], "incoming");
            assert_eq!(sent.3["msg_seq"], 2);
        }
    }
}

#[tokio::test]
async fn terminal_rejection_and_repeated_auth_failure_do_not_loop() {
    for (status, code, attempts) in [
        (StatusCode::BAD_REQUEST, 304023, 1),
        (StatusCode::OK, 304005, 1),
        (StatusCode::UNAUTHORIZED, 40014, 2),
    ] {
        let response = (status, json!({"code":code}));
        let server = FakeApi::start(Calls {
            // A refresh must return a different token before it may be sent again.
            token_responses: VecDeque::from([
                (
                    StatusCode::OK,
                    json!({"access_token":"first","expires_in":7200}),
                ),
                (
                    StatusCode::OK,
                    json!({"access_token":"second","expires_in":7200}),
                ),
            ]),
            message_responses: VecDeque::from([response.clone(), response]),
            ..Calls::default()
        })
        .await;
        let client = ApiClient::new(&server.config).unwrap();
        let error = client
            .send(APP, "group", "group", "incoming", 1, "hello")
            .await
            .unwrap_err();
        assert!(!error.retryable());
        assert_eq!(server.calls.lock().await.messages.len(), attempts);
    }
}

async fn advance_clock(duration: Duration) {
    // Keep socket IO on a running clock; pause only while advancing cached-token deadlines.
    tokio::time::pause();
    tokio::time::advance(duration).await;
    tokio::time::resume();
}

#[tokio::test]
async fn send_once_keeps_both_codes_and_never_rewrites_or_retries() {
    let server = FakeApi::start(Calls {
        message_responses: VecDeque::from([
            (
                StatusCode::BAD_REQUEST,
                json!({"code":304003,"err_code":"40054010","trace_id":"trace-123"}),
            ),
            (
                StatusCode::UNAUTHORIZED,
                json!({"code":11201,"err_code":40012001}),
            ),
        ]),
        ..Calls::default()
    })
    .await;
    let client = ApiClient::new(&server.config).unwrap();
    let first = client
        .send_once(
            APP,
            "group",
            "group",
            "incoming",
            1,
            "link https://interk.net/post/123",
        )
        .await;
    assert_eq!(first.outcome, SendOutcome::Rejected);
    assert_eq!(first.class, ErrorClass::Url);
    assert_eq!(first.meta.code, Some(304003));
    assert_eq!(first.meta.err_code, Some(40054010));
    assert_eq!(first.meta.trace_id.as_deref(), Some("trace-123"));
    let second = client
        .send_once(APP, "group", "group", "incoming", 2, "second")
        .await;
    assert_eq!(second.class, ErrorClass::Auth);
    let calls = server.calls.lock().await;
    assert_eq!(calls.tokens.len(), 1);
    assert_eq!(calls.messages.len(), 2);
    assert_eq!(
        calls.messages[0].3["content"],
        "link https://interk.net/post/123"
    );
}

#[tokio::test]
async fn response_headers_and_http_failures_survive_bad_bodies() {
    for body in [
        "not json".to_owned(),
        "x".repeat(65 * 1024),
        r#"{"code":{},"err_code":"unexpected"}"#.to_owned(),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "75".parse().unwrap());
        headers.insert("x-tps-trace-id", "header-trace".parse().unwrap());
        let server = FakeApi::start(Calls {
            raw_message_responses: VecDeque::from([(StatusCode::TOO_MANY_REQUESTS, headers, body)]),
            ..Calls::default()
        })
        .await;
        let client = ApiClient::new(&server.config).unwrap();
        let result = client
            .send_once(APP, "c2c", "user", "incoming", 1, "hello")
            .await;
        assert_eq!(result.outcome, SendOutcome::Rejected);
        assert_eq!(result.class, ErrorClass::RateLimited);
        assert_eq!(result.meta.http_status, Some(429));
        assert_eq!(result.meta.trace_id.as_deref(), Some("header-trace"));
        assert_eq!(result.meta.retry_after, Some(Duration::from_secs(75)));
        assert!(result.meta.malformed);
    }
}

#[tokio::test]
async fn pending_and_deduplicated_messages_are_not_reported_as_sent() {
    let server = FakeApi::start(Calls {
        message_responses: VecDeque::from([
            (
                StatusCode::ACCEPTED,
                json!({"code":304024,"data":{"message_audit":{"audit_id":"audit-1"}}}),
            ),
            (StatusCode::BAD_REQUEST, json!({"err_code":40054005})),
        ]),
        ..Calls::default()
    })
    .await;
    let client = ApiClient::new(&server.config).unwrap();
    let pending = client
        .send_once(APP, "group", "group", "in", 1, "hello")
        .await;
    assert_eq!(pending.outcome, SendOutcome::Pending);
    assert_eq!(pending.audit_id.as_deref(), Some("audit-1"));
    let duplicate = client
        .send_once(APP, "group", "group", "in", 1, "hello")
        .await;
    assert_eq!(duplicate.outcome, SendOutcome::Unknown);
    assert_eq!(duplicate.class, ErrorClass::Deduplicated);
    assert_eq!(server.calls.lock().await.messages.len(), 2);
}

#[tokio::test]
async fn token_rate_limit_is_shared_by_concurrent_requests_and_then_recovers() {
    let mut headers = HeaderMap::new();
    headers.insert("retry-after", "90".parse().unwrap());
    let server = FakeApi::start(Calls {
        token_responses: VecDeque::from([
            (StatusCode::OK, json!({"code":100001})),
            (
                StatusCode::OK,
                json!({"access_token":"recovered","expires_in":7200}),
            ),
        ]),
        token_response_headers: headers,
        ..Calls::default()
    })
    .await;
    let client = Arc::new(ApiClient::new(&server.config).unwrap());
    let results = futures_util::future::join_all((0..100).map(|_| {
        let client = client.clone();
        async move {
            client
                .send_once(APP, "group", "group", "in", 1, "hello")
                .await
        }
    }))
    .await;
    for result in results {
        assert_eq!(result.meta.endpoint, ApiEndpoint::Token);
        assert_eq!(result.meta.code, Some(100001));
        assert_eq!(result.class, ErrorClass::RateLimited);
        assert!(!result.message_attempted);
        assert!(result.meta.retry_after.unwrap() > Duration::from_secs(80));
    }
    {
        let calls = server.calls.lock().await;
        assert_eq!(calls.tokens.len(), 1);
        assert!(calls.messages.is_empty());
    }
    advance_clock(Duration::from_secs(91)).await;
    let result = client
        .send_once(APP, "group", "group", "in", 1, "hello")
        .await;
    assert_eq!(result.outcome, SendOutcome::Accepted);
    let calls = server.calls.lock().await;
    assert_eq!(calls.tokens.len(), 2);
    assert_eq!(calls.messages.len(), 1);
}

#[tokio::test]
async fn failed_refresh_uses_valid_cache_but_never_extends_its_expiry() {
    let server = FakeApi::start(Calls {
        token_responses: VecDeque::from([
            (
                StatusCode::OK,
                json!({"access_token":"old","expires_in":60}),
            ),
            (StatusCode::OK, json!({"code":100001})),
            (
                StatusCode::OK,
                json!({"access_token":"new","expires_in":7200}),
            ),
        ]),
        ..Calls::default()
    })
    .await;
    let client = ApiClient::new(&server.config).unwrap();
    assert_eq!(
        client
            .send_once(APP, "group", "group", "in", 1, "first")
            .await
            .outcome,
        SendOutcome::Accepted
    );
    advance_clock(Duration::from_secs(31)).await;
    assert_eq!(
        client
            .send_once(APP, "group", "group", "in", 2, "fallback")
            .await
            .outcome,
        SendOutcome::Accepted
    );
    assert_eq!(
        client
            .send_once(APP, "group", "group", "in", 3, "cached")
            .await
            .outcome,
        SendOutcome::Accepted
    );
    advance_clock(Duration::from_secs(25)).await;
    let unavailable = client
        .send_once(APP, "group", "group", "in", 4, "expired")
        .await;
    assert!(!unavailable.message_attempted);
    assert_eq!(unavailable.class, ErrorClass::RateLimited);
    {
        let calls = server.calls.lock().await;
        assert_eq!(calls.tokens.len(), 2);
        assert_eq!(calls.messages.len(), 3);
        assert!(
            calls
                .messages
                .iter()
                .all(|(_, _, auth, _)| auth == "QQBot old")
        );
    }
    advance_clock(Duration::from_secs(15)).await;
    assert_eq!(
        client
            .send_once(APP, "group", "group", "in", 4, "new")
            .await
            .outcome,
        SendOutcome::Accepted
    );
    let calls = server.calls.lock().await;
    assert_eq!(calls.tokens.len(), 3);
    assert_eq!(calls.messages.last().unwrap().2, "QQBot new");
}

#[tokio::test]
async fn rejection_never_falls_back_to_the_rejected_generation() {
    let server = FakeApi::start(Calls {
        token_responses: VecDeque::from([
            (
                StatusCode::OK,
                json!({"access_token":"old","expires_in":7200}),
            ),
            (StatusCode::OK, json!({"code":100001})),
        ]),
        message_responses: VecDeque::from([(
            StatusCode::UNAUTHORIZED,
            json!({"code":11201,"err_code":40012001}),
        )]),
        ..Calls::default()
    })
    .await;
    let client = ApiClient::new(&server.config).unwrap();
    let denied = client.send_once(APP, "c2c", "user", "in", 1, "hello").await;
    assert!(
        client
            .invalidate_token(APP, denied.token_generation.unwrap())
            .await
    );
    for _ in 0..5 {
        let result = client.send_once(APP, "c2c", "user", "in", 1, "hello").await;
        assert_eq!(result.class, ErrorClass::RateLimited);
        assert!(!result.message_attempted);
    }
    let calls = server.calls.lock().await;
    assert_eq!(calls.tokens.len(), 2);
    assert_eq!(calls.messages.len(), 1);
}

#[tokio::test]
async fn late_rejection_cannot_invalidate_a_new_generation() {
    let server = FakeApi::start(Calls {
        token_responses: VecDeque::from([
            (
                StatusCode::OK,
                json!({"access_token":"old","expires_in":7200}),
            ),
            (
                StatusCode::OK,
                json!({"access_token":"new","expires_in":7200}),
            ),
        ]),
        ..Calls::default()
    })
    .await;
    let client = ApiClient::new(&server.config).unwrap();
    let old = client
        .send_once(APP, "group", "group", "in", 1, "one")
        .await
        .token_generation
        .unwrap();
    assert!(client.invalidate_token(APP, old).await);
    let new = client
        .send_once(APP, "group", "group", "in", 2, "two")
        .await
        .token_generation
        .unwrap();
    assert_ne!(old, new);
    assert!(!client.invalidate_token(APP, old).await);
    assert_eq!(
        client
            .send_once(APP, "group", "group", "in", 3, "three")
            .await
            .token_generation,
        Some(new)
    );
    assert_eq!(server.calls.lock().await.tokens.len(), 2);
}

#[tokio::test]
async fn token_endpoint_cannot_revalidate_the_same_rejected_token() {
    let server = FakeApi::start(Calls {
        message_responses: VecDeque::from([(StatusCode::UNAUTHORIZED, json!({"code":40014}))]),
        ..Calls::default()
    })
    .await;
    let client = ApiClient::new(&server.config).unwrap();
    let denied = client
        .send_once(APP, "group", "group", "in", 1, "hello")
        .await;
    client
        .invalidate_token(APP, denied.token_generation.unwrap())
        .await;
    let denied_again = client
        .send_once(APP, "group", "group", "in", 1, "hello")
        .await;
    assert_eq!(denied_again.class, ErrorClass::Auth);
    assert_eq!(denied_again.meta.endpoint, ApiEndpoint::Token);
    assert!(!denied_again.message_attempted);
    let calls = server.calls.lock().await;
    assert_eq!(calls.tokens.len(), 2);
    assert_eq!(calls.messages.len(), 1);
}

#[tokio::test]
async fn refreshing_the_same_token_keeps_its_generation() {
    let token = (
        StatusCode::OK,
        json!({"access_token":"same","expires_in":120}),
    );
    let server = FakeApi::start(Calls {
        token_responses: VecDeque::from([token.clone(), token]),
        ..Calls::default()
    })
    .await;
    let client = ApiClient::new(&server.config).unwrap();
    let first = client
        .send_once(APP, "group", "group", "in", 1, "first")
        .await;
    advance_clock(Duration::from_secs(31)).await;
    let second = client
        .send_once(APP, "group", "group", "in", 2, "second")
        .await;
    assert_eq!(first.token_generation, second.token_generation);
    assert!(
        client
            .invalidate_token(APP, first.token_generation.unwrap())
            .await
    );
    assert_eq!(server.calls.lock().await.tokens.len(), 2);
}

#[tokio::test]
async fn token_and_message_success_keep_optional_fields_permissive() {
    let server=FakeApi::start(Calls {
        token_responses:VecDeque::from([(StatusCode::OK,json!({
            "access_token":"usable","expires_in":" 7200 ","code":"ok","err_code":null,"extra":{"new":true}
        }))]),
        message_responses:VecDeque::from([
            (StatusCode::OK,json!({"code":null,"err_code":null,"new_field":[]})),
            (StatusCode::OK,json!({"id":"sent","code":{},"timestamp":null})),
        ]),..Calls::default()
    }).await;
    let client = ApiClient::new(&server.config).unwrap();
    let first = client
        .send_once(APP, "group", "group", "in", 1, "first")
        .await;
    assert_eq!(first.outcome, SendOutcome::Accepted);
    assert!(first.message_id.is_none());
    let second = client
        .send_once(APP, "group", "group", "in", 2, "second")
        .await;
    assert_eq!(second.outcome, SendOutcome::Accepted);
    assert!(second.meta.malformed);
    assert_eq!(server.calls.lock().await.tokens.len(), 1);
}

#[tokio::test]
async fn permanent_refresh_rejection_does_not_hide_behind_valid_cache() {
    let server = FakeApi::start(Calls {
        token_responses: VecDeque::from([
            (
                StatusCode::OK,
                json!({"access_token":"old","expires_in":120}),
            ),
            (StatusCode::OK, json!({"code":100016})),
        ]),
        ..Calls::default()
    })
    .await;
    let client = ApiClient::new(&server.config).unwrap();
    assert_eq!(
        client
            .send_once(APP, "c2c", "user", "in", 1, "first")
            .await
            .outcome,
        SendOutcome::Accepted
    );
    advance_clock(Duration::from_secs(31)).await;
    for _ in 0..3 {
        let denied = client
            .send_once(APP, "c2c", "user", "in", 2, "second")
            .await;
        assert_eq!(denied.class, ErrorClass::Permanent);
        assert!(!denied.message_attempted);
    }
    let calls = server.calls.lock().await;
    assert_eq!(calls.tokens.len(), 2);
    assert_eq!(calls.messages.len(), 1);
}

#[tokio::test]
async fn repeated_token_failures_back_off_and_success_resets_the_backoff() {
    let limited = (StatusCode::OK, json!({"code":100001}));
    let server = FakeApi::start(Calls {
        token_responses: VecDeque::from([
            limited.clone(),
            limited.clone(),
            (
                StatusCode::OK,
                json!({"access_token":"recovered","expires_in":120}),
            ),
            limited,
        ]),
        ..Calls::default()
    })
    .await;
    let client = ApiClient::new(&server.config).unwrap();
    let first = client
        .send_once(APP, "group", "group", "in", 1, "hello")
        .await;
    let first_delay = first.meta.retry_after.unwrap();
    assert!((Duration::from_secs(30)..=Duration::from_secs(36)).contains(&first_delay));
    advance_clock(first_delay + Duration::from_secs(1)).await;
    let second = client
        .send_once(APP, "group", "group", "in", 1, "hello")
        .await;
    let second_delay = second.meta.retry_after.unwrap();
    assert!((Duration::from_secs(60)..=Duration::from_secs(72)).contains(&second_delay));
    advance_clock(second_delay + Duration::from_secs(1)).await;
    let recovered = client
        .send_once(APP, "group", "group", "in", 1, "hello")
        .await;
    assert_eq!(recovered.outcome, SendOutcome::Accepted);
    client
        .invalidate_token(APP, recovered.token_generation.unwrap())
        .await;
    let next_failure = client
        .send_once(APP, "group", "group", "in", 2, "hello")
        .await;
    assert!(
        (Duration::from_secs(30)..=Duration::from_secs(36))
            .contains(&next_failure.meta.retry_after.unwrap())
    );
    assert_eq!(server.calls.lock().await.tokens.len(), 4);
}

#[tokio::test]
async fn cancelling_a_refresh_leaves_an_attempt_guard_for_waiters() {
    let wait = Arc::new(Notify::new());
    let server = FakeApi::start(Calls {
        token_wait: Some(wait.clone()),
        ..Calls::default()
    })
    .await;
    let client = Arc::new(ApiClient::new(&server.config).unwrap());
    let sending = client.clone();
    let task = tokio::spawn(async move {
        sending
            .send_once(APP, "group", "group", "in", 1, "hello")
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.calls.lock().await.tokens.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let blocked = client
        .send_once(APP, "group", "group", "in", 1, "hello")
        .await;
    assert_eq!(blocked.class, ErrorClass::Transient);
    assert!(!blocked.message_attempted);
    assert_eq!(server.calls.lock().await.tokens.len(), 1);
    wait.notify_one();
    advance_clock(Duration::from_secs(31)).await;
    assert_eq!(
        client
            .send_once(APP, "group", "group", "in", 1, "hello")
            .await
            .outcome,
        SendOutcome::Accepted
    );
    assert_eq!(server.calls.lock().await.tokens.len(), 2);
}

#[tokio::test]
async fn a_truncated_http_success_stays_unknown_and_keeps_transport_semantics() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let server = FakeApi::start(Calls::default()).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let broken = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            assert!(stream.read_buf(&mut request).await.unwrap() > 0);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"id\":").await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let mut config = server.config.clone();
    config.api_base = Some(format!("http://{address}"));
    let client = ApiClient::new(&config).unwrap();
    let result = client
        .send_once(APP, "group", "group", "in", 1, "hello")
        .await;
    assert_eq!(result.meta.http_status, Some(200));
    assert_eq!(result.outcome, SendOutcome::Unknown);
    assert_eq!(result.class, ErrorClass::Transient);
    assert!(result.message_attempted);
    assert!(!result.meta.malformed);
    broken.await.unwrap();
}

#[tokio::test]
async fn explicit_formats_use_mutually_exclusive_bodies_in_both_scenes() {
    use ik_qqbot::message::Message;
    let server = FakeApi::start(Calls::default()).await;
    let client = ApiClient::new(&server.config).unwrap();
    for scene in ["group", "c2c"] {
        for message in [
            Message::Plain("普通 ^_^"),
            Message::Markdown("**标题** [外链](https://example.com)"),
        ] {
            let attempt = client
                .send_message_once(APP, scene, "target", "incoming", 2, message)
                .await;
            assert_eq!(attempt.outcome, SendOutcome::Accepted);
        }
    }
    let calls = server.calls.lock().await;
    assert_eq!(calls.messages.len(), 4);
    for (i, (collection, _, _, body)) in calls.messages.iter().enumerate() {
        assert_eq!(collection, if i < 2 { "groups" } else { "users" });
        assert_eq!(body["msg_id"], "incoming");
        assert_eq!(body["msg_seq"], 2);
        if i % 2 == 0 {
            assert_eq!(
                *body,
                json!({"msg_type":0,"content":"普通 ^_^","msg_id":"incoming","msg_seq":2})
            );
        } else {
            assert_eq!(
                *body,
                json!({"msg_type":2,"markdown":{"content":"**标题** [外链](https://example.com)"},"msg_id":"incoming","msg_seq":2})
            );
        }
    }
}
