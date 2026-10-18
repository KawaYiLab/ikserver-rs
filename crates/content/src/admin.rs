//! 分区、标签与帖子的管理后台（media-ops.md ADMINCRUD：category、tag）。
//!
//! 分区与标签：查看是版主或管理员，修改只有管理员。写操作与审计日志同一事务。
//! 分区「删除」只允许删没有帖子（含草稿）的；有帖子的请停用（`isActive = false`）。
//!
//! 帖子：列表与移动分区是版主或管理员，置顶只有管理员（与前台置顶一致）。
//! 隐藏、恢复、删除要通知作者、写审核日志，放在 ik-moderation（`/v1/admin/moderation/actions`）。
//! 匿名帖不返回作者，按作者筛选时也排除匿名帖（AGENTS.md 规则 10）。

use axum::extract::{FromRef, State};
use chrono::{DateTime, Utc};
use http::StatusCode;
use ik_auth::admin::{self, Audit};
use ik_auth::{AdminOperator, AuthState, Operator, StaffOperator};
use ik_kernel::bus::Effects;
use ik_kernel::cursor::{Paged, page_window};
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::{Id, Problem};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::ContentState;
use crate::repo::{self, AdminCategoryRow, CategoryUpdate};
use crate::tags;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminCategory {
    #[schema(value_type = String)]
    pub id: Id,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub sort_order: i32,
    pub is_active: bool,
    /// 只有管理员能在该分区发帖。
    pub admin_only: bool,
    /// 帖主可以采纳评论。
    pub allow_comment_accept: bool,
    pub post_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn category_view(row: AdminCategoryRow) -> AdminCategory {
    AdminCategory {
        id: Id::new(row.id),
        slug: row.slug,
        name: row.name,
        description: row.description,
        sort_order: row.sort_order,
        is_active: row.is_active,
        admin_only: row.admin_only,
        allow_comment_accept: row.allow_comment_accept,
        post_count: row.post_count,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CategoryInput {
    /// 小写字母、数字、连字符，1–48。
    pub slug: String,
    /// 1–30 字。
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default = "yes")]
    pub is_active: bool,
    #[serde(default)]
    pub admin_only: bool,
    #[serde(default)]
    pub allow_comment_accept: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminTag {
    #[schema(value_type = String)]
    pub id: Id,
    pub name: String,
    pub slug: String,
    /// 封禁的标签不能再被使用，也不出现在联想与帖子上。
    pub is_blocked: bool,
    pub post_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct TagQuery {
    /// 名称或 slug 包含。
    pub q: Option<String>,
    pub blocked: Option<bool>,
    pub page: Option<u32>,
    /// 默认 50，最大 100。
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TagUpdate {
    /// 改名：按发帖时同样的规则归一化，slug 随名称重新派生（POST-027）。
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub is_blocked: Option<bool>,
}

/// 后台列表里的账号（作者、操作人）。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminUserBrief {
    #[schema(value_type = String)]
    pub id: Id,
    pub username: String,
    /// active / blocked / deleted / merged。
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Visible,
    Hidden,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct PostQuery {
    /// 标题包含；是数字时也按帖子 id 匹配。
    pub q: Option<String>,
    #[param(value_type = Option<String>)]
    pub category_id: Option<Id>,
    /// 作者。匿名帖不会出现在按作者筛选的结果里。
    #[param(value_type = Option<String>)]
    pub author_id: Option<Id>,
    #[param(inline)]
    pub visibility: Option<Visibility>,
    /// 隐藏原因：`auto_report`（多人举报自动隐藏）或 `moderator`（版主隐藏）。
    pub hidden_reason: Option<String>,
    pub pinned: Option<bool>,
    pub anonymous: Option<bool>,
    pub page: Option<u32>,
    /// 默认 20，最大 100。
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminPostCategory {
    #[schema(value_type = String)]
    pub id: Id,
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminPost {
    #[schema(value_type = String)]
    pub id: Id,
    pub title: String,
    pub category: Option<AdminPostCategory>,
    /// 作者；匿名帖为 null。
    pub author: Option<AdminUserBrief>,
    pub is_anonymous: bool,
    /// 匿名帖对外显示的名字。
    pub anonymous_name: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub edited_at: Option<DateTime<Utc>>,
    pub pinned_at: Option<DateTime<Utc>>,
    pub hidden_at: Option<DateTime<Utc>>,
    /// `auto_report` 或 `moderator`。
    pub hidden_reason: Option<String>,
    /// 隐藏它的版主；自动隐藏为 null。
    pub hidden_by: Option<AdminUserBrief>,
    pub view_count: i64,
    pub like_count: i32,
    pub comment_count: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MovePostInput {
    #[schema(value_type = String)]
    pub category_id: Id,
}

fn invalid(field: &str, detail: &str) -> Problem {
    Problem::field(field, "INVALID", detail)
}

/// slug 与名称的校验（与 `create_category` 一致）。
fn checked_category(input: &CategoryInput) -> Result<(String, String), Problem> {
    let slug = input.slug.trim().to_lowercase();
    let slug_ok = !slug.is_empty()
        && slug.len() <= 48
        && slug.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !slug_ok {
        return Err(invalid("slug", "slug 只能包含小写字母、数字和连字符"));
    }
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 30 {
        return Err(invalid("name", "名称需为 1–30 个字"));
    }
    Ok((slug, name.to_owned()))
}

fn slug_taken() -> Problem {
    Problem::conflict("ALREADY_EXISTS", "该 slug 已被使用")
}

pub async fn categories(state: &ContentState) -> Result<Vec<AdminCategory>, Problem> {
    let mut conn = state.db.acquire().await?;
    Ok(repo::admin_categories(&mut conn)
        .await?
        .into_iter()
        .map(category_view)
        .collect())
}

async fn category(state: &ContentState, id: i64) -> Result<AdminCategory, Problem> {
    categories(state)
        .await?
        .into_iter()
        .find(|c| c.id.get() == id)
        .ok_or_else(Problem::not_found)
}

fn category_details(input: &CategoryInput) -> serde_json::Value {
    serde_json::json!({
        "slug": input.slug, "name": input.name, "sortOrder": input.sort_order,
        "isActive": input.is_active, "adminOnly": input.admin_only,
        "allowCommentAccept": input.allow_comment_accept,
    })
}

pub async fn create_category(
    state: &ContentState,
    op: &Operator,
    input: CategoryInput,
) -> Result<AdminCategory, Problem> {
    let (slug, name) = checked_category(&input)?;
    let id = state.ids.next_raw();
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let inserted = repo::insert_category(
        &mut tx,
        &repo::NewCategory {
            id,
            slug: &slug,
            name: &name,
            description: input.description.trim(),
            sort_order: input.sort_order,
            admin_only: input.admin_only,
            allow_comment_accept: input.allow_comment_accept,
        },
    )
    .await;
    match inserted {
        Ok(()) => {}
        Err(error) if ik_kernel::db::is_unique_violation(&error, "categories_slug_uq") => {
            return Err(slug_taken());
        }
        Err(error) => return Err(error.into()),
    }
    if !input.is_active {
        repo::update_category(
            &mut tx,
            &CategoryUpdate {
                id,
                slug: &slug,
                name: &name,
                description: input.description.trim(),
                sort_order: input.sort_order,
                is_active: false,
                admin_only: input.admin_only,
                allow_comment_accept: input.allow_comment_accept,
                now,
            },
        )
        .await?;
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "content.category.create",
            target_type: "category",
            target_id: Some(id.to_string()),
            details: category_details(&input),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    category(state, id).await
}

pub async fn update_category(
    state: &ContentState,
    op: &Operator,
    id: i64,
    input: CategoryInput,
) -> Result<AdminCategory, Problem> {
    let (slug, name) = checked_category(&input)?;
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let updated = repo::update_category(
        &mut tx,
        &CategoryUpdate {
            id,
            slug: &slug,
            name: &name,
            description: input.description.trim(),
            sort_order: input.sort_order,
            is_active: input.is_active,
            admin_only: input.admin_only,
            allow_comment_accept: input.allow_comment_accept,
            now,
        },
    )
    .await;
    match updated {
        Ok(true) => {}
        Ok(false) => return Err(Problem::not_found()),
        Err(error) if ik_kernel::db::is_unique_violation(&error, "categories_slug_uq") => {
            return Err(slug_taken());
        }
        Err(error) => return Err(error.into()),
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "content.category.update",
            target_type: "category",
            target_id: Some(id.to_string()),
            details: category_details(&input),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    category(state, id).await
}

pub async fn delete_category(state: &ContentState, op: &Operator, id: i64) -> Result<(), Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    match repo::delete_empty_category(&mut tx, id).await? {
        None => return Err(Problem::not_found()),
        Some(false) => {
            return Err(Problem::conflict(
                "CATEGORY_IN_USE",
                "分区下还有帖子，不能删除，请改为停用",
            ));
        }
        Some(true) => {}
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "content.category.delete",
            target_type: "category",
            target_id: Some(id.to_string()),
            details: serde_json::json!({}),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(())
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

pub async fn tag_list(state: &ContentState, query: TagQuery) -> Result<Paged<AdminTag>, Problem> {
    let (page, limit, offset) = page_window(query.page, query.limit, 50);
    let pattern = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(contains_pattern);
    let mut conn = state.db.acquire().await?;
    let total = repo::count_admin_tags(&mut conn, pattern.as_deref(), query.blocked).await?;
    let rows = repo::admin_tags(
        &mut conn,
        pattern.as_deref(),
        query.blocked,
        i64::from(limit),
        offset,
    )
    .await?;
    Ok(Paged {
        items: rows
            .into_iter()
            .map(|t| AdminTag {
                id: Id::new(t.id),
                name: t.name,
                slug: t.slug,
                is_blocked: t.is_blocked,
                post_count: t.post_count,
                created_at: t.created_at,
            })
            .collect(),
        total,
        page,
        limit,
    })
}

/// 改名或封禁 / 解封标签。改名后 slug 与别的标签冲突时 409。
pub async fn update_tag(
    state: &ContentState,
    op: &Operator,
    id: i64,
    input: TagUpdate,
) -> Result<AdminTag, Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let current = repo::tags_by_ids(&mut tx, &[id])
        .await?
        .into_iter()
        .next()
        .ok_or_else(Problem::not_found)?;
    let (name, slug) = match input.name.as_deref() {
        None => (current.name.clone(), current.slug.clone()),
        Some(raw) => {
            let name = tags::normalize_name(raw)
                .ok_or_else(|| invalid("name", "标签名需要包含文字或数字"))?;
            let slug = tags::slug(&name).ok_or_else(|| invalid("name", "标签名无法生成 slug"))?;
            (name, slug)
        }
    };
    let is_blocked = input.is_blocked.unwrap_or(current.is_blocked);
    match repo::update_tag(&mut tx, id, &name, &slug, is_blocked).await {
        Ok(_) => {}
        Err(error) if ik_kernel::db::is_unique_violation(&error, "tags_slug_uq") => {
            return Err(Problem::conflict("ALREADY_EXISTS", "已有同名标签"));
        }
        Err(error) => return Err(error.into()),
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "content.tag.update",
            target_type: "tag",
            target_id: Some(id.to_string()),
            details: serde_json::json!({ "from": current.name, "name": name, "isBlocked": is_blocked }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    let mut conn = state.db.acquire().await?;
    let row = repo::admin_tag(&mut conn, id)
        .await?
        .ok_or_else(Problem::internal)?;
    Ok(AdminTag {
        id: Id::new(row.id),
        name: row.name,
        slug: row.slug,
        is_blocked: row.is_blocked,
        post_count: row.post_count,
        created_at: row.created_at,
    })
}

/// 后台列表里的账号简况（按 id）。
pub async fn user_briefs(
    conn: &mut sqlx::PgConnection,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, AdminUserBrief>, Problem> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    Ok(ik_auth::interop::user_statuses(conn, &ids)
        .await?
        .into_values()
        .map(|u| {
            (
                u.id,
                AdminUserBrief {
                    id: Id::new(u.id),
                    username: u.username,
                    status: u.status,
                },
            )
        })
        .collect())
}

pub async fn post_list(
    state: &ContentState,
    query: PostQuery,
) -> Result<Paged<AdminPost>, Problem> {
    let (page, limit, offset) = page_window(query.page, query.limit, 20);
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let pattern = q.map(contains_pattern);
    let hidden_reason = query
        .hidden_reason
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    let filter = repo::AdminPostFilter {
        pattern: pattern.as_deref(),
        id: q.and_then(|q| q.parse().ok()),
        category_id: query.category_id.map(Id::get),
        author_id: query.author_id.map(Id::get),
        hidden: query.visibility.map(|v| v == Visibility::Hidden),
        hidden_reason,
        pinned: query.pinned,
        anonymous: query.anonymous,
    };
    let mut conn = state.db.acquire().await?;
    let total = repo::count_admin_posts(&mut conn, &filter).await?;
    let rows = repo::admin_posts(&mut conn, &filter, i64::from(limit), offset).await?;
    let user_ids: Vec<i64> = rows
        .iter()
        .flat_map(|r| [(!r.is_anonymous).then_some(r.author_id), r.hidden_by])
        .flatten()
        .collect();
    let users = user_briefs(&mut conn, &user_ids).await?;
    let items = rows
        .into_iter()
        .map(|r| AdminPost {
            id: Id::new(r.id),
            title: r.title,
            category: r
                .category_id
                .zip(r.category_name)
                .map(|(id, name)| AdminPostCategory {
                    id: Id::new(id),
                    name,
                }),
            author: if r.is_anonymous {
                None
            } else {
                users.get(&r.author_id).cloned()
            },
            anonymous_name: if r.is_anonymous {
                r.anonymous_seed
                    .as_deref()
                    .map(crate::anonymous::display_name)
            } else {
                None
            },
            is_anonymous: r.is_anonymous,
            published_at: r.first_published_at,
            edited_at: r.edited_at,
            pinned_at: r.pinned_at,
            hidden_at: r.hidden_at,
            hidden_reason: r.hidden_reason,
            hidden_by: r.hidden_by.and_then(|id| users.get(&id).cloned()),
            view_count: r.view_count,
            like_count: r.like_count,
            comment_count: r.comment_count,
        })
        .collect();
    Ok(Paged {
        items,
        total,
        page,
        limit,
    })
}

/// 置顶 / 取消置顶（幂等）。只能置顶公开的帖子。
pub async fn set_pinned(
    state: &ContentState,
    op: &Operator,
    post_id: i64,
    pinned: bool,
) -> Result<(), Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let changed = if pinned {
        repo::pin(&mut tx, post_id, op.principal.user_id.get(), now).await?
    } else {
        repo::unpin(&mut tx, post_id).await?
    };
    if !changed {
        return Err(Problem::not_found());
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: if pinned {
                "content.post.pin"
            } else {
                "content.post.unpin"
            },
            target_type: "post",
            target_id: Some(post_id.to_string()),
            details: serde_json::json!({}),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 移动分区。目标分区必须存在且启用；仅限管理员发帖的分区只有管理员能移进去。
pub async fn move_post(
    state: &ContentState,
    op: &Operator,
    post_id: i64,
    input: MovePostInput,
) -> Result<(), Problem> {
    let category_id = input.category_id.get();
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    match repo::category_by_id(&mut tx, category_id).await? {
        Some(category) if category.is_active => {
            if category.admin_only && !op.principal.role.is_admin() {
                return Err(crate::service::category_admin_only());
            }
        }
        _ => {
            return Err(Problem::field(
                "categoryId",
                "NOT_FOUND",
                "分区不存在或已停用",
            ));
        }
    }
    let Some((from, public)) = repo::lock_published(&mut tx, post_id).await? else {
        return Err(Problem::not_found());
    };
    if from == Some(category_id) {
        return Ok(());
    }
    repo::move_post(&mut tx, post_id, category_id, now).await?;
    if public {
        effects.publish(crate::service::published_event(
            post_id,
            Some(category_id),
            false,
        )?);
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "content.post.move",
            target_type: "post",
            target_id: Some(post_id.to_string()),
            details: serde_json::json!({
                "from": from.map(|id| id.to_string()),
                "to": category_id.to_string(),
            }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(())
}

// ---------- HTTP ----------

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    ContentState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(list_categories, post_category))
        .routes(routes!(put_category, remove_category))
        .routes(routes!(list_tags))
        .routes(routes!(patch_tag))
        .routes(routes!(list_posts))
        .routes(routes!(pin_post, unpin_post))
        .routes(routes!(put_post_category))
}

/// 分区（含停用的，附帖子数）
#[utoipa::path(
    get,
    path = "/v1/admin/categories",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Vec<AdminCategory>),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn list_categories(
    State(state): State<ContentState>,
    StaffOperator(_): StaffOperator,
) -> Result<Json<Vec<AdminCategory>>, Problem> {
    Ok(Json(categories(&state).await?))
}

/// 新建分区
#[utoipa::path(
    post,
    path = "/v1/admin/categories",
    tag = "admin",
    request_body = CategoryInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = AdminCategory),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn post_category(
    State(state): State<ContentState>,
    AdminOperator(op): AdminOperator,
    Json(input): Json<CategoryInput>,
) -> Result<(StatusCode, Json<AdminCategory>), Problem> {
    Ok((
        StatusCode::CREATED,
        Json(create_category(&state, &op, input).await?),
    ))
}

/// 修改分区（整体覆盖：排序、上下架、仅管理员发帖、允许采纳）
#[utoipa::path(
    put,
    path = "/v1/admin/categories/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = CategoryInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminCategory),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_category(
    State(state): State<ContentState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
    Json(input): Json<CategoryInput>,
) -> Result<Json<AdminCategory>, Problem> {
    Ok(Json(update_category(&state, &op, id.get(), input).await?))
}

/// 删除分区
///
/// 只能删除没有帖子（含草稿）的分区，否则 409 `CATEGORY_IN_USE`。
#[utoipa::path(
    delete,
    path = "/v1/admin/categories/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn remove_category(
    State(state): State<ContentState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
) -> Result<StatusCode, Problem> {
    delete_category(&state, &op, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 标签（按使用次数倒序）
#[utoipa::path(
    get,
    path = "/v1/admin/tags",
    tag = "admin",
    params(TagQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = Paged<AdminTag>))
)]
async fn list_tags(
    State(state): State<ContentState>,
    StaffOperator(_): StaffOperator,
    Query(query): Query<TagQuery>,
) -> Result<Json<Paged<AdminTag>>, Problem> {
    Ok(Json(tag_list(&state, query).await?))
}

/// 改名或封禁标签
#[utoipa::path(
    patch,
    path = "/v1/admin/tags/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = TagUpdate,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminTag),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn patch_tag(
    State(state): State<ContentState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
    Json(input): Json<TagUpdate>,
) -> Result<Json<AdminTag>, Problem> {
    Ok(Json(update_tag(&state, &op, id.get(), input).await?))
}

/// 帖子（后台；含隐藏的，不含草稿）
#[utoipa::path(
    get,
    path = "/v1/admin/posts",
    tag = "admin",
    params(PostQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Paged<AdminPost>),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn list_posts(
    State(state): State<ContentState>,
    StaffOperator(_): StaffOperator,
    Query(query): Query<PostQuery>,
) -> Result<Json<Paged<AdminPost>>, Problem> {
    Ok(Json(post_list(&state, query).await?))
}

/// 置顶帖子（只有管理员；只能置顶公开的帖子）
#[utoipa::path(
    put,
    path = "/v1/admin/posts/{id}/pin",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn pin_post(
    State(state): State<ContentState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
) -> Result<StatusCode, Problem> {
    set_pinned(&state, &op, id.get(), true).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 取消置顶（只有管理员）
#[utoipa::path(
    delete,
    path = "/v1/admin/posts/{id}/pin",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unpin_post(
    State(state): State<ContentState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
) -> Result<StatusCode, Problem> {
    set_pinned(&state, &op, id.get(), false).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 移动帖子到另一个分区（待发布的修改一并改）
#[utoipa::path(
    put,
    path = "/v1/admin/posts/{id}/category",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = MovePostInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_post_category(
    State(state): State<ContentState>,
    StaffOperator(op): StaffOperator,
    Path(id): Path<Id>,
    Json(input): Json<MovePostInput>,
) -> Result<StatusCode, Problem> {
    move_post(&state, &op, id.get(), input).await?;
    Ok(StatusCode::NO_CONTENT)
}
