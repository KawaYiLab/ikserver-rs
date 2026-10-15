//! 帧格式 `{ op, seq, t, d }`。客户端帧宽松解析（未知字段忽略），服务端帧固定形状。

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// 服务端无活动多少秒后发 ping 帧；客户端两个周期没有任何响应视为断线。
pub const HEARTBEAT_SECS: u64 = 30;
/// 单帧上限（客户端→服务端）。
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// 每条连接最多订阅的频道数。
pub const MAX_SUBSCRIPTIONS: usize = 100;
/// 每个用户最多同时保持的连接数，超出时挤掉最旧的。
pub const MAX_CONNECTIONS_PER_USER: usize = 5;

/// 客户端 → 服务端。
#[derive(Debug, Deserialize, ToSchema)]
pub struct ClientFrame {
    /// `ping` | `subscribe` | `unsubscribe` | `typing`
    pub op: String,
    /// 客户端自增序号；服务端在 `ack` / `error` 里原样带回。
    #[serde(default)]
    pub seq: Option<u64>,
    /// 频道类型，例如 `post`。
    #[serde(default)]
    pub t: Option<String>,
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub d: Option<serde_json::Value>,
}

/// 服务端 → 客户端。
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum ServerFrame {
    /// 握手成功后的第一帧。
    Hello { d: HelloData },
    /// 对带 `seq` 的客户端帧的确认。
    Ack { seq: u64 },
    /// 业务事件，`t` 为事件类型（如 `notification.created`）。
    Event {
        t: String,
        #[schema(value_type = Object)]
        d: serde_json::Value,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<u64>,
        d: ErrorData,
    },
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HelloData {
    pub heartbeat_sec: u64,
    /// 已登录时为当前用户 id；游客连接省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// 服务端时间。断线重连后，客户端用 REST 补齐这个时间之后的变化。
    pub server_time: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorData {
    /// `INVALID_FRAME` | `UNKNOWN_OP` | `CHANNEL_INVALID` | `TOO_MANY_SUBSCRIPTIONS` | `UNSUPPORTED` | `NOT_ALLOWED`
    pub code: &'static str,
    pub detail: &'static str,
}

impl ServerFrame {
    pub(crate) fn error(seq: Option<u64>, code: &'static str, detail: &'static str) -> Self {
        Self::Error {
            seq,
            d: ErrorData { code, detail },
        }
    }
}

/// 连接关闭码（4000–4999 为应用自定义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseReason {
    pub code: u16,
    pub reason: &'static str,
}

/// 同一用户的连接超过上限，最旧的被挤掉。客户端不要自动重连。
pub const CLOSE_EVICTED: CloseReason = CloseReason {
    code: 4000,
    reason: "evicted",
};
/// 会话已吊销、过期，或账号被封禁。客户端应重新登录，不要自动重连。
pub const CLOSE_SESSION_ENDED: CloseReason = CloseReason {
    code: 4001,
    reason: "session ended",
};
/// 两个心跳周期没有任何来自客户端的帧。
pub const CLOSE_IDLE: CloseReason = CloseReason {
    code: 4002,
    reason: "idle timeout",
};
/// 客户端读得太慢，待发队列满了。可以重连后用 REST 补齐。
pub const CLOSE_SLOW_CONSUMER: CloseReason = CloseReason {
    code: 4003,
    reason: "slow consumer",
};
/// 服务端停机。
pub const CLOSE_GOING_AWAY: CloseReason = CloseReason {
    code: 1001,
    reason: "server shutting down",
};

fn decimal_id(value: &serde_json::Value) -> Option<&str> {
    let id = value.as_str()?;
    let valid = !id.is_empty() && id.len() <= 19 && id.bytes().all(|b| b.is_ascii_digit());
    valid.then_some(id)
}

/// `typing` 帧的目标会话：`t` 省略或为 `dm`，`d = { conversationId }`。
pub(crate) fn typing_target(t: Option<&str>, d: Option<&serde_json::Value>) -> Option<i64> {
    if !matches!(t, None | Some("dm")) {
        return None;
    }
    decimal_id(d?.get("conversationId")?)?
        .parse()
        .ok()
        .filter(|id| *id > 0)
}

/// 订阅频道。目前开放的只有公开帖子（`t = "post"`，`d = { postId }`）。
/// 私信事件按用户投递给会话成员，不需要订阅。
pub(crate) fn channel_of(t: Option<&str>, d: Option<&serde_json::Value>) -> Option<String> {
    match t? {
        "post" => decimal_id(d?.get("postId")?).map(|id| format!("post:{id}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_frames_serialize_with_op_tag() {
        let ack = serde_json::to_value(ServerFrame::Ack { seq: 3 }).unwrap();
        assert_eq!(ack, serde_json::json!({ "op": "ack", "seq": 3 }));
        let error = serde_json::to_value(ServerFrame::error(None, "UNKNOWN_OP", "x")).unwrap();
        assert_eq!(
            error,
            serde_json::json!({ "op": "error", "d": { "code": "UNKNOWN_OP", "detail": "x" } })
        );
    }

    #[test]
    fn only_known_channels_are_accepted() {
        let d = serde_json::json!({ "postId": "123" });
        assert_eq!(
            channel_of(Some("post"), Some(&d)).as_deref(),
            Some("post:123")
        );
        assert!(channel_of(Some("post"), Some(&serde_json::json!({ "postId": 1 }))).is_none());
        assert!(channel_of(Some("post"), Some(&serde_json::json!({ "postId": "1x" }))).is_none());
        assert!(channel_of(Some("dm"), Some(&d)).is_none());
        assert!(channel_of(None, Some(&d)).is_none());
    }

    #[test]
    fn typing_targets() {
        let d = serde_json::json!({ "conversationId": "42" });
        assert_eq!(typing_target(None, Some(&d)), Some(42));
        assert_eq!(typing_target(Some("dm"), Some(&d)), Some(42));
        assert_eq!(typing_target(Some("post"), Some(&d)), None);
        assert_eq!(
            typing_target(None, Some(&serde_json::json!({ "conversationId": 42 }))),
            None
        );
        assert_eq!(
            typing_target(None, Some(&serde_json::json!({ "conversationId": "0" }))),
            None
        );
        assert_eq!(typing_target(None, None), None);
    }
}
