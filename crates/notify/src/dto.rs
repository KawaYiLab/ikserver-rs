//! 对外的请求与响应类型。

use chrono::{DateTime, Utc};
use ik_kernel::Id;
use ik_kernel::id::marker::{Comment, Notification, Post, User};
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
