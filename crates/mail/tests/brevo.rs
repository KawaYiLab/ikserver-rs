//! 通过公开的 `build()` 走完整路径：配置 → 调度 → Brevo 通道 → 本地假接口。

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use ik_kernel::config::{Mail, MailProvider, Secret};
use ik_kernel::mail::OutgoingMail;
use parking_lot::Mutex;
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<(HeaderMap, Value)>>>);

async fn stub(status: StatusCode) -> (String, Captured) {
    let captured = Captured::default();
    let app = Router::new()
        .route(
            "/v3/smtp/email",
            post(
                move |State(captured): State<Captured>,
                      headers: HeaderMap,
                      axum::Json(body): axum::Json<Value>| async move {
                    captured.0.lock().push((headers, body));
                    (
                        status,
                        axum::Json(json!({ "messageId": "<x@brevo>", "message": "nope" })),
                    )
                },
            ),
        )
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v3/", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, captured)
}

fn config(api_url: String) -> Mail {
    let mut mail = Mail {
        provider: MailProvider::Dual,
        ..Mail::default()
    };
    mail.brevo.api_key = Secret::new("xkeysib-test");
    mail.brevo.api_url = api_url;
    mail.brevo.from = "\"绳网\" <noreply@example.com>".to_owned();
    mail.brevo.reply_to = "help@example.com".to_owned();
    mail
}

fn code_mail(to: &str) -> OutgoingMail {
    OutgoingMail {
        to: to.to_owned(),
        subject: "【123456】登录验证码 - 绳网".to_owned(),
        text: "您的登录验证码是 123456".to_owned(),
        html: Some("<b>123456</b>".to_owned()),
    }
}

#[tokio::test]
async fn sends_through_brevo_api() {
    let (url, captured) = stub(StatusCode::CREATED).await;
    let mailer = ik_mail::build(&config(url)).unwrap();
    mailer.send(code_mail("alice@gmail.com")).await.unwrap();

    let calls = captured.0.lock().clone();
    assert_eq!(calls.len(), 1);
    let (headers, body) = &calls[0];
    assert_eq!(headers["api-key"], "xkeysib-test");
    assert_eq!(
        body,
        &json!({
            "sender": { "name": "绳网", "email": "noreply@example.com" },
            "to": [{ "email": "alice@gmail.com" }],
            "subject": "【123456】登录验证码 - 绳网",
            "textContent": "您的登录验证码是 123456",
            "htmlContent": "<b>123456</b>",
            "replyTo": { "email": "help@example.com" },
        })
    );
}

#[tokio::test]
async fn upstream_errors_fail_the_send() {
    let (url, captured) = stub(StatusCode::UNAUTHORIZED).await;
    let mailer = ik_mail::build(&config(url)).unwrap();
    // 国内邮箱的主通道（SMTP）没配置，也走 Brevo；失败时没有备用。
    let error = mailer.send(code_mail("bob@qq.com")).await.unwrap_err();
    assert!(error.0.contains("401"), "{error}");
    assert_eq!(captured.0.lock().len(), 1);
}

#[test]
fn bad_sender_fails_at_startup() {
    let mut mail = config("http://127.0.0.1:1/".to_owned());
    mail.brevo.from = "绳网".to_owned();
    assert!(ik_mail::build(&mail).is_err());
}
