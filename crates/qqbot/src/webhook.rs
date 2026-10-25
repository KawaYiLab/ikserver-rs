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

#[utoipa::path(post, operation_id = "qq_webhook", path="/v1/qq/webhook", tag="qq", responses((status=200, description="QQ event acknowledged"),(status=401, description="Signature rejected")))]
async fn webhook(
    State(state): State<QqState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, Problem> {
    // 生产日志级别下 HTTP 请求本身不打印，这里的每个出口都要留一条，否则「机器人不回复」无从排查。
    // 不记 openid、正文、签名与密钥。
    if !state.config.enabled {
        tracing::warn!("收到 QQ 回调，但 IK_QQBOT__ENABLED=false，已拒绝");
        return Err(Problem::unauthorized());
    }
    let Some(app_id) = header(&headers, "x-bot-appid") else {
        tracing::warn!("QQ 回调缺少 X-Bot-Appid，已拒绝");
        return Err(Problem::unauthorized());
    };
    let Some(app) = state.config.apps.values().find(|a| a.app_id == app_id) else {
        tracing::warn!(app_id, "QQ 回调的 AppID 不在部署配置里，已拒绝");
        return Err(Problem::unauthorized());
    };
    let payload = authenticate(
        app.secret.expose(),
        state.config.allow_validation,
        &headers,
        &body,
    )
    .inspect_err(|problem| {
        tracing::warn!(
            app_id,
            status = problem.status().as_u16(),
            signed = headers.contains_key("x-signature-ed25519"),
            "QQ 回调验签或校验失败（AppSecret 与平台不一致、或未签名的校验请求但开关关着）"
        );
    })?;
    if !state.allow_webhook(app_id) {
        tracing::warn!(app_id, "QQ 回调超过每秒限流");
        return Err(Problem::rate_limited().with_retry_after(1));
    }
    if let Authenticated::Validation(result) = payload {
        tracing::info!(app_id, "已应答 QQ 回调地址校验（op=13）");
        return Ok(Json(result));
    }
    // Only authenticated event bytes are ever parsed as a normal event.
    let envelope: Envelope = serde_json::from_slice(&body).map_err(|error| {
        tracing::warn!(app_id, %error, "QQ 回调不是合法的 JSON");
        invalid("JSON 无效")
    })?;
    if envelope.op == 0 {
        match envelope.t.as_str() {
            "GROUP_AT_MESSAGE_CREATE" | "GROUP_MESSAGE_CREATE" | "C2C_MESSAGE_CREATE" => {
                // 解析不了的事件记下原因后照样 ACK：回 4xx 平台会反复重推同一条。
                let message: Incoming = match serde_json::from_value(envelope.d) {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::warn!(app_id, event = %envelope.t, %error, "QQ 消息事件字段无法解析，已忽略");
                        return Ok(Json(json!({"op":12,"d":0})));
                    }
                };
                tracing::info!(app_id, event = %envelope.t, msg_id = %message.id, bot = message.author.bot, "收到 QQ 消息事件");
                if !message.author.bot {
                    match repo::ingest(&state, app_id, &envelope.t, message).await {
                        Ok(()) => {}
                        // 消息本身不合规（缺 id、缺身份）：记下后 ACK，重推也不会变好。
                        Err(problem) if problem.status() == StatusCode::BAD_REQUEST => {
                            tracing::warn!(app_id, event = %envelope.t, ?problem, "QQ 消息事件缺少必要字段，已忽略");
                        }
                        // 数据库等临时故障：返回 5xx 让平台重推。
                        Err(problem) => {
                            tracing::error!(app_id, event = %envelope.t, ?problem, "QQ 消息入库失败");
                            return Err(problem);
                        }
                    }
                }
            }
            "GROUP_ADD_ROBOT" | "GROUP_DEL_ROBOT" | "FRIEND_ADD" | "FRIEND_DEL" => {
                tracing::info!(app_id, event = %envelope.t, "收到 QQ 群 / 好友事件");
                let receipt = hex(ring::digest::digest(&ring::digest::SHA256, &body).as_ref());
                repo::lifecycle(&state, app_id, &envelope.t, &envelope.d, &receipt).await?;
            }
            other => tracing::info!(app_id, event = other, "忽略未处理的 QQ 事件"),
        }
    } else {
        tracing::info!(app_id, op = envelope.op, "忽略非事件的 QQ 回调");
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
        // 带了有效签名的回调校验（op=13）：签名证明请求来自平台，直接应答，不需要临时开关。
        // 否则会回 op=12 ACK，平台拿不到 plain_token / signature，保存回调地址失败。
        let value: Value = serde_json::from_slice(body).map_err(|_| invalid("JSON 无效"))?;
        if value.get("op").and_then(Value::as_u64) == Some(13) {
            return validation(&key, value);
        }
        return Ok(Authenticated::Event);
    }
    if !allow_validation {
        return Err(Problem::unauthorized());
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| Problem::unauthorized())?;
    if value.get("op").and_then(Value::as_u64) != Some(13) {
        return Err(Problem::unauthorized());
    }
    validation(&key, value)
}

/// 应答回调校验：签 `event_ts + plain_token`。不带签名的校验请求谁都能发，所以只签数字时间戳与字母数字 token，
/// 签出来的内容不可能是 JSON，不能拿去伪造事件推送。
fn validation(key: &Ed25519KeyPair, value: Value) -> Result<Authenticated, Problem> {
    let v: Validation = serde_json::from_value(value).map_err(|_| invalid("回调校验字段无效"))?;
    // event_ts 文档里是字符串；也接受数字，免得平台换了类型就校验失败。
    let event_ts = match &v.d.event_ts {
        Value::String(text) => text.clone(),
        Value::Number(number) if number.is_u64() => number.to_string(),
        _ => String::new(),
    };
    if event_ts.is_empty()
        || event_ts.len() > 20
        || !event_ts.bytes().all(|c| c.is_ascii_digit())
        || v.d.plain_token.is_empty()
        || v.d.plain_token.len() > 64
        || !v.d.plain_token.bytes().all(|c| c.is_ascii_alphanumeric())
    {
        return Err(invalid("回调校验 token 或时间戳格式无效"));
    }
    let sig = key.sign(format!("{event_ts}{}", v.d.plain_token).as_bytes());
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
    event_ts: Value,
}
#[derive(Deserialize)]
struct Envelope {
    op: u8,
    #[serde(default)]
    t: String,
    #[serde(default)]
    d: Value,
}

/// QQ 的消息事件。按宽松的方式解析：可选字段为 `null` 当作没有，时间戳接受 RFC3339、秒或毫秒（数字或字符串）。
/// 平台文档与实际推送常有出入，任何一处对不上都不应让整条消息被拒（被拒的事件平台会反复重推）。
#[derive(Debug, Deserialize)]
pub(crate) struct Incoming {
    #[serde(deserialize_with = "text")]
    pub id: String,
    #[serde(default, deserialize_with = "text")]
    pub content: String,
    #[serde(default, deserialize_with = "timestamp")]
    pub timestamp: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "optional_text")]
    pub group_openid: Option<String>,
    pub author: Author,
    #[serde(default, deserialize_with = "list")]
    pub mentions: Vec<Mention>,
}
#[derive(Debug, Deserialize)]
pub(crate) struct Author {
    #[serde(default, deserialize_with = "optional_text")]
    pub member_openid: Option<String>,
    #[serde(default, deserialize_with = "optional_text")]
    pub user_openid: Option<String>,
    /// 部分事件只给 `id`（与场景对应的 openid）。
    #[serde(default, deserialize_with = "optional_text")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "text")]
    pub username: String,
    #[serde(default, deserialize_with = "flag")]
    pub bot: bool,
}
#[derive(Debug, Deserialize)]
pub(crate) struct Mention {
    #[serde(default, deserialize_with = "flag")]
    pub is_you: bool,
    #[serde(default, deserialize_with = "flag")]
    pub bot: bool,
}

fn text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(optional_text(d)?.unwrap_or_default())
}
fn optional_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(text) => Some(text),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}
fn flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(matches!(Value::deserialize(d)?, Value::Bool(true)))
}
fn list<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|item| serde_json::from_value(item).ok())
            .collect(),
        _ => Vec::new(),
    })
}
fn timestamp<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<DateTime<Utc>>, D::Error> {
    let from_number = |n: i64| {
        // 13 位是毫秒，10 位是秒。
        if n > 100_000_000_000 {
            DateTime::from_timestamp_millis(n)
        } else {
            DateTime::from_timestamp(n, 0)
        }
    };
    Ok(match Value::deserialize(d)? {
        Value::String(text) => DateTime::parse_from_rfc3339(text.trim())
            .map(|t| t.with_timezone(&Utc))
            .ok()
            .or_else(|| text.trim().parse::<i64>().ok().and_then(from_number)),
        Value::Number(number) => number.as_i64().and_then(from_number),
        _ => None,
    })
}

/// 这条群消息是不是在 @ 本机器人。群 @ 事件一定是；全量模式（GROUP_MESSAGE_CREATE）里 @ 写成
/// `<@机器人在这个群的 member_openid>`，我们不知道这个 openid，`mentions` 也没有 `is_you`（官方文档 2026-09），
/// 所以把被 @ 的机器人（`mentions[].bot`）都当成在 @ 我们：群里有多个机器人时，@ 别的机器人也会触发，代价可接受。
pub(crate) fn mentioned(message: &Incoming, app_id: &str, event: &str) -> bool {
    event == "GROUP_AT_MESSAGE_CREATE"
        || message.mentions.iter().any(|m| m.is_you || m.bot)
        || message.content.contains(&format!("<@!{app_id}>"))
        || message.content.contains(&format!("<@{app_id}>"))
}

/// 集成测试用：签名与直接调用处理函数。
#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    pub(crate) fn sign(secret: &str, timestamp: &str, body: &[u8]) -> String {
        let mut message = timestamp.as_bytes().to_vec();
        message.extend_from_slice(body);
        signing_key(secret)
            .map(|key| hex(key.sign(&message).as_ref()))
            .unwrap_or_default()
    }

    pub(crate) async fn call(
        state: &QqState,
        app_id: &str,
        _secret: &str,
        signature: &str,
        timestamp: &str,
        body: Vec<u8>,
    ) -> (StatusCode, Value) {
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("x-bot-appid", app_id),
            ("x-signature-ed25519", signature),
            ("x-signature-timestamp", timestamp),
        ] {
            if let Ok(value) = value.parse() {
                headers.insert(name, value);
            }
        }
        match webhook(State(state.clone()), headers, Bytes::from(body)).await {
            Ok(Json(value)) => (StatusCode::OK, value),
            Err(problem) => (problem.status(), Value::Null),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    #[test]
    fn official_signature_vector() {
        // 输入与公钥取自官方「签名校验」文档的 DEMO。那页给出的 sig（865ad13a…）用它写明的输入验不过
        // （Python cryptography 与 ring 一致地算出下面这个），所以签名用独立实现交叉生成：
        // python: Ed25519PrivateKey.from_private_bytes(seed).sign(b"1725442341" + body)。
        let secret = "naOC0ocQE3shWLAfffVLB1rhYPG7";
        let body = br#"{ "op": 0,"d": {}, "t": "GATEWAY_EVENT_NAME"}"#;
        let signature = "2eb9983ebb8bb209e78fd095942f58e442656656e7975d01e64f9023a84b7c964290fdd40e5500c33867ccfe9563b7e0b6bac0e1d42c13e787b304fd51f71102";
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
        // op=13：签 event_ts + plain_token（同样用 Python 交叉生成）。
        let challenge =
            br#"{"op":13,"d":{"plain_token":"Arq0D5A61EgUu4OxUvOp","event_ts":"1725439208"}}"#;
        let Ok(Authenticated::Validation(reply)) =
            authenticate(secret, true, &HeaderMap::new(), challenge)
        else {
            panic!("op=13 应当返回签名");
        };
        assert_eq!(reply["plain_token"], "Arq0D5A61EgUu4OxUvOp");
        assert_eq!(
            reply["signature"],
            "82e076c9ca890e60b38d5f91e2d78eca2772e935bf24424700d834a569af03e26ffc2ae939b5824649d950899013e8a1a4cb7c39cb532955743be1b016681c0c"
        );
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
        let state =
            QqState::new(db.clone(), ids, Jobs::new(db), ai, config, b"key".to_vec()).unwrap();
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
            Ed25519KeyPair::from_seed_unchecked(b"abcabcabcabcabcabcabcabcabcabcab").unwrap();
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
    fn official_validation_example() {
        // 官方「事件订阅与通知 → Webhook 方式」页面的回调验证示例（appid 11111111）。
        let body =
            br#"{"d":{"plain_token":"Arq0D5A61EgUu4OxUvOp","event_ts":"1725442341"},"op":13}"#;
        let Ok(Authenticated::Validation(reply)) =
            authenticate("DG5g3B4j9X2KOErG", true, &HeaderMap::new(), body)
        else {
            panic!("官方示例应当应答");
        };
        assert_eq!(
            reply["signature"],
            "87befc99c42c651b3aac0278e71ada338433ae26fcb24307bdc5ad38c1adc2d01bcfcadc0842edac85e85205028a1132afe09280305f13aa6909ffc2d652c706"
        );
    }

    #[test]
    fn signed_validation_is_answered_without_the_switch() {
        // 平台的校验请求若带签名头：验签通过就直接应答（不再回 op=12），开关关着也行。
        let secret = "naOC0ocQE3shWLAfffVLB1rhYPG7";
        let body =
            br#"{"op":13,"d":{"plain_token":"Arq0D5A61EgUu4OxUvOp","event_ts":"1725439208"}}"#;
        let key = signing_key(secret).unwrap();
        let mut signed = b"1725439300".to_vec();
        signed.extend_from_slice(body);
        let mut headers = HeaderMap::new();
        headers.insert("x-signature-timestamp", "1725439300".parse().unwrap());
        headers.insert(
            "x-signature-ed25519",
            hex(key.sign(&signed).as_ref()).parse().unwrap(),
        );
        let Ok(Authenticated::Validation(reply)) = authenticate(secret, false, &headers, body)
        else {
            panic!("带签名的 op=13 应当返回校验签名");
        };
        assert_eq!(
            reply["signature"],
            "82e076c9ca890e60b38d5f91e2d78eca2772e935bf24424700d834a569af03e26ffc2ae939b5824649d950899013e8a1a4cb7c39cb532955743be1b016681c0c"
        );
        // 签名不对仍然 401。
        headers.insert("x-signature-timestamp", "1725439301".parse().unwrap());
        assert_eq!(
            authenticate(secret, false, &headers, body)
                .err()
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        // event_ts 是数字也能应答。
        let numeric =
            br#"{"op":13,"d":{"plain_token":"Arq0D5A61EgUu4OxUvOp","event_ts":1725439208}}"#;
        let Ok(Authenticated::Validation(reply)) =
            authenticate(secret, true, &HeaderMap::new(), numeric)
        else {
            panic!("数字 event_ts 应当可以应答");
        };
        assert_eq!(reply["plain_token"], "Arq0D5A61EgUu4OxUvOp");
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
    fn official_group_at_example_parses() {
        // 官方「群@机器人消息」页面的示例 1（2026-09 版）：content 已去掉 @ 前缀，带 member_role、message_type。
        let message: Incoming = serde_json::from_value(json!({
            "id": "ROBOT1.0_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            "author": {"id": "A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4", "member_openid": "A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4",
                       "member_role": "member", "username": "小明", "bot": false},
            "content": " /今日天气 ", "group_openid": "B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5",
            "message_type": 0, "timestamp": "2026-07-21T10:00:00+08:00",
            "message_scene": {"source": "default", "ext": ["msg_idx=REFIDX_xxxxxxxxxxxxxxx==", "auth_token=x"]}
        }))
        .unwrap();
        assert_eq!(
            message.author.member_openid.as_deref(),
            Some("A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4")
        );
        assert!(mentioned(&message, "1", "GROUP_AT_MESSAGE_CREATE"));
        assert_eq!(crate::text::incoming(&message.content), "/今日天气");
    }

    #[test]
    fn real_world_event_shapes_parse() {
        // 群 @：RFC3339 带 +08:00、mentions 带 is_you、多余字段（message_scene 等）忽略。
        let group: Incoming = serde_json::from_value(json!({
            "id": "ROBOT1.0_abc", "content": " 你好", "timestamp": "2026-10-26T05:18:29+08:00",
            "group_id": "G", "group_openid": "GROUPOPENID", "message_scene": {"source": "default"},
            "author": {"id": "MEMBER", "member_openid": "MEMBER", "union_openid": "U"},
            "mentions": [{"scope": "single", "id": "BOT", "is_you": true, "bot": true}],
            "attachments": null
        }))
        .unwrap();
        assert_eq!(
            group.timestamp.unwrap().to_rfc3339(),
            "2026-10-25T21:18:29+00:00"
        );
        assert!(group.mentions[0].is_you);
        // 单聊：时间戳是秒数字符串；各种 null。
        let c2c: Incoming = serde_json::from_value(json!({
            "id": "1", "content": null, "timestamp": "1761427109",
            "author": {"user_openid": "USER", "username": null, "bot": null}, "mentions": null
        }))
        .unwrap();
        assert_eq!(c2c.timestamp.unwrap().timestamp(), 1_761_427_109);
        assert!(c2c.content.is_empty() && c2c.mentions.is_empty() && !c2c.author.bot);
        // 毫秒数字、无法识别的时间：分别解析成毫秒时刻与「没有时间」，都不报错。
        let ms: Incoming = serde_json::from_value(
            json!({"id": "2", "timestamp": 1_761_427_109_123_i64, "author": {"id": "X"}}),
        )
        .unwrap();
        assert_eq!(ms.timestamp.unwrap().timestamp_millis(), 1_761_427_109_123);
        let odd: Incoming =
            serde_json::from_value(json!({"id": "3", "timestamp": "昨天", "author": {}})).unwrap();
        assert!(odd.timestamp.is_none());
    }

    #[test]
    fn all_message_mode_only_replies_to_mentions() {
        let mut msg: Incoming = serde_json::from_value(json!({"id":"1","timestamp":"2026-10-25T00:00:00Z","content":"hello","author":{"member_openid":"a"},"mentions":[]})).unwrap();
        assert!(!mentioned(&msg, "123", "GROUP_MESSAGE_CREATE"));
        msg.mentions.push(Mention {
            is_you: true,
            bot: false,
        });
        assert!(mentioned(&msg, "123", "GROUP_MESSAGE_CREATE"));
        msg.mentions.clear();
        msg.content = "<@123> hello".to_owned();
        assert!(mentioned(&msg, "123", "GROUP_MESSAGE_CREATE"));
    }
}
