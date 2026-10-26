//! QQ OpenAPI. Platform IO runs outside database transactions.
mod response;

pub use response::{ApiEndpoint, ApiMetadata, ErrorClass, SendAttempt, SendOutcome};

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use ik_kernel::{
    Problem,
    config::{Qqbot, Secret},
};
use reqwest13::{Client, Url};
use ring::rand::SecureRandom;
use serde_json::json;
use tokio::{sync::Mutex, time::Instant};

use crate::message::Message;
use response::{number, read_response};

// Local refresh policy. QQ returns the same credential until its final 60 seconds:
// https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/access-token.html
const REFRESH_AHEAD: Duration = Duration::from_secs(300);
const REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const EXPIRY_MARGIN: Duration = Duration::from_secs(5);
const ERROR_COOLDOWN: Duration = Duration::from_secs(300);

struct Token {
    value: String,
    expires_at: Instant,
    fetched_at: Instant,
    generation: u64,
    rejected: bool,
}

#[derive(Default)]
struct TokenState {
    cached: Option<Token>,
    generation: u64,
    retry_not_before: Option<Instant>,
    failures: u32,
    last_failure: Option<SendAttempt>,
    fallback_allowed: bool,
}

struct TokenLease {
    value: String,
    generation: u64,
}

impl TokenState {
    fn usable(&self, now: Instant) -> Option<TokenLease> {
        self.cached
            .as_ref()
            .filter(|token| !token.rejected && token.expires_at > now + EXPIRY_MARGIN)
            .map(|token| TokenLease {
                value: token.value.clone(),
                generation: token.generation,
            })
    }
}

struct App {
    secret: Secret,
    token: Mutex<TokenState>,
}

pub struct ApiClient {
    http: Client,
    token_url: Url,
    api_base: Url,
    apps: BTreeMap<String, App>,
}

/// Compatibility error for `send`/`send_rendered`; durable delivery uses `SendAttempt`.
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("QQ transport failed")]
    Transport,
    #[error("QQ response is invalid")]
    Protocol,
    #[error("QQ app is not configured")]
    AppMissing,
    #[error("QQ message was accepted for asynchronous processing")]
    Pending,
    #[error("QQ API failed (HTTP {status}, code {code})")]
    Api { status: u16, code: i64 },
}

impl SendError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Transport => "qq_transport_failed",
            Self::Protocol => "qq_invalid_response",
            Self::AppMissing => "qq_app_missing",
            Self::Pending => "qq_pending",
            Self::Api { status: 401, .. }
            | Self::Api {
                code: 40014 | 40001 | 11244 | 11201 | 40012001,
                ..
            } => "qq_token_rejected",
            Self::Api {
                code: 304003 | 40034028 | 40054010,
                ..
            } => "qq_url_rejected",
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
        let token_url = endpoint(&config.token_url)?;
        let api_base = endpoint(config.api_base())?;
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
        let mut builder = Client::builder()
            .tls_backend_preconfigured(tls)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest13::redirect::Policy::none());
        // Loopback endpoints are the supported local HTTP test transport, never a proxy target.
        if local(&token_url) && local(&api_base) {
            builder = builder.no_proxy();
        }
        let http = builder.build().map_err(|_| Problem::internal())?;
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
                            token: Mutex::new(TokenState::default()),
                        },
                    )
                })
                .collect(),
        })
    }

    /// Reject only the generation used by this request; a late 401 cannot invalidate a newer token.
    /// This does not fetch a token. The next `send_once` acquires one, respecting failed-refresh cooldown.
    pub async fn invalidate_token(&self, app_id: &str, generation: u64) -> bool {
        let Some(app) = self.apps.get(app_id) else {
            return false;
        };
        let mut state = app.token.lock().await;
        if let Some(token) = state
            .cached
            .as_mut()
            .filter(|token| token.generation == generation)
        {
            token.rejected = true;
            true
        } else {
            false
        }
    }

    async fn token(&self, app_id: &str) -> Result<TokenLease, SendAttempt> {
        let app = self
            .apps
            .get(app_id)
            .ok_or_else(|| SendAttempt::local(ApiEndpoint::Token, ErrorClass::AppMissing))?;
        // One in-flight refresh and one shared failure/cooldown per AppID, including cancellation.
        let mut state = app.token.lock().await;
        let now = Instant::now();
        if let Some(until) = state.retry_not_before.filter(|until| *until > now) {
            if state.fallback_allowed
                && let Some(token) = state.usable(now)
            {
                return Ok(token);
            }
            let mut failure = state
                .last_failure
                .clone()
                .unwrap_or_else(|| SendAttempt::local(ApiEndpoint::Token, ErrorClass::Transient));
            failure.meta.retry_after = Some(until - now);
            return Err(failure);
        }
        if let Some(token) = state.usable(now)
            && state.cached.as_ref().is_some_and(|cached| {
                cached.expires_at > now + REFRESH_AHEAD
                    || now.duration_since(cached.fetched_at) < REFRESH_INTERVAL
            })
        {
            return Ok(token);
        }
        // If this future is cancelled, waiting requests still observe the attempt guard.
        state.retry_not_before = Some(now + REFRESH_INTERVAL);
        state.fallback_allowed = true;
        state.last_failure = Some(SendAttempt::local(
            ApiEndpoint::Token,
            ErrorClass::Transient,
        ));
        let fetched = self.fetch_token(app_id, &app.secret).await;
        let now = Instant::now();
        let failure = match fetched {
            Ok((value, expires, mut attempt)) => {
                if state
                    .cached
                    .as_ref()
                    .is_some_and(|old| old.rejected && old.value == value)
                {
                    // A successful token endpoint call does not make a rejected token usable again.
                    attempt.outcome = SendOutcome::Rejected;
                    attempt.class = ErrorClass::Auth;
                    attempt.meta.retry_after = Some(REFRESH_INTERVAL);
                    attempt
                } else {
                    // A refresh returning the same credential is the same generation: a late
                    // rejection of that credential must still invalidate the current cache.
                    if !state.cached.as_ref().is_some_and(|old| old.value == value) {
                        state.generation = state.generation.wrapping_add(1).max(1);
                    }
                    let generation = state.generation;
                    state.cached = Some(Token {
                        value: value.clone(),
                        expires_at: now + Duration::from_secs(expires),
                        fetched_at: now,
                        generation,
                        rejected: false,
                    });
                    state.failures = 0;
                    state.retry_not_before = None;
                    state.last_failure = None;
                    state.fallback_allowed = false;
                    return Ok(TokenLease { value, generation });
                }
            }
            Err(failure) => failure,
        };
        state.failures = state.failures.saturating_add(1);
        let recoverable = matches!(
            failure.class,
            ErrorClass::Transient | ErrorClass::RateLimited
        ) || (failure.meta.malformed
            && failure
                .meta
                .http_status
                .is_some_and(|status| (200..300).contains(&status))
            && failure.meta.code.unwrap_or(0) == 0
            && failure.meta.err_code.unwrap_or(0) == 0);
        let delay = if recoverable {
            backoff(state.failures)
        } else if failure.class == ErrorClass::Auth {
            REFRESH_INTERVAL
        } else {
            ERROR_COOLDOWN
        }
        .max(failure.meta.retry_after.unwrap_or_default());
        let until = now.checked_add(delay).unwrap_or(now + ERROR_COOLDOWN);
        state.retry_not_before = Some(until);
        state.fallback_allowed = recoverable;
        let mut failure = failure;
        failure.meta.retry_after = Some(until - now);
        state.last_failure = Some(failure.clone());
        tracing::warn!(
            app_id,
            reason = failure.reason(),
            status = failure.meta.http_status,
            code = failure.meta.code,
            err_code = failure.meta.err_code,
            retry_secs = delay.as_secs(),
            "QQ token refresh failed; sharing cooldown"
        );
        if recoverable && let Some(token) = state.usable(now) {
            return Ok(token);
        }
        Err(failure)
    }

    async fn fetch_token(
        &self,
        app_id: &str,
        secret: &Secret,
    ) -> Result<(String, u64, SendAttempt), SendAttempt> {
        let response = self
            .http
            .post(self.token_url.clone())
            .json(&json!({"appId":app_id,"clientSecret":secret.expose()}))
            .send()
            .await
            .map_err(|_| SendAttempt::local(ApiEndpoint::Token, ErrorClass::Transient))?;
        let decoded = read_response(response, ApiEndpoint::Token).await;
        let mut attempt = decoded.attempt();
        if attempt.outcome != SendOutcome::Accepted {
            // An unconfirmed credential response never means a message was attempted or accepted.
            attempt.outcome = SendOutcome::Rejected;
            return Err(attempt);
        }
        let token = decoded.body.as_ref().and_then(|body| {
            let value = body
                .get("access_token")?
                .as_str()
                .filter(|v| !v.is_empty())?;
            let expires = number(body.get("expires_in")?).filter(|n| *n > 0 && *n <= 86400)?;
            Some((value.to_owned(), expires as u64))
        });
        match token {
            Some((value, expires)) => Ok((value, expires, attempt)),
            None => {
                // These two fields are actually needed to authenticate; other token fields are optional.
                attempt.outcome = SendOutcome::Rejected;
                attempt.class = ErrorClass::Unknown;
                attempt.meta.malformed = true;
                Err(attempt)
            }
        }
    }

    /// At most one message POST. No token-rejection or URL retry, and no content rewriting.
    /// The durable caller persists text/sequence and cancels this future at its passive-reply deadline.
    pub async fn send_once(
        &self,
        app_id: &str,
        scene: &str,
        openid: &str,
        msg_id: &str,
        seq: i32,
        text: &str,
    ) -> SendAttempt {
        self.send_message_once(app_id, scene, openid, msg_id, seq, Message::Plain(text))
            .await
    }

    /// Submit the persisted message type with mutually exclusive content fields.
    pub async fn send_message_once(
        &self,
        app_id: &str,
        scene: &str,
        openid: &str,
        msg_id: &str,
        seq: i32,
        message: Message<'_>,
    ) -> SendAttempt {
        let (collection, endpoint) = match scene {
            "group" => ("groups", ApiEndpoint::GroupMessage),
            "c2c" => ("users", ApiEndpoint::C2cMessage),
            _ => return SendAttempt::local(ApiEndpoint::Message, ErrorClass::Permanent),
        };
        if openid.is_empty() || msg_id.is_empty() || seq < 1 {
            return SendAttempt::local(endpoint, ErrorClass::Permanent);
        }
        let mut url = self.api_base.clone();
        match url.path_segments_mut() {
            Ok(mut parts) => {
                parts
                    .pop_if_empty()
                    .extend(["v2", collection, openid, "messages"]);
            }
            Err(_) => return SendAttempt::local(endpoint, ErrorClass::Permanent),
        }
        let token = match self.token(app_id).await {
            Ok(token) => token,
            Err(failure) => return failure,
        };
        let body = match message {
            Message::Plain(text) => {
                json!({"msg_type":0,"content":text,"msg_id":msg_id,"msg_seq":seq})
            }
            Message::Markdown(text) => {
                json!({"msg_type":2,"markdown":{"content":text},"msg_id":msg_id,"msg_seq":seq})
            }
        };
        let response = self
            .http
            .post(url)
            .header("Authorization", format!("QQBot {}", token.value))
            .header("X-Union-Appid", app_id)
            .json(&body)
            .send()
            .await;
        let mut attempt = match response {
            Ok(response) => read_response(response, endpoint).await.attempt(),
            Err(error) => {
                let mut failure = SendAttempt::local(endpoint, ErrorClass::Transient);
                // A connect error never reached the send endpoint; other transport errors may have.
                failure.message_attempted = !error.is_connect();
                if failure.message_attempted {
                    failure.outcome = SendOutcome::Unknown;
                }
                failure
            }
        };
        attempt.token_generation = Some(token.generation);
        attempt
    }

    /// Compatibility convenience API. Durable delivery must use `send_message_once` instead.
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

    /// Preserves the old bounded auth/URL recovery behavior for existing callers.
    pub async fn send_rendered(
        &self,
        app_id: &str,
        scene: &str,
        openid: &str,
        msg_id: &str,
        seq: i32,
        text: &str,
    ) -> Result<String, SendError> {
        let mut rendered = text.to_owned();
        let mut refreshed = false;
        let mut stripped = false;
        loop {
            let attempt = self
                .send_once(app_id, scene, openid, msg_id, seq, &rendered)
                .await;
            if attempt.outcome == SendOutcome::Accepted {
                return Ok(rendered);
            }
            if attempt.outcome == SendOutcome::Rejected && attempt.meta.endpoint.is_message() {
                if attempt.class == ErrorClass::Auth && !refreshed {
                    if let Some(generation) = attempt.token_generation {
                        self.invalidate_token(app_id, generation).await;
                        refreshed = true;
                        continue;
                    }
                }
                if attempt.class == ErrorClass::Url && !stripped {
                    let clean =
                        crate::text::without_links(&rendered, crate::message::MessageFormat::Plain);
                    if !clean.trim().is_empty() && clean != rendered {
                        rendered = clean;
                        stripped = true;
                        continue;
                    }
                }
            }
            return Err(compatibility_error(&attempt));
        }
    }
}

fn compatibility_error(attempt: &SendAttempt) -> SendError {
    if attempt.class == ErrorClass::AppMissing {
        return SendError::AppMissing;
    }
    if attempt.outcome == SendOutcome::Pending {
        return SendError::Pending;
    }
    if attempt.class == ErrorClass::Unknown
        && attempt.meta.malformed
        && attempt.meta.code.unwrap_or(0) == 0
        && attempt.meta.err_code.unwrap_or(0) == 0
        && attempt
            .meta
            .http_status
            .is_some_and(|status| (200..300).contains(&status))
    {
        return SendError::Protocol;
    }
    if let Some(status) = attempt.meta.http_status {
        let code = attempt
            .meta
            .code
            .filter(|code| *code != 0)
            .or(attempt.meta.err_code)
            .unwrap_or(0);
        return SendError::Api { status, code };
    }
    if attempt.class == ErrorClass::Transient {
        SendError::Transport
    } else {
        SendError::Protocol
    }
}

fn backoff(failures: u32) -> Duration {
    let seconds = (30u64 << failures.saturating_sub(1).min(4)).min(300);
    let mut random = [0u8; 2];
    let _ = ring::rand::SystemRandom::new().fill(&mut random);
    // Positive 0–20% jitter prevents synchronized refresh retries between instances.
    let extra_millis = seconds * 1000 * u64::from(u16::from_le_bytes(random) % 2001) / 10000;
    Duration::from_millis(seconds * 1000 + extra_millis)
}

fn local(url: &Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

fn endpoint(value: &str) -> Result<Url, Problem> {
    let url = Url::parse(value).map_err(|_| Problem::internal())?;
    if !(url.scheme() == "https" || url.scheme() == "http" && local(&url))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Problem::internal());
    }
    Ok(url)
}
