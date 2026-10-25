#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use ik_kernel::config::{Qqbot, QqbotApp, Secret};
use ik_qqbot::client::{ApiClient, SendError};
use serde_json::{Value, json};
use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};

const APP: &str = "102123456";

#[derive(Default)]
struct Calls {
    tokens: Vec<Value>,
    messages: Vec<(String, String, String, Value)>,
    token_responses: VecDeque<(StatusCode, Value)>,
    message_responses: VecDeque<(StatusCode, Value)>,
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
) -> (StatusCode, Json<Value>) {
    let mut calls = state.lock().await;
    calls.tokens.push(body);
    let (status, body) = calls.token_responses.pop_front().unwrap_or_else(|| {
        (
            StatusCode::OK,
            json!({"access_token":"cached-token","expires_in":7200}),
        )
    });
    (status, Json(body))
}

async fn message(
    State(state): State<Arc<Mutex<Calls>>>,
    Path((collection, openid)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
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
    let (status, body) = calls
        .message_responses
        .pop_front()
        .unwrap_or_else(|| (StatusCode::OK, json!({"id":"sent"})));
    (status, Json(body))
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
async fn near_expiry_refresh_accepts_the_same_platform_token() {
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
    let calls = server.calls.lock().await;
    assert_eq!(calls.tokens.len(), 2);
    assert!(
        calls
            .messages
            .iter()
            .all(|(_, _, auth, _)| auth == "QQBot same")
    );
}

#[tokio::test]
async fn url_rejection_strips_links_and_reuses_the_same_sequence() {
    for code in [304003, 40034028] {
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
