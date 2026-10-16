//! 装扮目录的管理后台（media-ops.md ADMINCRUD：avatar、business-card）。
//!
//! 查看：版主或管理员；增删改：只有管理员。图片用后台账号自己上传的媒体文件 id。
//! 系统头像换图时同步装备着它的账号；删除前先卸下（外键不级联）。

use axum::extract::{FromRef, State};
use chrono::Utc;
use http::StatusCode;
use ik_auth::admin::{self, Audit};
use ik_auth::{AdminOperator, AuthState, Operator, StaffOperator};
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path};
use ik_kernel::id::marker::Media;
use ik_kernel::{FieldError, Id, Problem};
use ik_media::dto::MediaView;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::CosmeticsState;
use crate::dto::CosmeticKind;
use crate::repo::{self, NewCard};

const NAME_MAX_CHARS: usize = 50;
const DESCRIPTION_MAX_CHARS: usize = 500;
const STORY_MAX_CHARS: usize = 20_000;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminAvatar {
    #[schema(value_type = String)]
    pub id: Id,
    pub name: String,
    pub kind: CosmeticKind,
    #[schema(value_type = String)]
    pub image_id: Id<Media>,
    pub image: Option<MediaView>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminCard {
    #[schema(value_type = String)]
    pub id: Id,
    pub name: String,
    pub kind: CosmeticKind,
    pub description: String,
    /// 背景故事（Markdown）。
    pub story: String,
    #[schema(value_type = String)]
    pub image_id: Id<Media>,
    pub image: Option<MediaView>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvatarInput {
    pub name: String,
    #[serde(default)]
    pub kind: CosmeticKind,
    #[schema(value_type = String)]
    pub image_id: Id<Media>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CardInput {
    pub name: String,
    #[serde(default)]
    pub kind: CosmeticKind,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub story: String,
    #[schema(value_type = String)]
    pub image_id: Id<Media>,
}

fn check_len(field: &str, value: &str, min: usize, max: usize, errors: &mut Vec<FieldError>) {
    let n = value.chars().count();
    if n < min || n > max {
        errors.push(FieldError {
            field: field.to_owned(),
            code: "INVALID".to_owned(),
            detail: format!("长度需在 {min}–{max} 字之间"),
        });
    }
}

async fn check_image(
    state: &CosmeticsState,
    image: Id<Media>,
    errors: &mut Vec<FieldError>,
) -> Result<(), Problem> {
    if state.media.views(&[image.get()]).await?.is_empty() {
        errors.push(FieldError {
            field: "imageId".to_owned(),
            code: "NOT_FOUND".to_owned(),
            detail: "图片不存在".to_owned(),
        });
    }
    Ok(())
}

fn name_taken() -> Problem {
    Problem::conflict("ALREADY_EXISTS", "已有同名装扮")
}

pub async fn avatars(state: &CosmeticsState) -> Result<Vec<AdminAvatar>, Problem> {
    let mut conn = state.db.acquire().await?;
    let rows = repo::all_avatars(&mut conn).await?;
    drop(conn);
    let media: Vec<i64> = rows.iter().map(|r| r.image_media_id).collect();
    let views = state.media.views(&media).await?;
    Ok(rows
        .into_iter()
        .map(|r| AdminAvatar {
            id: Id::new(r.id),
            name: r.name,
            kind: CosmeticKind::parse(&r.kind),
            image_id: Id::new(r.image_media_id),
            image: views.get(&r.image_media_id).cloned(),
        })
        .collect())
}

pub async fn cards(state: &CosmeticsState) -> Result<Vec<AdminCard>, Problem> {
    let mut conn = state.db.acquire().await?;
    let rows = repo::all_cards(&mut conn).await?;
    drop(conn);
    let media: Vec<i64> = rows.iter().map(|r| r.image_media_id).collect();
    let views = state.media.views(&media).await?;
    Ok(rows
        .into_iter()
        .map(|r| AdminCard {
            id: Id::new(r.id),
            name: r.name,
            kind: CosmeticKind::parse(&r.kind),
            description: r.description,
            story: r.story,
            image_id: Id::new(r.image_media_id),
            image: views.get(&r.image_media_id).cloned(),
        })
        .collect())
}

/// 新建（`id` 为 None）或修改系统头像。
pub async fn save_avatar(
    state: &CosmeticsState,
    op: &Operator,
    id: Option<i64>,
    input: AvatarInput,
) -> Result<AdminAvatar, Problem> {
    let name = input.name.trim();
    let mut errors = Vec::new();
    check_len("name", name, 1, NAME_MAX_CHARS, &mut errors);
    check_image(state, input.image_id, &mut errors).await?;
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let target = id.unwrap_or_else(|| state.ids.next_raw());
    let written = match id {
        None => repo::insert_avatar(
            &mut tx,
            target,
            name,
            input.kind.as_str(),
            input.image_id.get(),
        )
        .await
        .map(|()| true),
        Some(id) => {
            repo::update_avatar(&mut tx, id, name, input.kind.as_str(), input.image_id.get()).await
        }
    };
    match written {
        Ok(true) => {}
        Ok(false) => return Err(Problem::not_found()),
        Err(error) if ik_kernel::db::is_unique_violation(&error, "system_avatars_name_uq") => {
            return Err(name_taken());
        }
        Err(error) => return Err(error.into()),
    }
    // 换图：装备着它的账号同步（COSMETIC-008）。
    ik_auth::interop::refresh_equipped_avatar(&mut tx, target, input.image_id.get()).await?;
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: if id.is_some() {
                "cosmetics.avatar.update"
            } else {
                "cosmetics.avatar.create"
            },
            target_type: "system_avatar",
            target_id: Some(target.to_string()),
            details: serde_json::json!({ "name": name, "kind": input.kind.as_str(), "imageId": input.image_id }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    avatars(state)
        .await?
        .into_iter()
        .find(|a| a.id.get() == target)
        .ok_or_else(Problem::internal)
}

/// 删除系统头像：先让装备着它的账号卸下。
pub async fn delete_avatar(state: &CosmeticsState, op: &Operator, id: i64) -> Result<(), Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let unequipped = ik_auth::interop::unequip_system_avatar(&mut tx, id).await?;
    if !repo::delete_avatar(&mut tx, id).await? {
        return Err(Problem::not_found());
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "cosmetics.avatar.delete",
            target_type: "system_avatar",
            target_id: Some(id.to_string()),
            details: serde_json::json!({ "unequipped": unequipped }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 新建（`id` 为 None）或修改名片。
pub async fn save_card(
    state: &CosmeticsState,
    op: &Operator,
    id: Option<i64>,
    input: CardInput,
) -> Result<AdminCard, Problem> {
    let name = input.name.trim();
    let mut errors = Vec::new();
    check_len("name", name, 1, NAME_MAX_CHARS, &mut errors);
    check_len(
        "description",
        &input.description,
        0,
        DESCRIPTION_MAX_CHARS,
        &mut errors,
    );
    check_len("story", &input.story, 0, STORY_MAX_CHARS, &mut errors);
    check_image(state, input.image_id, &mut errors).await?;
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }
    let now = Utc::now();
    let target = id.unwrap_or_else(|| state.ids.next_raw());
    let card = NewCard {
        id: target,
        name,
        kind: input.kind.as_str(),
        description: &input.description,
        story: &input.story,
        image_media_id: input.image_id.get(),
    };
    let mut tx = state.db.begin().await?;
    let written = match id {
        None => repo::insert_card(&mut tx, &card).await.map(|()| true),
        Some(_) => repo::update_card(&mut tx, &card).await,
    };
    match written {
        Ok(true) => {}
        Ok(false) => return Err(Problem::not_found()),
        Err(error) if ik_kernel::db::is_unique_violation(&error, "business_cards_name_uq") => {
            return Err(name_taken());
        }
        Err(error) => return Err(error.into()),
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: if id.is_some() {
                "cosmetics.card.update"
            } else {
                "cosmetics.card.create"
            },
            target_type: "business_card",
            target_id: Some(target.to_string()),
            details: serde_json::json!({ "name": name, "kind": input.kind.as_str(), "imageId": input.image_id }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    cards(state)
        .await?
        .into_iter()
        .find(|c| c.id.get() == target)
        .ok_or_else(Problem::internal)
}

pub async fn delete_card(state: &CosmeticsState, op: &Operator, id: i64) -> Result<(), Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let unequipped = ik_auth::interop::unequip_card(&mut tx, id).await?;
    if !repo::delete_card(&mut tx, id).await? {
        return Err(Problem::not_found());
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "cosmetics.card.delete",
            target_type: "business_card",
            target_id: Some(id.to_string()),
            details: serde_json::json!({ "unequipped": unequipped }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
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
        .routes(routes!(list_avatars, post_avatar))
        .routes(routes!(put_avatar, remove_avatar))
        .routes(routes!(list_cards, post_card))
        .routes(routes!(put_card, remove_card))
}

/// 系统头像目录
#[utoipa::path(
    get,
    path = "/v1/admin/cosmetics/avatars",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = Vec<AdminAvatar>))
)]
async fn list_avatars(
    State(state): State<CosmeticsState>,
    StaffOperator(_): StaffOperator,
) -> Result<Json<Vec<AdminAvatar>>, Problem> {
    Ok(Json(avatars(&state).await?))
}

/// 新建系统头像
#[utoipa::path(
    post,
    path = "/v1/admin/cosmetics/avatars",
    tag = "admin",
    request_body = AvatarInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = AdminAvatar),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn post_avatar(
    State(state): State<CosmeticsState>,
    AdminOperator(op): AdminOperator,
    Json(input): Json<AvatarInput>,
) -> Result<(StatusCode, Json<AdminAvatar>), Problem> {
    Ok((
        StatusCode::CREATED,
        Json(save_avatar(&state, &op, None, input).await?),
    ))
}

/// 修改系统头像
#[utoipa::path(
    put,
    path = "/v1/admin/cosmetics/avatars/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = AvatarInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminAvatar),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_avatar(
    State(state): State<CosmeticsState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
    Json(input): Json<AvatarInput>,
) -> Result<Json<AdminAvatar>, Problem> {
    Ok(Json(save_avatar(&state, &op, Some(id.get()), input).await?))
}

/// 删除系统头像（装备着它的账号自动卸下）
#[utoipa::path(
    delete,
    path = "/v1/admin/cosmetics/avatars/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn remove_avatar(
    State(state): State<CosmeticsState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
) -> Result<StatusCode, Problem> {
    delete_avatar(&state, &op, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 论坛名片目录
#[utoipa::path(
    get,
    path = "/v1/admin/cosmetics/cards",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = Vec<AdminCard>))
)]
async fn list_cards(
    State(state): State<CosmeticsState>,
    StaffOperator(_): StaffOperator,
) -> Result<Json<Vec<AdminCard>>, Problem> {
    Ok(Json(cards(&state).await?))
}

/// 新建名片
#[utoipa::path(
    post,
    path = "/v1/admin/cosmetics/cards",
    tag = "admin",
    request_body = CardInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = AdminCard),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn post_card(
    State(state): State<CosmeticsState>,
    AdminOperator(op): AdminOperator,
    Json(input): Json<CardInput>,
) -> Result<(StatusCode, Json<AdminCard>), Problem> {
    Ok((
        StatusCode::CREATED,
        Json(save_card(&state, &op, None, input).await?),
    ))
}

/// 修改名片
#[utoipa::path(
    put,
    path = "/v1/admin/cosmetics/cards/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = CardInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminCard),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn put_card(
    State(state): State<CosmeticsState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
    Json(input): Json<CardInput>,
) -> Result<Json<AdminCard>, Problem> {
    Ok(Json(save_card(&state, &op, Some(id.get()), input).await?))
}

/// 删除名片（装备着它的账号自动卸下）
#[utoipa::path(
    delete,
    path = "/v1/admin/cosmetics/cards/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 204),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn remove_card(
    State(state): State<CosmeticsState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
) -> Result<StatusCode, Problem> {
    delete_card(&state, &op, id.get()).await?;
    Ok(StatusCode::NO_CONTENT)
}
