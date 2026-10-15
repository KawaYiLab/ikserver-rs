//! 对外的请求与响应类型。

use chrono::{DateTime, Utc};
use ik_kernel::Id;
use ik_kernel::id::marker::{Conversation, Message, User};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// 会话的对方。已注销的账号 `id`、`level` 省略，名字显示为「已注销用户」。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PeerView {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub id: Option<Id<User>>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i16>,
}

/// 被回复的消息（摘要）。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplyView {
    #[schema(value_type = String)]
    pub id: Id<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub sender_id: Option<Id<User>>,
    /// 前 80 个字；被回复的消息已撤回时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub withdrawn: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    #[schema(value_type = String)]
    pub id: Id<Message>,
    #[schema(value_type = String)]
    pub conversation_id: Id<Conversation>,
    #[schema(value_type = String)]
    pub sender_id: Id<User>,
    /// `text` | `system`
    pub kind: String,
    /// 已撤回时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 被回复的消息已被物理删除时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<ReplyView>,
    /// 发送时带的去重键，便于客户端把乐观插入的消息替换成正式消息。只在发送者自己的视图里有意义。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withdrawn_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// 会话列表里的最后一条消息。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LastMessageView {
    #[schema(value_type = String)]
    pub id: Id<Message>,
    #[schema(value_type = String)]
    pub sender_id: Id<User>,
    /// 预览文案：前 80 个字；已撤回为「[消息已撤回]」（CONV-032）。
    pub preview: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationView {
    #[schema(value_type = String)]
    pub id: Id<Conversation>,
    /// 目前只有 `direct`（一对一私聊）。
    pub kind: String,
    pub peer: PeerView,
    /// 对方的用户 id（含已注销的账号）。不下发，供收件箱合并对方的通知分组。
    #[serde(skip)]
    pub peer_user_id: i64,
    pub muted: bool,
    pub pinned: bool,
    /// 收件箱排序时间：最后一条消息，或对方最后一条通知的时间。
    pub activity_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_read_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message: Option<LastMessageView>,
    /// 未读消息数（不含对方发来的通知；合并后的数字见收件箱）。
    pub unread_count: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenDirectRequest {
    #[schema(value_type = String)]
    pub user_id: Id<User>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateConversationRequest {
    #[serde(default)]
    pub muted: Option<bool>,
    /// 每人最多置顶 50 个会话。
    #[serde(default)]
    pub pinned: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SendMessageRequest {
    /// 1～4000 字（首尾空白会被去掉）。
    pub content: String,
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub reply_to_id: Option<Id<Message>>,
    /// 可选的去重键（1～64 位字母、数字、`-`、`_`，例如 UUID）。同一发送者重复提交时返回第一次的消息。
    #[serde(default)]
    pub client_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditMessageRequest {
    /// 1～4000 字。
    pub content: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MessagesQuery {
    pub cursor: Option<String>,
    /// 默认 50，最大 100。
    pub limit: Option<u32>,
}
