//! 真实的 HTTP 客户端对着本地桩服务器跑：请求头、路径、retcode、Set-Cookie 的处理与协议文档一致
//! （docs/business-rules/auth-account.md MHY-P1～P5）。没有米游社测试账号，联调前以此为准。

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use http::{HeaderMap, StatusCode};
use ik_mihoyo::device::new_device;
use ik_mihoyo::upstream::{HttpApi, MihoyoApi, QrStatus, UpstreamError};
use parking_lot::Mutex;

#[derive(Default)]
struct Stub {
    device_ids: Mutex<Vec<String>>,
    polls: AtomicUsize,
    cookie_header: Mutex<Option<String>>,
}

fn json(value: serde_json::Value) -> impl IntoResponse {
    (StatusCode::OK, axum::Json(value))
}

async fn create(State(stub): State<Arc<Stub>>, headers: HeaderMap) -> impl IntoResponse {
    assert_eq!(headers["x-rpc-app_id"], "bll8iq97cem8");
    assert_eq!(headers["x-rpc-client_type"], "4");
    assert_eq!(
        headers["x-rpc-device_name"],
        "%E7%BB%B3%E7%BD%91%E8%AE%BA%E5%9D%9B"
    );
    assert_eq!(headers["x-rpc-device_model"], "Chrome%20124");
    assert_eq!(headers["referer"], "https://user.mihoyo.com/");
    assert_eq!(headers["origin"], "https://user.mihoyo.com");
    stub.device_ids
        .lock()
        .push(headers["x-rpc-device_id"].to_str().unwrap().to_owned());
    json(serde_json::json!({
        "retcode": 0,
        "message": "OK",
        "data": { "url": "https://user.mihoyo.com/qr_code_in_game.html?app_id=x&ticket=T1&expire=4102444800", "ticket": "T1" }
    }))
}

async fn query(
    State(stub): State<Arc<Stub>>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::response::Response {
    assert_eq!(body["ticket"], "T1");
    assert_eq!(
        Some(headers["x-rpc-device_id"].to_str().unwrap()),
        stub.device_ids.lock().first().map(String::as_str),
        "轮询的 device_id 必须与创建时一致"
    );
    match stub.polls.fetch_add(1, Ordering::SeqCst) {
        0 => json(serde_json::json!({ "retcode": 0, "data": { "status": "Created" } }))
            .into_response(),
        1 => json(serde_json::json!({ "retcode": 0, "data": { "status": "Scanned" } }))
            .into_response(),
        2 => json(serde_json::json!({ "retcode": -3505, "message": "cancelled", "data": null }))
            .into_response(),
        3 => json(serde_json::json!({ "retcode": -3501, "message": "expired", "data": null }))
            .into_response(),
        4 => (
            // 两个 Set-Cookie 头（数组形式的响应头会互相覆盖，所以用 append）。
            {
                let mut headers = HeaderMap::new();
                headers.append(
                    "set-cookie",
                    "ltoken_v2=v2_secret; Path=/; Domain=.mihoyo.com; HttpOnly"
                        .parse()
                        .unwrap(),
                );
                headers.append("set-cookie", "ltuid_v2=12345; Path=/".parse().unwrap());
                headers
            },
            json(serde_json::json!({
                "retcode": 0,
                "data": { "status": "Confirmed", "user_info": { "aid": " 12345 ", "mid": "abc" } }
            })),
        )
            .into_response(),
        5 => json(
            serde_json::json!({ "retcode": 0, "data": { "status": "Confirmed", "user_info": {} } }),
        )
        .into_response(),
        _ => json(serde_json::json!({ "retcode": -100, "message": "boom", "data": null }))
            .into_response(),
    }
}

async fn roles(
    State(stub): State<Arc<Stub>>,
    headers: HeaderMap,
    uri: http::Uri,
) -> impl IntoResponse {
    assert_eq!(uri.query(), Some("game_biz=nap_cn"));
    let cookie = headers
        .get("cookie")
        .map(|v| v.to_str().unwrap().to_owned());
    *stub.cookie_header.lock() = cookie.clone();
    if cookie.as_deref() == Some("bad=1") {
        return json(serde_json::json!({ "retcode": -100, "message": "未登录", "data": null }));
    }
    json(serde_json::json!({
        "retcode": 0,
        "data": { "list": [
            { "game_uid": "10000001", "nickname": "邦布", "level": 45, "region": "prod_gf_cn", "region_name": "新艾利都", "is_chosen": true }
        ] }
    }))
}

#[tokio::test]
async fn http_client_follows_the_protocol() {
    let stub = Arc::new(Stub::default());
    let app = Router::new()
        .route("/account/ma-cn-passport/web/createQRLogin", post(create))
        .route(
            "/account/ma-cn-passport/web/queryQRLoginStatus",
            post(query),
        )
        .route("/binding/api/getUserGameRolesByCookie", get(roles))
        .with_state(stub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let api = HttpApi::new(&base, &format!("{base}/"), Duration::from_secs(5)).unwrap();
    let device = new_device(Some(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
    ))
    .unwrap();

    let code = api.create_qr(&device).await.unwrap();
    assert_eq!(code.ticket, "T1");
    assert_eq!(code.expires_at.unwrap().timestamp(), 4_102_444_800);

    assert_eq!(
        api.query_qr("T1", &device).await.unwrap(),
        QrStatus::Created
    );
    assert_eq!(
        api.query_qr("T1", &device).await.unwrap(),
        QrStatus::Scanned
    );
    assert_eq!(
        api.query_qr("T1", &device).await.unwrap(),
        QrStatus::Cancelled
    );
    assert_eq!(
        api.query_qr("T1", &device).await.unwrap(),
        QrStatus::Expired
    );
    let QrStatus::Confirmed(confirmed) = api.query_qr("T1", &device).await.unwrap() else {
        panic!("应当是已确认");
    };
    assert_eq!(confirmed.aid, "12345");
    assert_eq!(confirmed.mid.as_deref(), Some("abc"));
    assert!(matches!(
        api.query_qr("T1", &device).await,
        Err(UpstreamError::Malformed(_))
    ));
    assert!(matches!(
        api.query_qr("T1", &device).await,
        Err(UpstreamError::Retcode { retcode: -100, .. })
    ));

    let roles = api.game_roles(&confirmed.cookies).await.unwrap();
    assert_eq!(roles[0].game_uid, "10000001");
    assert!(roles[0].is_chosen);
    assert_eq!(
        stub.cookie_header.lock().as_deref(),
        Some("ltoken_v2=v2_secret; ltuid_v2=12345")
    );
    let bad = ik_mihoyo::upstream::Cookies(vec![("bad".into(), "1".into())]);
    assert!(matches!(
        api.game_roles(&bad).await,
        Err(UpstreamError::Retcode { .. })
    ));
}
