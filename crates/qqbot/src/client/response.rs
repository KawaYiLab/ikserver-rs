//! QQ has two error-code namespaces. Keep both; differing numbers are normal.
//! Sources: https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/api-call-guide.html
//! and https://bot.q.qq.com/wiki/develop/api-v2/autogen/api/v2_groups_group_openid_messages.post.html
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;

const MAX_BODY: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiEndpoint {
    Token,
    GroupMessage,
    C2cMessage,
    /// Unspecified message endpoint, used for caller-side timeouts.
    Message,
}

impl ApiEndpoint {
    pub fn is_message(self) -> bool {
        self != Self::Token
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendOutcome {
    Accepted,
    Pending,
    Rejected,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorClass {
    None,
    Auth,
    Url,
    RateLimited,
    Transient,
    Deduplicated,
    Permanent,
    Unknown,
    AppMissing,
}

/// Safe diagnostic fields only: never retain a response body, token or destination.
#[derive(Clone, Debug)]
pub struct ApiMetadata {
    pub endpoint: ApiEndpoint,
    pub http_status: Option<u16>,
    pub code: Option<i64>,
    pub err_code: Option<i64>,
    pub trace_id: Option<String>,
    pub retry_after: Option<Duration>,
    /// An error field or response body could not be interpreted. Null is not malformed.
    pub malformed: bool,
}

#[derive(Clone, Debug)]
pub struct SendAttempt {
    pub outcome: SendOutcome,
    pub class: ErrorClass,
    pub meta: ApiMetadata,
    pub message_id: Option<String>,
    pub audit_id: Option<String>,
    pub token_generation: Option<u64>,
    /// False means token acquisition/local validation/connect failed before a message could be sent.
    pub message_attempted: bool,
}

impl SendAttempt {
    pub fn reason(&self) -> &'static str {
        match self.class {
            ErrorClass::Auth => "qq_token_rejected",
            ErrorClass::Url => "qq_url_rejected",
            ErrorClass::RateLimited => "qq_rate_limited",
            ErrorClass::Transient if self.meta.http_status.is_none() => "qq_transport_failed",
            ErrorClass::Transient => "qq_unavailable",
            ErrorClass::Deduplicated => "qq_deduplicated",
            ErrorClass::Permanent => "qq_message_rejected",
            ErrorClass::AppMissing => "qq_app_missing",
            ErrorClass::Unknown if self.meta.malformed => "qq_invalid_response",
            ErrorClass::Unknown => "qq_unknown_result",
            ErrorClass::None if self.outcome == SendOutcome::Pending => "qq_pending",
            ErrorClass::None => "qq_accepted",
        }
    }

    pub(super) fn local(endpoint: ApiEndpoint, class: ErrorClass) -> Self {
        Self {
            outcome: SendOutcome::Rejected,
            class,
            meta: ApiMetadata {
                endpoint,
                http_status: None,
                code: None,
                err_code: None,
                trace_id: None,
                retry_after: None,
                malformed: false,
            },
            message_id: None,
            audit_id: None,
            token_generation: None,
            message_attempted: false,
        }
    }
}

pub(super) struct Decoded {
    pub meta: ApiMetadata,
    pub body: Option<Value>,
    code_malformed: bool,
    transport_failed: bool,
}

pub(super) fn number(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn code_field(body: &Value, key: &str) -> (Option<i64>, bool) {
    match body.get(key) {
        None | Some(Value::Null) => (None, false),
        Some(value) => {
            let parsed = number(value);
            (parsed, parsed.is_none())
        }
    }
}

fn identifier(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) if !value.is_empty() && value.len() <= 256 => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn trace(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.')))
    .then(|| value.to_owned())
}

fn retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    DateTime::parse_from_rfc2822(value).ok().map(|date| {
        (date.with_timezone(&Utc) - Utc::now())
            .to_std()
            .unwrap_or_default()
    })
}

pub(super) async fn read_response(
    mut response: reqwest13::Response,
    endpoint: ApiEndpoint,
) -> Decoded {
    let mut decoded = Decoded {
        meta: ApiMetadata {
            endpoint,
            http_status: Some(response.status().as_u16()),
            code: None,
            err_code: None,
            trace_id: response
                .headers()
                .get("x-tps-trace-id")
                .and_then(|v| v.to_str().ok())
                .and_then(trace),
            retry_after: response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(retry_after),
            malformed: false,
        },
        body: None,
        code_malformed: false,
        transport_failed: false,
    };
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY as u64)
    {
        decoded.meta.malformed = true;
        return decoded;
    }
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if bytes.len() + chunk.len() <= MAX_BODY => {
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Ok(Some(_)) => {
                decoded.meta.malformed = true;
                return decoded;
            }
            Err(_) => {
                decoded.transport_failed = true;
                return decoded;
            }
        }
    }
    decoded.set_body(serde_json::from_slice(&bytes).ok());
    decoded
}

impl Decoded {
    fn set_body(&mut self, body: Option<Value>) {
        let Some(body) = body.filter(Value::is_object) else {
            self.meta.malformed = true;
            return;
        };
        let (code, bad_code) = code_field(&body, "code");
        let (err_code, bad_err_code) = code_field(&body, "err_code");
        self.meta.code = code;
        self.meta.err_code = err_code;
        self.code_malformed = bad_code || bad_err_code;
        self.meta.malformed = self.code_malformed;
        if self.meta.trace_id.is_none() {
            self.meta.trace_id = body.get("trace_id").and_then(Value::as_str).and_then(trace);
        }
        self.body = Some(body);
    }

    pub fn attempt(&self) -> SendAttempt {
        let message_id = identifier(self.body.as_ref().and_then(|v| v.get("id")));
        let audit_id = identifier(self.body.as_ref().and_then(|v| {
            v.get("audit_id")
                .or_else(|| v.pointer("/data/message_audit/audit_id"))
                .or_else(|| v.pointer("/message_audit/audit_id"))
        }));
        let has_token = self.meta.endpoint == ApiEndpoint::Token
            && self.body.as_ref().is_some_and(|body| {
                body.get("access_token")
                    .and_then(Value::as_str)
                    .is_some_and(|token| !token.is_empty())
                    && body
                        .get("expires_in")
                        .and_then(number)
                        .is_some_and(|expires| expires > 0)
            });
        let (outcome, class) = self.classify(message_id.is_some() || has_token);
        SendAttempt {
            outcome,
            class,
            meta: self.meta.clone(),
            message_id,
            audit_id,
            token_generation: None,
            message_attempted: self.meta.endpoint.is_message(),
        }
    }

    fn classify(&self, has_receipt: bool) -> (SendOutcome, ErrorClass) {
        use ErrorClass as C;
        use SendOutcome as O;
        let status = self.meta.http_status.unwrap_or(0);
        // HTTP evidence survives invalid JSON, malformed error fields, and oversized bodies.
        match status {
            429 => return (O::Rejected, C::RateLimited),
            500..=599 => return (O::Rejected, C::Transient),
            401 => return (O::Rejected, C::Auth),
            _ => {}
        }
        if self.transport_failed {
            return (O::Unknown, C::Transient);
        }
        let success = (200..300).contains(&status);
        let codes: Vec<i64> = [self.meta.code, self.meta.err_code]
            .into_iter()
            .flatten()
            .filter(|code| *code != 0)
            .collect();
        let pending = self.meta.endpoint.is_message()
            && codes.iter().any(|code| matches!(code, 304023 | 304024));
        let mut known = None;
        for code in &codes {
            let class = classify_code(self.meta.endpoint, *code);
            if class == C::Unknown {
                continue;
            }
            if known.is_some_and(|old| old != class) || pending {
                return (O::Unknown, C::Unknown);
            }
            known = Some(class);
        }
        if let Some(class) = known {
            return (
                if class == C::Deduplicated {
                    O::Unknown
                } else {
                    O::Rejected
                },
                class,
            );
        }
        if !success {
            return (O::Rejected, C::Unknown);
        }
        if pending
            || (self.meta.endpoint.is_message() && codes.is_empty() && matches!(status, 201 | 202))
        {
            return (O::Pending, C::None);
        }
        if !codes.is_empty() {
            return (O::Rejected, C::Unknown);
        }
        // Preserve existing successful objects without requiring every documented field.
        // Only uninterpretable *error* fields without a receipt remain unconfirmed.
        if self.body.is_some() && (!self.code_malformed || has_receipt) {
            (O::Accepted, C::None)
        } else {
            (O::Unknown, C::Unknown)
        }
    }
}

fn classify_code(endpoint: ApiEndpoint, code: i64) -> ErrorClass {
    use ErrorClass as C;
    if endpoint == ApiEndpoint::Token {
        return match code {
            100001 => C::RateLimited,
            100007 | 100016 | 10004 => C::Permanent,
            _ => C::Unknown,
        };
    }
    match code {
        40014 | 40001 | 11244 | 11201 | 40012001 => C::Auth,
        // 40054010: current group API; 304003: official common codes;
        // 40034028: retained legacy compatibility.
        304003 | 40034028 => C::Url,
        40054010 if endpoint == ApiEndpoint::GroupMessage => C::Url,
        40054005 => C::Deduplicated,
        50055001 | 50055006 if endpoint == ApiEndpoint::GroupMessage => C::Transient,
        50055002 | 40054006 if endpoint == ApiEndpoint::C2cMessage => C::Transient,
        304005 | 304025 | 304026 | 304027 | 304028 | 40034005 | 40034006 | 40034024 | 40034026
        | 40034027 | 40034128 | 40054002 | 40054003 | 40054004 | 40054007 | 40054013 | 40054016
        | 40054018 => C::Permanent,
        _ => C::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decoded(status: u16, body: Value) -> Decoded {
        let mut result = Decoded {
            meta: SendAttempt::local(ApiEndpoint::GroupMessage, ErrorClass::None).meta,
            body: None,
            code_malformed: false,
            transport_failed: false,
        };
        result.meta.http_status = Some(status);
        result.set_body(Some(body));
        result
    }

    #[test]
    fn permissive_success_and_explicit_error_fields_are_separate() {
        for body in [
            json!({}),
            json!({"code":null,"err_code":null,"extra":{}}),
            json!({"code":"0","id":null}),
            json!({"id":"sent","code":"ok"}),
        ] {
            assert_eq!(decoded(200, body).attempt().outcome, SendOutcome::Accepted);
        }
        let result = decoded(200, json!({"code":0,"err_code":"40054010"})).attempt();
        assert_eq!(result.class, ErrorClass::Url);
        assert_eq!(result.meta.code, Some(0));
        assert_eq!(result.meta.err_code, Some(40054010));
        assert_eq!(
            decoded(200, json!({"code":{}})).attempt().outcome,
            SendOutcome::Unknown
        );
    }

    #[test]
    fn different_codes_are_normal_but_incompatible_actions_are_not_guessed() {
        let result = decoded(401, json!({"code":11201,"err_code":40012001})).attempt();
        assert_eq!(result.class, ErrorClass::Auth);
        let result = decoded(400, json!({"code":304003,"err_code":40054010})).attempt();
        assert_eq!(result.class, ErrorClass::Url);
        let result = decoded(400, json!({"code":40014,"err_code":40054010})).attempt();
        assert_eq!(result.outcome, SendOutcome::Unknown);
    }

    #[test]
    fn async_acceptance_and_deduplication_are_not_delivery_confirmation() {
        for (status, body) in [
            (201, json!({})),
            (202, json!({"code":304023})),
            (
                200,
                json!({"err_code":304024,"data":{"message_audit":{"audit_id":"audit"}}}),
            ),
        ] {
            assert_eq!(
                decoded(status, body).attempt().outcome,
                SendOutcome::Pending
            );
        }
        assert_eq!(
            decoded(400, json!({"code":304023})).attempt().outcome,
            SendOutcome::Rejected
        );
        let result = decoded(400, json!({"err_code":40054005})).attempt();
        assert_eq!(result.outcome, SendOutcome::Unknown);
        assert_eq!(result.class, ErrorClass::Deduplicated);
        assert_eq!(
            decoded(400, json!({"code":100001})).attempt().class,
            ErrorClass::Unknown
        );
    }

    #[test]
    fn endpoint_specific_codes_do_not_change_other_endpoints() {
        for (endpoint, code, expected) in [
            (ApiEndpoint::Token, 100001, ErrorClass::RateLimited),
            (ApiEndpoint::GroupMessage, 100001, ErrorClass::Unknown),
            (ApiEndpoint::C2cMessage, 100001, ErrorClass::Unknown),
            (ApiEndpoint::GroupMessage, 50055001, ErrorClass::Transient),
            (ApiEndpoint::GroupMessage, 50055006, ErrorClass::Transient),
            (ApiEndpoint::C2cMessage, 50055001, ErrorClass::Unknown),
            (ApiEndpoint::C2cMessage, 50055006, ErrorClass::Unknown),
            (ApiEndpoint::C2cMessage, 50055002, ErrorClass::Transient),
            (ApiEndpoint::C2cMessage, 40054006, ErrorClass::Transient),
            (ApiEndpoint::GroupMessage, 50055002, ErrorClass::Unknown),
            (ApiEndpoint::GroupMessage, 40054006, ErrorClass::Unknown),
            (ApiEndpoint::GroupMessage, 40054010, ErrorClass::Url),
            (ApiEndpoint::C2cMessage, 40054010, ErrorClass::Unknown),
            (ApiEndpoint::Token, 40054010, ErrorClass::Unknown),
        ] {
            let mut response = decoded(200, json!({"err_code": code}));
            response.meta.endpoint = endpoint;
            let attempt = response.attempt();
            assert_eq!(attempt.class, expected, "{endpoint:?} code {code}");
            assert_eq!(attempt.outcome, SendOutcome::Rejected);
            assert_eq!(attempt.message_attempted, endpoint.is_message());
        }
    }

    #[test]
    fn bad_fields_never_mask_http_failure_semantics() {
        for (status, class) in [
            (401, ErrorClass::Auth),
            (429, ErrorClass::RateLimited),
            (503, ErrorClass::Transient),
        ] {
            for body in [json!({"code":"bad","err_code":{}}), Value::Null, json!([])] {
                let result = decoded(status, body).attempt();
                assert_eq!(result.class, class);
                assert_eq!(result.meta.http_status, Some(status));
            }
        }
    }
}
