//! 评论的管理后台：列表（版主或管理员）。
//!
//! 隐藏、恢复、删除要通知作者、写审核日志，放在 ik-moderation（`/v1/admin/moderation/actions`）。
//! 匿名评论不返回作者，按作者筛选时也排除匿名评论（AGENTS.md 规则 10）。

use axum::extract::{FromRef, State};
use chrono::{DateTime, Utc};
use ik_auth::{AuthState, StaffOperator};
use ik_content::admin::AdminUserBrief;
use ik_kernel::cursor::{Paged, page_window};
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Query};
use ik_kernel::{Id, Problem};
use ik_media::dto::MediaView;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::CommentsState;
use crate::repo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommentVisibility {
    Visible,
    Hidden,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct CommentQuery {
    /// 正文包含；是数字时也按评论 id 匹配。
    pub q: Option<String>,
    #[param(value_type = Option<String>)]
    pub post_id: Option<Id>,
    /// 作者。匿名评论不会出现在按作者筛选的结果里。
    #[param(value_type = Option<String>)]
    pub author_id: Option<Id>,
    #[param(inline)]
    pub visibility: Option<CommentVisibility>,
    pub page: Option<u32>,
    /// 默认 20，最大 100。
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminComment {
    #[schema(value_type = String)]
    pub id: Id,
    #[schema(value_type = String)]
    pub post_id: Id,
    /// 所属帖子的标题（帖子隐藏了也给）。
    pub post_title: Option<String>,
    /// 回复了哪条评论；直接评论帖子、或原评论已删除时为 null。
    #[schema(value_type = Option<String>)]
    pub reply_to_id: Option<Id>,
    pub floor: i32,
    /// 纯文本（提及与表情 token 已转成可读文本）。
    pub text: String,
    pub images: Vec<MediaView>,
    /// 作者；匿名评论为 null。
    pub author: Option<AdminUserBrief>,
    pub is_anonymous: bool,
    /// 匿名评论对外显示的名字。
    pub anonymous_name: Option<String>,
    pub like_count: i32,
    pub pinned: bool,
    pub accepted: bool,
    pub hidden_at: Option<DateTime<Utc>>,
    /// `auto_report` 或 `moderator`。
    pub hidden_reason: Option<String>,
    /// 隐藏它的版主；自动隐藏为 null。
    pub hidden_by: Option<AdminUserBrief>,
    pub created_at: DateTime<Utc>,
}

fn contains_pattern(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 2);
    out.push('%');
    for c in q.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

pub async fn comment_list(
    state: &CommentsState,
    query: CommentQuery,
) -> Result<Paged<AdminComment>, Problem> {
    let (page, limit, offset) = page_window(query.page, query.limit, 20);
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let pattern = q.map(contains_pattern);
    let filter = repo::AdminCommentFilter {
        pattern: pattern.as_deref(),
        id: q.and_then(|q| q.parse().ok()),
        post_id: query.post_id.map(Id::get),
        author_id: query.author_id.map(Id::get),
        hidden: query.visibility.map(|v| v == CommentVisibility::Hidden),
    };
    // 先在一个连接上查完，再去取图片（媒体查询自己取连接，规则 6）。
    let (total, rows, users, titles) = {
        let mut conn = state.db.acquire().await?;
        let total = repo::count_admin_comments(&mut conn, &filter).await?;
        let rows = repo::admin_comments(&mut conn, &filter, i64::from(limit), offset).await?;
        let user_ids: Vec<i64> = rows
            .iter()
            .flat_map(|r| [(!r.is_anonymous).then_some(r.author_id), r.hidden_by])
            .flatten()
            .collect();
        let users = ik_content::admin::user_briefs(&mut conn, &user_ids).await?;
        let mut post_ids: Vec<i64> = rows.iter().map(|r| r.post_id).collect();
        post_ids.sort_unstable();
        post_ids.dedup();
        let titles = ik_content::interop::post_titles(&mut conn, &post_ids).await?;
        (total, rows, users, titles)
    };
    let image_ids: Vec<i64> = rows
        .iter()
        .flat_map(|r| r.image_ids.iter().copied())
        .collect();
    let media = state.media.views(&image_ids).await?;
    let items = rows
        .into_iter()
        .map(|r| AdminComment {
            id: Id::new(r.id),
            post_id: Id::new(r.post_id),
            post_title: titles.get(&r.post_id).cloned(),
            reply_to_id: r.reply_to_id.map(Id::new),
            floor: r.floor,
            text: crate::text::plain_text(&r.body),
            images: r
                .image_ids
                .iter()
                .filter_map(|id| media.get(id).cloned())
                .collect(),
            author: if r.is_anonymous {
                None
            } else {
                users.get(&r.author_id).cloned()
            },
            anonymous_name: if r.is_anonymous {
                r.anonymous_seed
                    .as_deref()
                    .map(ik_content::anonymous::display_name)
            } else {
                None
            },
            is_anonymous: r.is_anonymous,
            like_count: r.like_count,
            pinned: r.pinned_at.is_some(),
            accepted: r.accepted_at.is_some(),
            hidden_at: r.hidden_at,
            hidden_reason: r.hidden_reason,
            hidden_by: r.hidden_by.and_then(|id| users.get(&id).cloned()),
            created_at: r.created_at,
        })
        .collect();
    Ok(Paged {
        items,
        total,
        page,
        limit,
    })
}

// ---------- HTTP ----------

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    CommentsState: FromRef<S>,
{
    OpenApiRouter::new().routes(routes!(list_comments))
}

/// 评论（后台；含隐藏的）
#[utoipa::path(
    get,
    path = "/v1/admin/comments",
    tag = "admin",
    params(CommentQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Paged<AdminComment>),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn list_comments(
    State(state): State<CommentsState>,
    StaffOperator(_): StaffOperator,
    Query(query): Query<CommentQuery>,
) -> Result<Json<Paged<AdminComment>>, Problem> {
    Ok(Json(comment_list(&state, query).await?))
}
