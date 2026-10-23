//! 对外的请求与响应类型。

use chrono::{DateTime, Utc};
use ik_kernel::Id;
use ik_kernel::id::marker::{Conversation, Media, Message, User};
use ik_media::dto::MediaView;
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
    /// 头像（360 宽缩略图）。已注销、没有设置时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
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

/// AI 回复的展示信息（AI-009：只下发状态与模型）。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AiReplyMeta {
    /// `streaming`（生成中，正文随 `dm.message.delta` 增长）| `done` | `failed` | `stopped`
    pub state: String,
    /// 本条回复所用模型（`/v1/ai/models` 的 key）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_key: Option<String>,
    /// 工作流时间线（WF-003，已脱敏；定稿后才有）。生成中的事件经 `dm.message.workflow` 实时推送。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow: Option<Vec<WorkflowEvent>>,
}

/// AI 回复的一个工作流事件（WF-001、第 7.1 节的事件类型表）。
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEvent {
    /// 如 `reasoning.start`、`forum.search.finish`、`answer.delta`、`citation.add`、`answer.finish`。
    #[serde(rename = "type")]
    pub kind: String,
    /// 同一步骤的开始 / 中间 / 结束共享（形如 `s<n>`）。
    pub step_id: String,
    /// 单调递增（去重、排序；同一回复重试后继续递增）。
    pub seq: i64,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub data: Option<serde_json::Value>,
    /// 只出现在 `answer.finish`：`{ promptTokens, completionTokens }`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub usage: Option<serde_json::Value>,
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
    /// 已撤回时省略。带图的消息可以为空串。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 图片（有序，MSG-030）。没有图片、已撤回时省略。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<MediaView>,
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
    /// AI 回复才有。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai: Option<AiReplyMeta>,
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
    /// `direct`（一对一私聊）| `ai`（AI 会话，同一个 AI 可以有多条）。
    pub kind: String,
    /// AI 会话的标题。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
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
    /// 1～4000 字（首尾空白会被去掉）；带了图片时可以为空。
    #[serde(default)]
    pub content: String,
    /// 图片（自己图库里的媒体 id，有序，最多 4 张，MSG-030）。
    #[serde(default)]
    #[schema(value_type = Vec<String>)]
    pub image_ids: Vec<Id<Media>>,
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
