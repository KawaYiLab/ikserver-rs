//! QQ OpenAPI：按 AppID 缓存凭据；所有 HTTP 调用必须在数据库事务之外。
use std::{collections::BTreeMap, sync::Arc, time::Duration};

use ik_kernel::{
    Problem,
    config::{Qqbot, Secret},
};
use reqwest13::{Client, StatusCode, Url};
use serde_json::{Value, json};
use tokio::{sync::Mutex, time::Instant};

const MAX_BODY: usize = 64 * 1024;

struct Token {
    value: String,
    expires_at: Instant,
}

struct App {
    secret: Secret,
    token: Mutex<Option<Token>>,
}

pub struct ApiClient {
    http: Client,
    token_url: Url,
    api_base: Url,
    apps: BTreeMap<String, App>,
}

/// 不保留上游响应正文、URL、token 或 openid，避免任务错误日志泄露凭据与身份。
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("QQ transport failed")]
    Transport,
    #[error("QQ response is invalid")]
    Protocol,
    #[error("QQ app is not configured")]
    AppMissing,
    #[error("QQ API failed (HTTP {status}, code {code})")]
    Api { status: u16, code: i64 },
}

impl SendError {
    fn token_expired(&self) -> bool {
        matches!(
            self,
            Self::Api { status: 401, .. }
                | Self::Api {
                    code: 40014 | 40001 | 11244,
                    ..
                }
        )
    }

    fn url_rejected(&self) -> bool {
        matches!(
            self,
            Self::Api {
                code: 304003 | 40034028,
                ..
            }
        )
    }

    pub fn reason(&self) -> &'static str {
        if self.token_expired() {
            return "qq_token_rejected";
        }
        if self.url_rejected() {
            return "qq_url_rejected";
        }
        match self {
            Self::Transport => "qq_transport_failed",
            Self::Protocol => "qq_invalid_response",
            Self::AppMissing => "qq_app_missing",
            Self::Api { status: 429, .. } => "qq_rate_limited",
            Self::Api {
                status: 500..=599, ..
            } => "qq_unavailable",
            Self::Api { .. } => "qq_message_rejected",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport
                | Self::Api {
                    status: 429 | 500..=599,
                    ..
                }
        )
    }
}

impl ApiClient {
    pub fn new(config: &Qqbot) -> Result<Self, Problem> {
        config.validate().map_err(|_| Problem::internal())?;
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| Problem::internal())?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let http = Client::builder()
            .tls_backend_preconfigured(tls)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest13::redirect::Policy::none())
            .build()
            .map_err(|_| Problem::internal())?;
        let token_url = endpoint(&config.token_url)?;
        let api_base = endpoint(config.api_base())?;
        Ok(Self {
            http,
            token_url,
            api_base,
            apps: config
                .apps
                .values()
                .map(|app| {
                    (
                        app.app_id.clone(),
                        App {
                            secret: app.secret.clone(),
                            token: Mutex::new(None),
                        },
                    )
                })
                .collect(),
        })
    }

    async fn token(&self, app_id: &str, rejected: Option<&str>) -> Result<String, SendError> {
        let app = self.apps.get(app_id).ok_or(SendError::AppMissing)?;
        // 每个 AppID 独立串行刷新，避免并发请求同时刷新。不是数据库锁。
        let mut cached = app.token.lock().await;
        if let Some(token) = cached.as_ref() {
            let fresh = token.expires_at > Instant::now() + Duration::from_secs(300);
            let replaced =
                rejected.is_some_and(|old| old != token.value) && token.expires_at > Instant::now();
            if (rejected.is_none() && fresh) || replaced {
                return Ok(token.value.clone());
            }
        }
        let response = self
            .http
            .post(self.token_url.clone())
            .json(&json!({"appId":app_id,"clientSecret":app.secret.expose()}))
            .send()
            .await
            .map_err(|_| SendError::Transport)?;
        let body = response_json(response).await?;
        let value = body
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(SendError::Protocol)?
            .to_owned();
        let expires = number(&body["expires_in"])
            .filter(|n| *n > 0 && *n <= 86400)
            .ok_or(SendError::Protocol)? as u64;
        *cached = Some(Token {
            value: value.clone(),
            expires_at: Instant::now() + Duration::from_secs(expires),
        });
        Ok(value)
    }

    /// 被动回复的 msg_id 与 msg_seq 由持久化 turn 提供，重试沿用同一组值。
    /// 调用方必须在被动回复的绝对截止时刻取消此 future。
    pub async fn send(
        &self,
        app_id: &str,
        scene: &str,
        openid: &str,
        msg_id: &str,
        seq: i32,
        text: &str,
    ) -> Result<(), SendError> {
        self.send_rendered(app_id, scene, openid, msg_id, seq, text)
            .await
            .map(|_| ())
    }

    /// 返回平台实际接受的正文，供渠道保存上下文（URL 降级时与原稿可能不同）。
    pub async fn send_rendered(
        &self,
        app_id: &str,
        scene: &str,
        openid: &str,
        msg_id: &str,
        seq: i32,
        text: &str,
    ) -> Result<String, SendError> {
        let collection = match scene {
            "group" => "groups",
            "c2c" => "users",
            _ => return Err(SendError::Protocol),
        };
        if openid.is_empty() || msg_id.is_empty() || seq < 1 {
            return Err(SendError::Protocol);
        }
        let mut url = self.api_base.clone();
        url.path_segments_mut()
            .map_err(|_| SendError::Protocol)?
            .pop_if_empty()
            .extend(["v2", collection, openid, "messages"]);
        let mut token = self.token(app_id, None).await?;
        let mut text = text.to_owned();
        let mut refreshed = false;
        let mut stripped = false;
        loop {
            let response = self
                .http
                .post(url.clone())
                .header("Authorization", format!("QQBot {token}"))
                .header("X-Union-Appid", app_id)
                .json(&json!({"msg_type":0,"content":text,"msg_id":msg_id,"msg_seq":seq}))
                .send()
                .await
                .map_err(|_| SendError::Transport)?;
            match response_json(response).await {
                Ok(_) => return Ok(text),
                Err(error) if error.token_expired() && !refreshed => {
                    token = self.token(app_id, Some(&token)).await?;
                    refreshed = true;
                }
                Err(error) if error.url_rejected() && !stripped => {
                    let clean = crate::text::clean(&text, "strip");
                    if clean.trim().is_empty() || clean == text {
                        return Err(error);
                    }
                    text = clean;
                    stripped = true;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

fn endpoint(value: &str) -> Result<Url, Problem> {
    let url = Url::parse(value).map_err(|_| Problem::internal())?;
    // HTTP 只供本地桩服务器；生产凭据与消息必须通过 HTTPS 传输。
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || url.scheme() == "http" && local)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Problem::internal());
    }
    Ok(url)
}

fn number(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

async fn response_json(mut response: reqwest13::Response) -> Result<Value, SendError> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY as u64)
    {
        return Err(SendError::Protocol);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| SendError::Transport)? {
        if bytes.len() + chunk.len() > MAX_BODY {
            return Err(SendError::Protocol);
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) if !status.is_success() => {
            return Err(SendError::Api {
                status: status.as_u16(),
                code: 0,
            });
        }
        Err(_) => return Err(SendError::Protocol),
    };
    // 腾讯的 token 接口出错也可能 HTTP 200，必须同时检查业务 code。
    let code = if let Some(value) = body.get("code") {
        number(value).ok_or(SendError::Protocol)?
    } else {
        0
    };
    if !status.is_success() || code != 0 {
        return Err(SendError::Api {
            status: status.as_u16(),
            code,
        });
    }
    if !body.is_object() || status == StatusCode::NO_CONTENT {
        return Err(SendError::Protocol);
    }
    Ok(body)
}
