//! 表情包：公开清单与管理后台（CMT-007～009）。
//!
//! 评论正文里写作 `:ik-<code>:`，服务端只按文本存、只数个数（comments 模块）；客户端按清单把它渲染成图片。
//! 下架的表情仍在清单里（`selectable = false`），旧评论照常显示；删除后旧评论里只剩文字。
//! 表情代码建好后不能改。查看：版主或管理员；增删改：只有管理员。

use std::sync::Arc;

use axum::extract::{FromRef, State};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use ik_auth::admin::{self, Audit};
use ik_auth::{AdminOperator, AuthState, Operator, StaffOperator};
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path};
use ik_kernel::id::marker::Media;
use ik_kernel::{FieldError, Id, Problem};
use ik_media::dto::MediaView;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::CosmeticsState;
use crate::dto::{EmoteGroupView, EmoteManifest, EmoteView};
use crate::repo::{self, NewEmote, NewEmoteGroup};

const GROUP_NAME_MAX_CHARS: usize = 20;
const EMOTE_NAME_MAX_CHARS: usize = 30;
const CODE_PREFIX: &str = "ik-";
const CODE_MAX_LEN: usize = 32;
const CACHE_CONTROL: &str = "public, max-age=60, stale-while-revalidate=600";

// ---------- 公开清单 ----------

/// 全部表情与启用中的分组。进程内缓存，后台修改后立即失效（多实例时其他实例最多晚 60 秒）。
pub async fn manifest(state: &CosmeticsState) -> Result<Arc<EmoteManifest>, Problem> {
    state
        .emotes
        .try_get_with((), build_manifest(state))
        .await
        .map_err(|error| Problem::clone(&error))
}

async fn build_manifest(state: &CosmeticsState) -> Result<Arc<EmoteManifest>, Problem> {
    let mut conn = state.db.acquire().await?;
    let group_rows = repo::emote_groups(&mut conn).await?;
    let emote_rows = repo::emotes(&mut conn).await?;
    let mut media: Vec<i64> = emote_rows.iter().map(|e| e.image_media_id).collect();
    media.extend(group_rows.iter().filter_map(|g| g.icon_media_id));
    let views = state.media.views(&mut conn, &media).await?;

    let groups: Vec<EmoteGroupView> = group_rows
        .iter()
        .filter(|g| g.active)
        .map(|g| EmoteGroupView {
            id: Id::new(g.id),
            name: g.name.clone(),
            // 分组图标用原图：表情图都很小，动图也要保留。
            icon_url: g
                .icon_media_id
                .and_then(|m| views.get(&m))
                .map(|v| v.url.clone()),
        })
        .collect();
    let emotes: Vec<EmoteView> = emote_rows
        .into_iter()
        .filter_map(|e| {
            let image = views.get(&e.image_media_id)?;
            let group_id = e
                .group_id
                .filter(|g| groups.iter().any(|v| v.id.get() == *g));
            // 所在分组被停用时，组里的表情也不出现在面板里。
            let group_off = e.group_id.is_some() && group_id.is_none();
            Some(EmoteView {
                code: e.code,
                name: e.name,
                group_id: group_id.map(Id::new),
                url: image.url.clone(),
                width: image.width,
                height: image.height,
                selectable: e.active && !group_off,
            })
        })
        .collect();
    let digest = Sha256::digest(
        serde_json::to_vec(&(&groups, &emotes)).map_err(|e| Problem::internal_from(&e))?,
    );
    let version = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    Ok(Arc::new(EmoteManifest {
        version,
        groups,
        emotes,
    }))
}

// ---------- 管理后台 ----------

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminEmoteGroup {
    #[schema(value_type = String)]
    pub id: Id,
    pub name: String,
    pub sort_order: i32,
    #[schema(value_type = Option<String>)]
    pub icon_id: Option<Id<Media>>,
    pub icon: Option<MediaView>,
    pub active: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminEmote {
    #[schema(value_type = String)]
    pub id: Id,
    pub code: String,
    pub name: String,
    #[schema(value_type = Option<String>)]
    pub group_id: Option<Id>,
    #[schema(value_type = String)]
    pub image_id: Id<Media>,
    pub image: Option<MediaView>,
    pub sort_order: i32,
    pub active: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AdminEmotes {
    pub groups: Vec<AdminEmoteGroup>,
    pub emotes: Vec<AdminEmote>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmoteGroupInput {
    /// 1–20 字，不能重名。
    pub name: String,
    /// 小的在前。
    #[serde(default)]
    pub sort_order: i32,
    /// 分组图标（媒体 id），不设时客户端用组里第一个表情。
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub icon_id: Option<Id<Media>>,
    #[serde(default = "default_true")]
    pub active: bool,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmoteCreateInput {
    /// `ik-` 开头，后面 1–32 个小写字母、数字或连字符。建好后不能改。
    pub code: String,
    /// 1–30 字（面板里的提示文字）。
    pub name: String,
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub group_id: Option<Id>,
    #[schema(value_type = String)]
    pub image_id: Id<Media>,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default = "default_true")]
    pub active: bool,
}

impl EmoteCreateInput {
    fn split(self) -> (String, EmoteInput) {
        (
            self.code.trim().to_owned(),
            EmoteInput {
                name: self.name,
                group_id: self.group_id,
                image_id: self.image_id,
                sort_order: self.sort_order,
                active: self.active,
            },
        )
    }
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmoteInput {
    /// 1–30 字（面板里的提示文字）。
    pub name: String,
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub group_id: Option<Id>,
    #[schema(value_type = String)]
    pub image_id: Id<Media>,
    /// 小的在前。
    #[serde(default)]
    pub sort_order: i32,
    /// 下架后不出现在面板里，旧评论照常显示。
    #[serde(default = "default_true")]
    pub active: bool,
}

fn default_true() -> bool {
    true
}

fn field_error(field: &str, code: &str, detail: impl Into<String>) -> FieldError {
    FieldError {
        field: field.to_owned(),
        code: code.to_owned(),
        detail: detail.into(),
    }
}

fn check_name(field: &str, value: &str, max: usize, errors: &mut Vec<FieldError>) {
    let n = value.chars().count();
    if n == 0 || n > max {
        errors.push(field_error(
            field,
            "INVALID",
            format!("长度需在 1–{max} 字之间"),
        ));
    }
}

/// 与评论里的写法 `:(ik-[a-z0-9-]{1,32}):` 一致（CMT-008）。
fn valid_code(code: &str) -> bool {
    code.strip_prefix(CODE_PREFIX).is_some_and(|rest| {
        !rest.is_empty()
            && rest.len() <= CODE_MAX_LEN
            && rest
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

async fn check_media(
    state: &CosmeticsState,
    field: &str,
    id: Option<Id<Media>>,
    errors: &mut Vec<FieldError>,
) -> Result<(), Problem> {
    let Some(id) = id else { return Ok(()) };
    let mut conn = state.db.acquire().await?;
    if state.media.views(&mut conn, &[id.get()]).await?.is_empty() {
        errors.push(field_error(field, "NOT_FOUND", "图片不存在"));
    }
    Ok(())
}

fn group_name_taken() -> Problem {
    Problem::conflict("ALREADY_EXISTS", "已有同名的表情分组")
}

fn code_taken() -> Problem {
    Problem::conflict("ALREADY_EXISTS", "已有相同代码的表情")
}

pub async fn admin_list(state: &CosmeticsState) -> Result<AdminEmotes, Problem> {
    let mut conn = state.db.acquire().await?;
    let groups = repo::emote_groups(&mut conn).await?;
    let emotes = repo::emotes(&mut conn).await?;
    let mut media: Vec<i64> = emotes.iter().map(|e| e.image_media_id).collect();
    media.extend(groups.iter().filter_map(|g| g.icon_media_id));
    let views = state.media.views(&mut conn, &media).await?;
    Ok(AdminEmotes {
        groups: groups
            .into_iter()
            .map(|g| AdminEmoteGroup {
                id: Id::new(g.id),
                name: g.name,
                sort_order: g.sort_order,
                icon_id: g.icon_media_id.map(Id::new),
                icon: g.icon_media_id.and_then(|m| views.get(&m).cloned()),
                active: g.active,
            })
            .collect(),
        emotes: emotes
            .into_iter()
            .map(|e| AdminEmote {
                id: Id::new(e.id),
                code: e.code,
                name: e.name,
                group_id: e.group_id.map(Id::new),
                image_id: Id::new(e.image_media_id),
                image: views.get(&e.image_media_id).cloned(),
                sort_order: e.sort_order,
                active: e.active,
            })
            .collect(),
    })
}

/// 新建（`id` 为 None）或修改分组。
pub async fn save_group(
    state: &CosmeticsState,
    op: &Operator,
    id: Option<i64>,
    input: EmoteGroupInput,
) -> Result<AdminEmoteGroup, Problem> {
    let name = input.name.trim();
    let mut errors = Vec::new();
    check_name("name", name, GROUP_NAME_MAX_CHARS, &mut errors);
    check_media(state, "iconId", input.icon_id, &mut errors).await?;
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }
    let target = id.unwrap_or_else(|| state.ids.next_raw());
    let group = NewEmoteGroup {
        id: target,
        name,
        sort_order: input.sort_order,
        icon_media_id: input.icon_id.map(Id::get),
        active: input.active,
    };
    let mut tx = state.db.begin().await?;
    let written = match id {
        None => repo::insert_emote_group(&mut tx, &group)
            .await
            .map(|()| true),
        Some(_) => repo::update_emote_group(&mut tx, &group).await,
    };
    match written {
        Ok(true) => {}
        Ok(false) => return Err(Problem::not_found()),
        Err(error) if ik_kernel::db::is_unique_violation(&error, "emote_groups_name_uq") => {
            return Err(group_name_taken());
        }
        Err(error) => return Err(error.into()),
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: if id.is_some() {
                "emotes.group.update"
            } else {
                "emotes.group.create"
            },
            target_type: "emote_group",
            target_id: Some(target.to_string()),
            details: serde_json::json!({
                "name": name, "sortOrder": input.sort_order, "iconId": input.icon_id, "active": input.active
            }),
        },
        Utc::now(),
    )
    .await?;
    tx.commit().await?;
    state.emotes.invalidate_all();
    admin_list(state)
        .await?
        .groups
        .into_iter()
        .find(|g| g.id.get() == target)
        .ok_or_else(Problem::internal)
}

/// 删除分组：组里的表情留下、变成未分组。
pub async fn delete_group(state: &CosmeticsState, op: &Operator, id: i64) -> Result<(), Problem> {
    let mut tx = state.db.begin().await?;
    let Some(members) = repo::delete_emote_group(&mut tx, id).await? else {
        return Err(Problem::not_found());
    };
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "emotes.group.delete",
            target_type: "emote_group",
            target_id: Some(id.to_string()),
            details: serde_json::json!({ "ungroupedEmotes": members }),
        },
        Utc::now(),
    )
    .await?;
    tx.commit().await?;
    state.emotes.invalidate_all();
    Ok(())
}

/// 新建（`id` 为 None，带 `code`）或修改表情（代码不能改）。
pub async fn save_emote(
    state: &CosmeticsState,
    op: &Operator,
    id: Option<i64>,
    code: Option<&str>,
    input: EmoteInput,
) -> Result<AdminEmote, Problem> {
    let name = input.name.trim();
    let mut errors = Vec::new();
    if let Some(code) = code
        && !valid_code(code)
    {
        errors.push(field_error(
            "code",
            "INVALID",
            "代码须以 ik- 开头，后面 1–32 个小写字母、数字或连字符",
        ));
    }
    check_name("name", name, EMOTE_NAME_MAX_CHARS, &mut errors);
    check_media(state, "imageId", Some(input.image_id), &mut errors).await?;
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }
    let target = id.unwrap_or_else(|| state.ids.next_raw());
    let mut tx = state.db.begin().await?;
    // 分组可能同时被删：外键兜底，这里给出友好的错误。
    if let Some(group) = input.group_id
        && !repo::emote_group_exists(&mut tx, group.get()).await?
    {
        return Err(Problem::field("groupId", "NOT_FOUND", "分组不存在"));
    }
    let emote = NewEmote {
        id: target,
        code: code.unwrap_or_default(),
        name,
        group_id: input.group_id.map(Id::get),
        image_media_id: input.image_id.get(),
        sort_order: input.sort_order,
        active: input.active,
    };
    let written = match id {
        None => repo::insert_emote(&mut tx, &emote).await.map(|()| true),
        Some(_) => repo::update_emote(&mut tx, &emote).await,
    };
    match written {
        Ok(true) => {}
        Ok(false) => return Err(Problem::not_found()),
        Err(error) if ik_kernel::db::is_unique_violation(&error, "emotes_code_uq") => {
            return Err(code_taken());
        }
        Err(error) => return Err(error.into()),
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: if id.is_some() {
                "emotes.emote.update"
            } else {
                "emotes.emote.create"
            },
            target_type: "emote",
            target_id: Some(target.to_string()),
            details: serde_json::json!({
                "code": code, "name": name, "groupId": input.group_id, "imageId": input.image_id,
                "sortOrder": input.sort_order, "active": input.active
            }),
        },
        Utc::now(),
    )
    .await?;
    tx.commit().await?;
    state.emotes.invalidate_all();
    admin_list(state)
        .await?
        .emotes
        .into_iter()
        .find(|e| e.id.get() == target)
        .ok_or_else(Problem::internal)
}

/// 删除表情：旧评论里的 `:code:` 之后只显示成文字。想保留旧评论的显示请改为下架。
pub async fn delete_emote(state: &CosmeticsState, op: &Operator, id: i64) -> Result<(), Problem> {
    let mut tx = state.db.begin().await?;
    let Some(code) = repo::delete_emote(&mut tx, id).await? else {
        return Err(Problem::not_found());
    };
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "emotes.emote.delete",
            target_type: "emote",
            target_id: Some(id.to_string()),
            details: serde_json::json!({ "code": code }),
        },
        Utc::now(),
    )
    .await?;
    tx.commit().await?;
    state.emotes.invalidate_all();
    Ok(())
}

// ---------- HTTP ----------

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    CosmeticsState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(get_manifest))
        .routes(routes!(admin_emotes, post_emote))
        .routes(routes!(put_emote, remove_emote))
        .routes(routes!(post_group))
        .routes(routes!(put_group, remove_group))
}

/// 表情清单
///
/// 全部表情（含已下架的，用于渲染旧评论）与启用中的分组，按面板顺序。表情面板只展示 `selectable` 的；
/// 评论里的 `:ik-xxx:` 按 `code` 查表渲染，查不到的原样显示文字。
/// 带 `ETag`，客户端用 `If-None-Match` 重新验证（没变时 304）；可以缓存 60 秒。
#[utoipa::path(
    get,
    path = "/v1/emotes",
    tag = "cosmetics",
    responses(
        (status = 200, body = EmoteManifest, headers(("ETag" = String))),
        (status = 304, description = "清单没有变化"),
    )
)]
async fn get_manifest(
    State(state): State<CosmeticsState>,
    headers: HeaderMap,
) -> Result<Response, Problem> {
    let manifest = manifest(&state).await?;
    let etag = format!("\"{}\"", manifest.version);
    let not_modified = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|tag| tag.trim().trim_start_matches("W/") == etag)
        });
    let mut response = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        Json(EmoteManifest::clone(&manifest)).into_response()
    };
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(CACHE_CONTROL),
    );
    Ok(response)
}

/// 表情与分组（后台）
#[utoipa::path(
    get,
    path = "/v1/admin/emotes",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = AdminEmotes))
)]
async fn admin_emotes(
    State(state): State<CosmeticsState>,
    StaffOperator(_): StaffOperator,
) -> Result<Json<AdminEmotes>, Problem> {
    Ok(Json(admin_list(&state).await?))
}

/// 新建表情
#[utoipa::path(
    post,
    path = "/v1/admin/emotes",
    tag = "admin",
    request_body = EmoteCreateInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = AdminEmote),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "ALREADY_EXISTS", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn post_emote(
    State(state): State<CosmeticsState>,
    AdminOperator(op): AdminOperator,
    Json(input): Json<EmoteCreateInput>,
) -> Result<(StatusCode, Json<AdminEmote>), Problem> {
    let (code, input) = input.split();
    Ok((
        StatusCode::CREATED,
        Json(save_emote(&state, &op, None, Some(&code), input).await?),
    ))
}

/// 修改表情（代码不能改）
#[utoipa::path(
    put,
    path = "/v1/admin/emotes/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = EmoteInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminEmote),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_emote(
    State(state): State<CosmeticsState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
    Json(input): Json<EmoteInput>,
) -> Result<Json<AdminEmote>, Problem> {
    Ok(Json(
        save_emote(&state, &op, Some(id.get()), None, input).await?,
    ))
}

/// 删除表情（旧评论里只剩文字；想保留显示请改为下架）
#[utoipa::path(
    delete,
    path = "/v1/admin/emotes/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn remove_emote(
    State(state): State<CosmeticsState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
) -> Result<StatusCode, Problem> {
    delete_emote(&state, &op, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 新建表情分组
#[utoipa::path(
    post,
    path = "/v1/admin/emotes/groups",
    tag = "admin",
    request_body = EmoteGroupInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = AdminEmoteGroup),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "ALREADY_EXISTS", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn post_group(
    State(state): State<CosmeticsState>,
    AdminOperator(op): AdminOperator,
    Json(input): Json<EmoteGroupInput>,
) -> Result<(StatusCode, Json<AdminEmoteGroup>), Problem> {
    Ok((
        StatusCode::CREATED,
        Json(save_group(&state, &op, None, input).await?),
    ))
}

/// 修改表情分组
#[utoipa::path(
    put,
    path = "/v1/admin/emotes/groups/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = EmoteGroupInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminEmoteGroup),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "ALREADY_EXISTS", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_group(
    State(state): State<CosmeticsState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
    Json(input): Json<EmoteGroupInput>,
) -> Result<Json<AdminEmoteGroup>, Problem> {
    Ok(Json(save_group(&state, &op, Some(id.get()), input).await?))
}

/// 删除表情分组（组里的表情变成未分组）
#[utoipa::path(
    delete,
    path = "/v1/admin/emotes/groups/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn remove_group(
    State(state): State<CosmeticsState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
) -> Result<StatusCode, Problem> {
    delete_group(&state, &op, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::valid_code;

    #[test]
    fn codes_match_the_comment_token_format() {
        assert!(valid_code("ik-smile"));
        assert!(valid_code("ik-a1-b2"));
        assert!(valid_code(&format!("ik-{}", "a".repeat(32))));
        assert!(!valid_code(&format!("ik-{}", "a".repeat(33))));
        assert!(!valid_code("ik-"));
        assert!(!valid_code("ik-Smile"));
        assert!(!valid_code("smile"));
        assert!(!valid_code("ik-笑"));
        assert!(!valid_code("ik-a_b"));
    }
}
