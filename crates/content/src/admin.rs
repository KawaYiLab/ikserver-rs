//! 分区与标签的管理后台（media-ops.md ADMINCRUD：category、tag）。
//!
//! 查看：版主或管理员；修改：只有管理员。写操作与审计日志同一事务。
//! 分区「删除」只允许删没有帖子（含草稿）的；有帖子的请停用（`isActive = false`）。

use axum::extract::{FromRef, State};
use chrono::{DateTime, Utc};
use http::StatusCode;
use ik_auth::admin::{self, Audit};
use ik_auth::{AdminOperator, AuthState, Operator, StaffOperator};
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
