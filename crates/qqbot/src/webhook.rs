use crate::{QqState, repo};
use axum::{
    Json,
    body::Bytes,
    extract::{DefaultBodyLimit, FromRef, State},
};
use chrono::{DateTime, Utc};
use http::{HeaderMap, StatusCode};
use ik_kernel::Problem;
use ring::signature::{self, Ed25519KeyPair, KeyPair};
use serde::Deserialize;
use serde_json::{Value, json};
use utoipa_axum::{router::OpenApiRouter, routes};

pub(crate) fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    QqState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(webhook))
        .layer(DefaultBodyLimit::max(64 * 1024))
}

#[utoipa::path(post, path="/v1/qq/webhook", tag="qq", responses((status=200, description="QQ event acknowledged"),(status=401, description="Signature rejected")))]
async fn webhook(
    State(state): State<QqState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, Problem> {
    if !state.config.enabled {
        return Err(Problem::unauthorized());
    }
    let app_id = header(&headers, "x-bot-appid").ok_or_else(Problem::unauthorized)?;
    let app = state
        .config
        .apps
        .values()
        .find(|a| a.app_id == app_id)
        .ok_or_else(Problem::unauthorized)?;
    let payload = authenticate(
        app.secret.expose(),
        state.config.allow_validation,
        &headers,
        &body,
    )?;
    if !state.allow_webhook(app_id) {
        return Err(Problem::rate_limited().with_retry_after(1));
    }
    if let Authenticated::Validation(result) = payload {
        return Ok(Json(result));
    }
    // Only authenticated event bytes are ever parsed as a normal event.
    let envelope: Envelope = serde_json::from_slice(&body).map_err(|_| invalid("JSON 无效"))?;
    if envelope.op == 0 {
        match envelope.t.as_str() {
            "GROUP_AT_MESSAGE_CREATE" | "GROUP_MESSAGE_CREATE" | "C2C_MESSAGE_CREATE" => {
                let message: Incoming =
                    serde_json::from_value(envelope.d).map_err(|_| invalid("消息字段无效"))?;
                if !message.author.bot {
                    repo::ingest(&state, app_id, &envelope.t, message).await?;
                }
            }
            "GROUP_ADD_ROBOT" | "GROUP_DEL_ROBOT" | "FRIEND_ADD" | "FRIEND_DEL" => {
                let receipt = hex(ring::digest::digest(&ring::digest::SHA256, &body).as_ref());
                repo::lifecycle(&state, app_id, &envelope.t, &envelope.d, &receipt).await?;
            }
            _ => {}
        }
    }
    Ok(Json(json!({"op":12,"d":0})))
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}
fn invalid(detail: &str) -> Problem {
    Problem::new(StatusCode::BAD_REQUEST, "VALIDATION_FAILED", detail)
}

enum Authenticated {
    Event,
    Validation(Value),
}
fn authenticate(
    secret: &str,
    allow_validation: bool,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Authenticated, Problem> {
    let key = signing_key(secret)?;
    let timestamp = header(headers, "x-signature-timestamp");
    let sig = header(headers, "x-signature-ed25519");
    if headers.contains_key("x-signature-timestamp") || headers.contains_key("x-signature-ed25519")
    {
        let timestamp = timestamp
            .filter(|value| !value.is_empty())
            .ok_or_else(Problem::unauthorized)?;
        let sig = sig
            .and_then(decode_signature)
            .ok_or_else(Problem::unauthorized)?;
        let mut message = timestamp.as_bytes().to_vec();
        message.extend_from_slice(body);
        signature::UnparsedPublicKey::new(&signature::ED25519, key.public_key().as_ref())
            .verify(&message, &sig)
            .map_err(|_| Problem::unauthorized())?;
        return Ok(Authenticated::Event);
    }
    if !allow_validation {
        return Err(Problem::unauthorized());
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| Problem::unauthorized())?;
    if value.get("op").and_then(Value::as_u64) != Some(13) {
        return Err(Problem::unauthorized());
    }
    let v: Validation = serde_json::from_value(value).map_err(|_| invalid("回调校验字段无效"))?;
    if v.d.event_ts.is_empty()
        || v.d.event_ts.len() > 20
        || !v.d.event_ts.bytes().all(|c| c.is_ascii_digit())
        || v.d.plain_token.is_empty()
        || v.d.plain_token.len() > 64
        || !v.d.plain_token.bytes().all(|c| c.is_ascii_alphanumeric())
    {
        return Err(invalid("回调校验 token 或时间戳格式无效"));
    }
    let sig = key.sign(format!("{}{}", v.d.event_ts, v.d.plain_token).as_bytes());
    Ok(Authenticated::Validation(
        json!({"plain_token":v.d.plain_token,"signature":hex(sig.as_ref())}),
    ))
}

fn signing_key(secret: &str) -> Result<Ed25519KeyPair, Problem> {
    if secret.is_empty() {
        return Err(Problem::internal());
    }
    let seed: Vec<u8> = secret.bytes().cycle().take(32).collect();
    Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|_| Problem::internal())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn decode_signature(raw: &str) -> Option<Vec<u8>> {
    if raw.len() != 128 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..128)
        .step_by(2)
        .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).ok())
        .collect()
}
#[derive(Deserialize)]
struct Validation {
    d: Challenge,
}
#[derive(Deserialize)]
struct Challenge {
    plain_token: String,
    event_ts: String,
}
#[derive(Deserialize)]
struct Envelope {
    op: u8,
    #[serde(default)]
    t: String,
    #[serde(default)]
    d: Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Incoming {
    pub id: String,
    #[serde(default)]
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub group_openid: Option<String>,
    pub author: Author,
    #[serde(default)]
    pub mentions: Vec<Mention>,
}
#[derive(Debug, Deserialize)]
pub(crate) struct Author {
    pub member_openid: Option<String>,
    pub user_openid: Option<String>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub bot: bool,
}
#[derive(Debug, Deserialize)]
pub(crate) struct Mention {
    #[serde(default)]
    pub is_you: bool,
}
pub(crate) fn mentioned(message: &Incoming, app_id: &str, event: &str) -> bool {
    event == "GROUP_AT_MESSAGE_CREATE"
        || message.mentions.iter().any(|m| m.is_you)
        || message.content.contains(&format!("<@!{app_id}>"))
        || message.content.contains(&format!("<@{app_id}>"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    #[test]
    fn official_signature_vector() {
        // https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/interface-framework/sign.html
        let secret = "naOC0ocQE3shWLAfffVLB1rhYPG7";
        let body = br#"{ "op": 0,"d": {}, "t": "GATEWAY_EVENT_NAME"}"#;
        let signature = "865ad13a61752ca65e26bde6676459cd36cf1be609375b37bd62af366e1dc25a8dc789ba7f14e017ada3d554c671a911bfdf075ba54835b23391d509579ed002";
        let mut headers = HeaderMap::new();
        headers.insert("x-signature-timestamp", "1725442341".parse().unwrap());
        headers.insert("x-signature-ed25519", signature.parse().unwrap());
        assert!(authenticate(secret, false, &headers, body).is_ok());
        assert_eq!(
            signing_key(secret).unwrap().public_key().as_ref(),
            &[
                215, 195, 98, 254, 120, 174, 248, 31, 242, 50, 135, 180, 147, 98, 139, 93, 176, 42,
                60, 79, 227, 11, 33, 94, 77, 25, 96, 155, 93, 118, 103, 58
            ]
        );
        headers.insert("x-signature-timestamp", "".parse().unwrap());
        assert!(authenticate(secret, false, &headers, body).is_err());
    }
    #[tokio::test]
    async fn invalid_signatures_do_not_consume_application_allowance() {
        use ik_kernel::{
            Snowflake,
            bus::LocalBus,
            config::{Ai, Qqbot, QqbotApp, Secret},
            jobs::Jobs,
        };
        use std::{collections::BTreeMap, sync::Arc};
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://test:test@127.0.0.1/test")
            .unwrap();
        let ids = Arc::new(Snowflake::new(3).unwrap());
        let ai = ik_ai::AiState::for_tests(
            db.clone(),
            ids.clone(),
            Arc::new(LocalBus::new(16)),
            Ai::default(),
        )
        .unwrap();
        let config = Qqbot {
            enabled: true,
            webhook_rate_per_second: 1,
            apps: BTreeMap::from([(
                "test".into(),
                QqbotApp {
                    app_id: "123".into(),
                    secret: Secret::new("abc"),
                },
            )]),
            ..Default::default()
        };
        let state = QqState::new(db.clone(), ids, Jobs::new(db), ai, config).unwrap();
        let body = br#"{"op":12}"#;
        let mut headers = HeaderMap::new();
        headers.insert("x-bot-appid", "123".parse().unwrap());
        for _ in 0..3 {
            assert_eq!(
                webhook(
                    State(state.clone()),
                    headers.clone(),
                    Bytes::from_static(body)
                )
                .await
                .err()
                .unwrap()
                .status(),
                StatusCode::UNAUTHORIZED
            );
        }
        let mut signed = b"time".to_vec();
        signed.extend_from_slice(body);
        headers.insert("x-signature-timestamp", "time".parse().unwrap());
        headers.insert(
            "x-signature-ed25519",
            hex(signing_key("abc").unwrap().sign(&signed).as_ref())
                .parse()
                .unwrap(),
        );
        assert!(
            webhook(
                State(state.clone()),
                headers.clone(),
                Bytes::from_static(body)
            )
            .await
            .is_ok()
        );
        assert_eq!(
            webhook(State(state), headers, Bytes::from_static(body))
                .await
                .err()
                .unwrap()
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }
    #[test]
    fn signed_bytes_and_short_seed() {
        let key = signing_key("abc").unwrap();
        let repeated =
            Ed25519KeyPair::from_seed_unchecked(b"abcabcabcabcabcabcabcabcabcabcabca").unwrap();
        assert_eq!(key.public_key().as_ref(), repeated.public_key().as_ref());
        let body = br#"{"op":0,"d":{}}"#;
        let mut bytes = b"time".to_vec();
        bytes.extend_from_slice(body);
        let mut headers = HeaderMap::new();
        headers.insert("x-signature-timestamp", "time".parse().unwrap());
        headers.insert(
            "x-signature-ed25519",
            hex(key.sign(&bytes).as_ref()).parse().unwrap(),
        );
        assert!(authenticate("abc", false, &headers, body).is_ok());
        assert_eq!(
            authenticate("abc", false, &headers, b"{}")
                .err()
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        headers.insert("x-signature-ed25519", "aabb".parse().unwrap());
        assert!(authenticate("abc", false, &headers, body).is_err());
    }
    #[test]
    fn validation_cannot_be_a_signing_oracle() {
        let h = HeaderMap::new();
        let good = br#"{"op":13,"d":{"event_ts":"12345","plain_token":"ABC123"}}"#;
        assert!(authenticate("abc", false, &h, good).is_err());
        assert!(matches!(
            authenticate("abc", true, &h, good),
            Ok(Authenticated::Validation(_))
        ));
        for token in ["{\"op\":0}", "has space", ""] {
            let bad =
                serde_json::to_vec(&json!({"op":13,"d":{"event_ts":"123","plain_token":token}}))
                    .unwrap();
            assert_eq!(
                authenticate("abc", true, &h, &bad).err().unwrap().status(),
                StatusCode::BAD_REQUEST
            );
        }
        let bad = br#"{"op":13,"d":{"event_ts":"no","plain_token":"abc"}}"#;
        assert!(authenticate("abc", true, &h, bad).is_err());
        let long = serde_json::to_vec(
            &json!({"op":13,"d":{"event_ts":"123","plain_token":"a".repeat(65)}}),
        )
        .unwrap();
        assert!(authenticate("abc", true, &h, &long).is_err());
        assert_eq!(
            authenticate("abc", true, &h, br#"{"op":0,"d":{}}"#)
                .err()
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    #[test]
    fn all_message_mode_only_replies_to_mentions() {
        let mut msg: Incoming = serde_json::from_value(json!({"id":"1","timestamp":"2026-10-25T00:00:00Z","content":"hello","author":{"member_openid":"a"},"mentions":[]})).unwrap();
        assert!(!mentioned(&msg, "123", "GROUP_MESSAGE_CREATE"));
        msg.mentions.push(Mention { is_you: true });
        assert!(mentioned(&msg, "123", "GROUP_MESSAGE_CREATE"));
        msg.mentions.clear();
        msg.content = "<@123> hello".to_owned();
        assert!(mentioned(&msg, "123", "GROUP_MESSAGE_CREATE"));
    }
}
