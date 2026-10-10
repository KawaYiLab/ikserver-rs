//! 对外的请求与响应类型。

use chrono::{DateTime, Utc};
use ik_content::dto::AuthorView;
use ik_kernel::Id;
use ik_kernel::id::marker::{Comment, Post};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateCommentRequest {
    /// 1–2000 字。可内嵌表情 `:ik-xxx:`（最多 20 个）与提及 `@[名字](用户id)`；
    /// 直接写 `@用户名` 会自动转成提及。
    #[schema(example = "同问，@绳网用户ab12 你知道吗？")]
    pub body: String,
    /// 回复哪条一级评论。回复「回复」时也填它所属的一级评论 id，并在正文开头提及对方。
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub parent_id: Option<Id<Comment>>,
    #[serde(default)]
    pub is_anonymous: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommentView {
    #[schema(value_type = String)]
    pub id: Id<Comment>,
    #[schema(value_type = String)]
    pub post_id: Id<Post>,
    #[schema(value_type = Option<String>)]
    pub parent_id: Option<Id<Comment>>,
    /// 一级评论的楼层号（发表时确定，不随删除变化）。回复没有楼层号。
    pub floor: Option<i32>,
    pub body: String,
    pub author: AuthorView,
    pub is_anonymous: bool,
    pub like_count: i32,
    pub is_pinned: bool,
    pub is_accepted: bool,
    /// 当前用户就是评论作者（用于显示删除按钮；匿名评论也只对作者本人为 true）。
    pub is_mine: bool,
    pub created_at: DateTime<Utc>,
}

/// 一级评论 + 前 3 条回复。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadView {
    #[serde(flatten)]
    pub comment: CommentView,
    pub replies: Vec<CommentView>,
    /// 可见回复总数。
    pub reply_count: i64,
    pub replies_has_more: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommentsPage {
    /// 采纳的最佳答案（只在第一页返回）。
    pub accepted: Option<ThreadView>,
    /// 置顶评论（只在第一页返回；同时被采纳时只出现在 `accepted`）。
    pub pinned: Option<ThreadView>,
    /// 其余一级评论，新的在前。
    pub items: Vec<ThreadView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    pub cursor: Option<String>,
    /// 默认 20，最大 50。
    pub limit: Option<u32>,
}
