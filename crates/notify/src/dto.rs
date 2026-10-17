//! 对外的请求与响应类型。

use chrono::{DateTime, Utc};
use ik_kernel::Id;
use ik_kernel::id::marker::{Comment, Conversation, Notification, Post, User};
use ik_messaging::dto::MessageView;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// 通知的发送者。已注销的账号 `id`、`level` 为空，名字显示为「已注销用户」。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SenderView {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub id: Option<Id<User>>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i16>,
    /// 头像（360 宽缩略图）。系统、已注销、没有设置时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationView {
    #[schema(value_type = String)]
    pub id: Id<Notification>,
    /// `comment` | `reply` | `mention` | `like` | `favorite` | `denny` | `accepted` | `system`
    pub kind: String,
    /// 所属敲敲会话：`user-<发送者 id>` 或 `system`。
    pub bucket: String,
    /// 系统通知没有发送者。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender: Option<SenderView>,
    /// 一句话概括，例如「赞了你的帖子」；系统通知为正文。
    pub summary: String,
    /// 评论摘要（评论、回复、提及、评论点赞、采纳）或系统通知正文。可能为空串。
    pub excerpt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub post_id: Option<Id<Post>>,
    /// 通知产生时的帖子标题。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub comment_id: Option<Id<Comment>>,
    pub is_read: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    pub cursor: Option<String>,
    /// 默认 20，最大 50。
    pub limit: Option<u32>,
    /// `true` 只看未读。
    #[serde(default)]
    pub unread: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    pub cursor: Option<String>,
    /// 默认 20，最大 50。
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnreadCount {
    pub count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadResult {
    /// 这次从未读变为已读的条数。
    pub updated: u64,
}

/// 敲敲会话：同一个发送者（或系统）的通知归为一组，按最后一条的时间倒序。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BucketView {
    /// `user-<发送者 id>` 或 `system`。
    pub key: String,
    /// 对方。系统会话的名字固定为「Fairy」，没有 id。
    pub peer: SenderView,
    pub unread_count: i64,
    pub last_at: DateTime<Utc>,
    /// 最后一条通知的概括。
    pub last_summary: String,
}

// ---------- 统一收件箱 ----------

/// 收件箱条目的最后一条内容。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxPreview {
    /// `message`（私信）| `notification`（通知）
    pub kind: String,
    /// 私信：前 80 个字或「[消息已撤回]」；通知：一句话概括（如「赞了你的帖子」）。
    pub text: String,
    /// 是否自己发的（只有私信可能为真）。
    pub from_me: bool,
    pub at: DateTime<Utc>,
}

/// 统一收件箱的一项：私聊会话，或某人（或系统）发来的通知分组（KNOCK-010）。
/// 与某人有私聊会话时，他发来的通知合并进那个会话，不再单独成项。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxEntry {
    /// `dm-<会话 id>` | `user-<发送者 id>` | `system`。
    pub key: String,
    /// `conversation` | `notifications`
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub conversation_id: Option<Id<Conversation>>,
    /// 私聊会话的类型：`direct`（一对一私聊）| `ai`（AI 会话）。通知分组没有。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_kind: Option<String>,
    /// AI 会话的标题。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 对方。系统通知的名字固定为「Fairy」，没有 id。
    pub peer: SenderView,
    /// 排序时间：最后一条私信或通知的时间。
    pub activity_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<InboxPreview>,
    /// 未读私信 + 未读通知。
    pub unread_count: i64,
    pub muted: bool,
    pub pinned: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxPage {
    /// 第一页先列出全部置顶会话（不参与分页），其后按 `activityAt` 倒序。
    pub items: Vec<InboxEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// 只在第一页给出：全部未读私信 + 全部未读通知（不止本页，CONV-026）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_unread: Option<i64>,
}

/// 收件箱时间线的一项（新的在前）：私信消息或通知。
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum InboxItem {
    Message { message: MessageView },
    Notification { notification: NotificationView },
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxReadResult {
    /// 私聊会话的新已读水位（通知分组没有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_read_at: Option<DateTime<Utc>>,
    /// 这次从未读变为已读的通知条数。
    pub notifications_updated: u64,
}
