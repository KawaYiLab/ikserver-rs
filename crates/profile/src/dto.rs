//! 对外的请求与响应类型。

use chrono::{DateTime, Utc};
use ik_content::dto::PostCard;
use ik_kernel::Id;
use ik_kernel::id::marker::{Media, Post, User};
use ik_media::dto::MediaView;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// 作者主页统计。只计公开的非匿名内容，作者本人看也一样（PROFILE-015）。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStats {
    pub post_count: i64,
    pub comment_count: i64,
    /// 这些帖子的浏览量之和。
    pub total_views: i64,
    /// 这些帖子下的评论总数（含他人的匿名评论，只计数）。
    pub total_comments: i64,
    /// 这些帖子收到的点赞总数。
    pub total_likes: i64,
}

/// 作者主页。
///
/// - 双方存在拉黑（任一方向）时：签名为空，`stats`、`exp` 省略，关注数为 0（PROFILE-021）。
/// - 对方隐藏了主页时：签名为空，`stats`、`exp` 省略，`isHidden` 为 true，关注数照常返回（PROFILE-022）。
/// - 拉黑优先于隐藏判断。本人看自己永远是完整视图。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    #[schema(value_type = String)]
    pub id: Id<User>,
    pub username: String,
    pub level: i16,
    pub is_ai_agent: bool,
    pub joined_at: DateTime<Utc>,
    pub bio: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<i64>,
    pub is_self: bool,
    /// 这是一份因对方隐藏主页而精简的视图。
    pub is_hidden: bool,
    pub is_blocked_by_me: bool,
    pub has_blocked_me: bool,
    pub is_following: bool,
    pub follower_count: i64,
    pub following_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<ProfileStats>,
    /// 自定义头像；没有设置时省略。展示时装备的系统头像优先（`equippedAvatar`），其次这个，
    /// 都没有时用默认头像。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<MediaView>,
    /// 装备的系统头像。对方隐藏主页时省略（PROFILE-022）；存在拉黑时照常返回（PROFILE-021）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipped_avatar: Option<ik_cosmetics::dto::AvatarView>,
    /// 装备的论坛名片。省略规则同 `equippedAvatar`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipped_card: Option<ik_cosmetics::dto::CardView>,
    /// 绑定的米游社账号上的绝区零主角色。没有绑定、存在拉黑或对方隐藏主页时省略（PROFILE-021/022）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zzz: Option<ik_mihoyo::dto::ZzzProfile>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PostsScope {
    /// 配置了主页精选时只返回精选，否则返回全部。
    #[default]
    Default,
    /// 忽略精选，返回全部。
    All,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ProfilePostsQuery {
    #[serde(default)]
    #[param(inline)]
    pub scope: PostsScope,
    pub cursor: Option<String>,
    /// 默认 10，最大 50。
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePostsPage {
    /// 这一页是主页精选（按作者设定的顺序，最多 6 篇，没有下一页）。
    pub pinned: bool,
    pub items: Vec<PostCard>,
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

/// 自己的资料设置。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MyProfileView {
    pub bio: String,
    pub hidden: bool,
    /// 主页精选的帖子 id（有序）。省略表示未配置：主页按时间展示全部帖子；空数组表示主动清空。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Vec<String>>)]
    pub pinned_post_ids: Option<Vec<Id<Post>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<MediaView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipped_avatar: Option<ik_cosmetics::dto::AvatarView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipped_card: Option<ik_cosmetics::dto::CardView>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateProfileRequest {
    /// 签名，最多 100 字（首尾空白会去掉）。
    pub bio: Option<String>,
    /// 隐藏主页。
    pub hidden: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameRequest {
    /// 3–20 个字，不能含 `@` 和控制字符。区分大小写。
    pub username: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RenameResult {
    pub username: String,
    /// 改名后的丁尼余额。和原名相同时不扣费。
    pub balance: i32,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PinnedPostsRequest {
    /// 精选帖子 id，有序，最多 6 个。`null` 表示恢复默认（按时间展示全部），`[]` 表示主动清空。
    /// 不是自己的、未发布的、匿名的帖子会被静默剔除。
    #[serde(default)]
    pub post_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchQuery {
    /// 关键词，按用户名子串匹配（不区分大小写），最多 32 字。
    #[serde(default)]
    pub q: String,
    pub limit: Option<u32>,
}

/// 用户搜索结果。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserSearchItem {
    #[schema(value_type = String)]
    pub id: Id<User>,
    pub username: String,
    pub level: i16,
    /// 头像（360 宽缩略图），没有设置时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    /// 签名（最多 120 字）。
    pub bio: String,
    pub follower_count: i64,
    /// 未登录时恒为 false。
    pub is_following: bool,
}

/// @ 提及候选。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MentionCandidate {
    #[schema(value_type = String)]
    pub id: Id<User>,
    pub username: String,
    pub level: i16,
    /// 头像（360 宽缩略图），没有设置时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetAvatarRequest {
    /// 自己图库里的图片（先走直传上传）。
    #[schema(value_type = String)]
    pub media_id: Id<Media>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AvatarResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<MediaView>,
    /// 操作后的丁尼余额。换成同一张图或清除头像时不扣费。
    pub balance: i32,
}

/// 在线人数与最近在线用户的头像。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PresenceView {
    /// 在线人数：开着网页或 App 的登录用户（去重）加游客连接数。
    pub online: i64,
    /// 最近上线、设置了头像的用户（最多 8 个，新的在前）。隐藏了主页的用户不出现。
    pub users: Vec<PresenceUser>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PresenceUser {
    #[schema(value_type = String)]
    pub id: Id<User>,
    pub username: String,
    /// 头像（360 宽缩略图）。
    pub avatar_url: String,
}
