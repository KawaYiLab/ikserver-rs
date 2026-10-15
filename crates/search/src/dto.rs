//! 对外的请求与响应类型。

use ik_content::dto::{AuthorView, CategoryRef};
use ik_kernel::Id;
use ik_kernel::id::marker::Post;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// 搜索结果分页。游标不透明；`total` 是估计值（搜索引擎的估算，或数据库兜底窗口内的条数，最多 1000）。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage<T> {
    pub items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub total: u64,
}

impl<T> SearchPage<T> {
    pub(crate) fn empty() -> Self {
        Self {
            items: Vec::new(),
            next_cursor: None,
            total: 0,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PostSearchQuery {
    /// 关键词。去首尾空白，最多取前 64 个字；为空时返回空结果。
    pub q: Option<String>,
    /// 分区 slug。
    pub category: Option<String>,
    /// `relevance`（默认）| `latest`（按信息流时间）| `hot`（按点赞、浏览）。其他值按 relevance。
    pub sort: Option<String>,
    pub cursor: Option<String>,
    /// 默认 20，最大 50。
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SuggestQuery {
    /// 关键词。去首尾空白，最多取前 64 个字；为空时返回空列表。
    pub q: Option<String>,
    pub category: Option<String>,
    /// 默认 8，范围 1～10。
    pub limit: Option<u32>,
}

/// 联想的一条。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SuggestItem {
    #[schema(value_type = String)]
    pub id: Id<Post>,
    pub title: String,
    /// 已做 HTML 转义的标题，命中词包在 `<mark>` 里，可直接作为 HTML 渲染。
    pub title_highlighted: String,
    /// 正文中命中词附近的片段（纯文本，未转义）；数据库兜底时为空串。
    pub excerpt: String,
    pub author: AuthorView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<CategoryRef>,
    pub is_anonymous: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct CommentSearchQuery {
    /// 关键词。去首尾空白，最多取前 32 个字（SRCH-030）；为空时返回空结果。
    pub q: Option<String>,
    /// 只搜这篇帖子下的评论。
    #[param(value_type = Option<String>)]
    pub post_id: Option<Id<Post>>,
    pub cursor: Option<String>,
    /// 默认 20，最大 50。
    pub limit: Option<u32>,
}
