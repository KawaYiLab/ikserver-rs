//! 对外的请求与响应类型。

use chrono::{DateTime, Utc};
use ik_kernel::Id;
use ik_kernel::id::marker::{Category, Post, User};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::video::ExternalVideo;

// ---------- 分区与标签 ----------

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CategoryView {
    #[schema(value_type = String, example = "7385159247626227712")]
    pub id: Id<Category>,
    pub slug: String,
    pub name: String,
    pub description: String,
    /// 只有管理员能在此分区发帖。
    pub admin_only: bool,
    /// 帖主可以采纳评论。
    pub allow_comment_accept: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRef {
    #[schema(value_type = String)]
    pub id: Id<Category>,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TagRef {
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TagSuggestQuery {
    /// 关键词（按 slug 子串匹配）。为空时按字母序返回。
    #[serde(default)]
    pub q: String,
    /// 默认 8，最大 10。
    pub limit: Option<u32>,
}

// ---------- 写入 ----------

/// 帖子的完整工作副本。保存草稿与发布都整份提交（PUT 语义）。
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PostInput {
    /// 1–50 个字。
    #[schema(example = "求助：零号空洞第三层怎么过")]
    pub title: String,
    /// Markdown 正文。上限随等级变化（Lv1 3000 字 … Lv7 16000 字）。
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub category_id: Option<Id<Category>>,
    /// 标签名，最多 5 个，服务端归一化并按 slug 去重。不存在的标签会自动创建。
    #[serde(default)]
    pub tags: Vec<String>,
    /// 匿名发布。发布后不能再切换。
    #[serde(default)]
    pub is_anonymous: bool,
    /// B 站视频链接 / BV 号 / av 号，最多 1 个。
    #[serde(default)]
    pub external_videos: Vec<String>,
    /// 编辑器内部状态，原样保存，只回给作者本人。
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub editor_state: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CreatePostQuery {
    /// `true` 直接发布；默认存为草稿。
    #[serde(default)]
    pub publish: bool,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnpublishRequest {
    /// 有未发布的修改时：`true` 丢弃，`false`（默认）保留进草稿。
    #[serde(default)]
    pub discard_pending: bool,
}

// ---------- 读取 ----------

/// 作者展示。匿名帖与已注销用户的 `id`、`level` 都为空。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthorView {
    #[schema(value_type = Option<String>)]
    pub id: Option<Id<User>>,
    pub name: String,
    pub level: Option<i16>,
    pub anonymous: bool,
}

/// 信息流卡片。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PostCard {
    #[schema(value_type = String)]
    pub id: Id<Post>,
    pub title: String,
    /// 正文开头（最多 120 字，空白折叠）。
    pub excerpt: String,
    pub author: AuthorView,
    pub category: Option<CategoryRef>,
    pub tags: Vec<TagRef>,
    pub is_anonymous: bool,
    pub is_pinned: bool,
    pub is_solved: bool,
    pub view_count: i64,
    pub like_count: i32,
    pub comment_count: i32,
    pub published_at: DateTime<Utc>,
    /// 信息流排序时间（发布或最近一次新评论）。
    pub bumped_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    /// 最近一次顶帖的是不是当前用户自己。
    pub bumped_by_self: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeedPage {
    /// 置顶帖。只在第一页、且未按标签筛选时返回；此时 `items` 不再包含置顶帖。
    pub pinned: Vec<PostCard>,
    pub items: Vec<PostCard>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct FeedQuery {
    /// 分区 slug。
    pub category: Option<String>,
    /// 标签 slug。
    pub tag: Option<String>,
    pub cursor: Option<String>,
    /// 默认 20，最大 50。
    pub limit: Option<u32>,
}

/// 公开详情。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PostDetail {
    #[schema(value_type = String)]
    pub id: Id<Post>,
    pub title: String,
    pub body: String,
    pub author: AuthorView,
    pub category: Option<CategoryRef>,
    pub tags: Vec<TagRef>,
    pub external_videos: Vec<ExternalVideo>,
    pub is_anonymous: bool,
    pub is_pinned: bool,
    pub pinned_at: Option<DateTime<Utc>>,
    pub solved_at: Option<DateTime<Utc>>,
    pub allow_comment_accept: bool,
    pub view_count: i64,
    pub like_count: i32,
    pub comment_count: i32,
    pub favorite_count: i32,
    pub denny_count: i32,
    pub published_at: DateTime<Utc>,
    pub first_published_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub bumped_at: DateTime<Utc>,
    /// 当前用户是作者。匿名帖也会如实告诉作者本人（只对作者本人为 true）。
    pub is_owner: bool,
    /// 被隐藏（只有作者本人能看到被隐藏的帖子）。
    pub is_hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PostStatus {
    Draft,
    Published,
}

/// 作者编辑视图：当前工作副本（有未发布修改时就是修改后的内容）。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PostEditView {
    #[schema(value_type = String)]
    pub id: Id<Post>,
    pub status: PostStatus,
    pub title: String,
    pub body: String,
    #[schema(value_type = Option<String>)]
    pub category_id: Option<Id<Category>>,
    pub tags: Vec<TagRef>,
    pub is_anonymous: bool,
    /// 作者原始输入的视频链接，编辑时原样回填。
    pub external_videos: Vec<String>,
    #[schema(value_type = Option<Object>)]
    pub editor_state: Option<serde_json::Value>,
    /// 乐观并发版本号，同时以 `ETag: "rev:<n>"` 返回；保存时带 `If-Match` 防止覆盖。
    pub revision: i32,
    /// 已发布帖子有尚未发布的修改。
    pub has_pending_changes: bool,
    pub published_at: Option<DateTime<Utc>>,
    pub first_published_at: Option<DateTime<Utc>>,
    pub is_hidden: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MyPostsState {
    #[default]
    All,
    Draft,
    Published,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MyPostsQuery {
    #[serde(default)]
    #[param(inline)]
    pub state: MyPostsState,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

/// 我的帖子列表项。已发布的匿名帖不在列表里（不可编辑，也避免被旁人看到）。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MyPostItem {
    #[schema(value_type = String)]
    pub id: Id<Post>,
    pub status: PostStatus,
    pub title: String,
    pub is_anonymous: bool,
    pub has_pending_changes: bool,
    pub is_hidden: bool,
    pub view_count: i64,
    pub like_count: i32,
    pub comment_count: i32,
    pub published_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ViewResult {
    /// 这次是否计数（同一人 5 分钟内重复浏览、或请求过于频繁时不计）。
    pub counted: bool,
    /// 计数时返回最新浏览量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_count: Option<i64>,
}
