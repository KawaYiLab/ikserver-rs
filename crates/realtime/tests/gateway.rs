//! 实时网关端到端测试：真实 TCP + WebSocket 客户端。

// clippy.toml 的测试豁免只覆盖 #[test] 函数本身，不覆盖这里的辅助函数。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::FromRef;
use futures_util::{SinkExt, StreamExt};
use ik_auth::dto::{ClientKind, RegisterRequest, SendCodeRequest};
use ik_auth::service::{self, RequestMeta, Resolution};
use ik_auth::{AuthSettings, AuthState};
use ik_kernel::Snowflake;
use ik_kernel::bus::{Audience, Event, EventBus, LocalBus};
use ik_kernel::mail::{MailError, Mailer, OutgoingMail};
use ik_realtime::RealtimeState;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderName;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");
const ORIGIN: &str = "https://interk.net";

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Default)]
struct CapturingMailer {
    sent: Mutex<Vec<OutgoingMail>>,
}

#[async_trait]
impl Mailer for CapturingMailer {
    async fn send(&self, mail: OutgoingMail) -> Result<(), MailError> {
        self.sent.lock().unwrap().push(mail);
        Ok(())
    }
}

impl CapturingMailer {
    fn last_code(&self) -> String {
        let sent = self.sent.lock().unwrap();
        let text: Vec<char> = sent.last().unwrap().text.chars().collect();
        text.windows(6)
            .find(|w| w.iter().all(char::is_ascii_digit))
            .map(|w| w.iter().collect())
            .unwrap()
    }
}

#[derive(Clone)]
struct TestState {
    auth: AuthState,
    realtime: RealtimeState,
}

impl FromRef<TestState> for AuthState {
    fn from_ref(state: &TestState) -> Self {
        state.auth.clone()
    }
}

impl FromRef<TestState> for RealtimeState {
    fn from_ref(state: &TestState) -> Self {
        state.realtime.clone()
    }
}

struct Server {
    addr: SocketAddr,
    bus: Arc<LocalBus>,
    auth: AuthState,
    mailer: Arc<CapturingMailer>,
}

async fn start(db: PgPool) -> Server {
    let bus = Arc::new(LocalBus::new(64));
    let mailer = Arc::new(CapturingMailer::default());
    let auth = AuthState::new(
        db,
        Arc::new(Snowflake::new(1).unwrap()),
        mailer.clone(),
        AuthSettings {
            cookie_name: "ik_session".into(),
            cookie_secure: false,
            session_ttl: chrono::Duration::days(30),
            session_max_lifetime: chrono::Duration::days(180),
            registration_open: true,
            trusted_proxy_hops: 0,
            code_key: vec![7u8; 32],
        },
        bus.clone(),
    );
    let realtime = RealtimeState::start(&bus, vec![ORIGIN.to_owned()]).unwrap();
    let (router, _) = ik_realtime::router::<TestState>().split_for_parts();
    let app = router.with_state(TestState {
        auth: auth.clone(),
        realtime,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server {
        addr,
        bus,
        auth,
        mailer,
    }
}

/// 不需要数据库的场景：连接池指向一个连不上的地址。
async fn start_without_db() -> Server {
    let db = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(300))
        .connect_lazy("postgres://test:test@127.0.0.1:1/test")
        .unwrap();
    start(db).await
}

impl Server {
    async fn connect(&self, headers: &[(&str, &str)]) -> Result<Ws, WsError> {
        let mut request = format!("ws://{}/v1/realtime", self.addr)
            .into_client_request()
            .unwrap();
        for (name, value) in headers {
            request.headers_mut().insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        tokio_tungstenite::connect_async(request)
            .await
            .map(|(ws, _)| ws)
    }

    fn publish(&self, kind: &'static str, audience: Audience) {
        self.bus
            .publish(Event::new(kind, audience, &serde_json::json!({ "ok": true })).unwrap());
    }

    async fn register(&self, email: &str) -> String {
        let meta = RequestMeta {
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            user_agent: None,
        };
        let request: SendCodeRequest =
            serde_json::from_value(serde_json::json!({ "email": email, "purpose": "register" }))
                .unwrap();
        service::send_code(&self.auth, request, &meta)
            .await
            .unwrap();
        service::register(
            &self.auth,
            RegisterRequest {
                email: email.into(),
                code: self.mailer.last_code(),
                password: "hunter22".into(),
                client: ClientKind::App,
            },
            &meta,
        )
        .await
        .unwrap()
        .token
    }
}

async fn next_frame(ws: &mut Ws) -> Message {
    tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("5 秒内没有收到帧")
        .expect("连接已结束")
        .unwrap()
}

async fn next_json(ws: &mut Ws) -> serde_json::Value {
    loop {
        if let Message::Text(text) = next_frame(ws).await {
            return serde_json::from_str(text.as_str()).unwrap();
        }
    }
}

async fn send(ws: &mut Ws, frame: serde_json::Value) {
    ws.send(Message::Text(frame.to_string().into()))
        .await
        .unwrap();
}

fn http_status(error: WsError) -> u16 {
    match error {
        WsError::Http(response) => response.status().as_u16(),
        other => panic!("预期 HTTP 拒绝，实际 {other:?}"),
    }
}

#[tokio::test]
async fn guest_connection_protocol() {
    let server = start_without_db().await;
    let mut ws = server.connect(&[("origin", ORIGIN)]).await.unwrap();

    let hello = next_json(&mut ws).await;
    assert_eq!(hello["op"], "hello");
    assert_eq!(hello["d"]["heartbeatSec"], 30);
    assert!(hello["d"].get("userId").is_none());

    send(&mut ws, serde_json::json!({ "op": "ping", "seq": 1 })).await;
    assert_eq!(
        next_json(&mut ws).await,
        serde_json::json!({ "op": "ack", "seq": 1 })
    );

    send(
        &mut ws,
        serde_json::json!({ "op": "subscribe", "seq": 2, "t": "post", "d": { "postId": "9" } }),
    )
    .await;
    assert_eq!(next_json(&mut ws).await["seq"], 2);
    send(
        &mut ws,
        serde_json::json!({ "op": "subscribe", "seq": 3, "t": "dm", "d": {} }),
    )
    .await;
    let error = next_json(&mut ws).await;
    assert_eq!(error["op"], "error");
    assert_eq!(error["seq"], 3);
    assert_eq!(error["d"]["code"], "CHANNEL_INVALID");

    send(&mut ws, serde_json::json!({ "op": "dance" })).await;
    assert_eq!(next_json(&mut ws).await["d"]["code"], "UNKNOWN_OP");
    ws.send(Message::Text("{not json".into())).await.unwrap();
    assert_eq!(next_json(&mut ws).await["d"]["code"], "INVALID_FRAME");

    // 游客能收到广播和自己订阅的公开频道，收不到发给某个用户的事件。
    server.publish("user.only", Audience::User(1));
    server.publish("post.published", Audience::Broadcast);
    server.publish("comment.created", Audience::Channel(Arc::from("post:9")));
    server.publish("comment.created", Audience::Channel(Arc::from("post:10")));
    let event = next_json(&mut ws).await;
    assert_eq!(event["op"], "event");
    assert_eq!(event["t"], "post.published");
    assert_eq!(event["d"]["ok"], true);
    let event = next_json(&mut ws).await;
    assert_eq!(event["t"], "comment.created");

    send(
        &mut ws,
        serde_json::json!({ "op": "unsubscribe", "seq": 4, "t": "post", "d": { "postId": "9" } }),
    )
    .await;
    assert_eq!(next_json(&mut ws).await["seq"], 4);
    server.publish("comment.created", Audience::Channel(Arc::from("post:9")));
    server.publish("marker", Audience::Broadcast);
    assert_eq!(next_json(&mut ws).await["t"], "marker");
}

#[tokio::test]
async fn handshake_rejections() {
    let server = start_without_db().await;
    // 跨站页面发起的（Cookie 或游客）连接被拒绝，防止跨站 WebSocket 劫持。
    let error = server
        .connect(&[("origin", "https://evil.example")])
        .await
        .unwrap_err();
    assert_eq!(http_status(error), 403);

    // 不是 WebSocket 握手。
    let mut tcp = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    tcp.write_all(b"GET /v1/realtime HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    tcp.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 426"), "{response}");
    assert!(response.contains("WEBSOCKET_REQUIRED"));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn logged_in_connection_receives_own_events_and_closes_on_logout(db: PgPool) {
    let server = start(db).await;
    let token = server.register("alice@example.com").await;
    let bearer = format!("Bearer {token}");

    let error = server
        .connect(&[("authorization", "Bearer not-a-real-token")])
        .await
        .unwrap_err();
    assert_eq!(http_status(error), 401, "凭据失效时不降级为游客");

    // Bearer 连接不受来源限制（浏览器不会自动携带）。
    let mut ws = server
        .connect(&[
            ("authorization", bearer.as_str()),
            ("origin", "https://evil.example"),
        ])
        .await
        .unwrap();
    let hello = next_json(&mut ws).await;
    let Resolution::Active(principal) = service::resolve(&server.auth, &token).await.unwrap()
    else {
        panic!("会话应当有效");
    };
    assert_eq!(hello["d"]["userId"], principal.user_id.to_string());

    server.publish(
        "notification.created",
        Audience::User(principal.user_id.get()),
    );
    server.publish("not.mine", Audience::User(principal.user_id.get() + 1));
    server.publish("marker", Audience::Broadcast);
    assert_eq!(next_json(&mut ws).await["t"], "notification.created");
    assert_eq!(next_json(&mut ws).await["t"], "marker");

    // 登出后网关立即复查并以 4001 关闭，不用等 30 秒的定时复查。
    service::logout(&server.auth, &principal).await.unwrap();
    match next_frame(&mut ws).await {
        Message::Close(Some(frame)) => {
            assert_eq!(frame.code, CloseCode::from(4001));
            assert_eq!(frame.reason.as_str(), "session ended");
        }
        other => panic!("预期关闭帧，实际 {other:?}"),
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn sixth_connection_evicts_the_oldest(db: PgPool) {
    let server = start(db).await;
    let token = server.register("bob@example.com").await;
    let bearer = format!("Bearer {token}");
    let mut connections = Vec::new();
    for _ in 0..6 {
        let mut ws = server
            .connect(&[("authorization", bearer.as_str())])
            .await
            .unwrap();
        next_json(&mut ws).await;
        connections.push(ws);
    }
    match next_frame(&mut connections[0]).await {
        Message::Close(Some(frame)) => assert_eq!(frame.code, CloseCode::from(4000)),
        other => panic!("预期关闭帧，实际 {other:?}"),
    }
}

#[tokio::test]
async fn client_close_gets_a_close_reply() {
    let server = start_without_db().await;
    let mut ws = server.connect(&[]).await.unwrap();
    next_json(&mut ws).await;
    ws.close(None).await.unwrap();
    // 服务端必须回关闭帧完成握手，而不是直接断开 TCP。
    assert!(matches!(next_frame(&mut ws).await, Message::Close(_)));
    assert!(
        tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .is_none()
    );
}
