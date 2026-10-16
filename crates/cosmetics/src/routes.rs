//! HTTP 接口：`/v1/me/avatars`、`/v1/me/cards` 及其装备。

use axum::extract::{FromRef, State};
use ik_auth::{AuthState, CurrentUser};
use ik_kernel::Problem;
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Query};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::CosmeticsState;
use crate::dto::{
    AvatarList, CardPage, CardQuery, EquipAvatarRequest, EquipCardRequest, EquippedAvatar,
    EquippedCard,
};
use crate::service;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    CosmeticsState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(avatars))
        .routes(routes!(equip_avatar))
        .routes(routes!(cards))
        .routes(routes!(equip_card))
        .merge(crate::admin::router())
}

/// 系统头像
///
/// 全部系统头像（按分类、再按上架先后）与自己装备的那个。所有头像免费、对所有人开放。
#[utoipa::path(
    get,
    path = "/v1/me/avatars",
    tag = "cosmetics",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AvatarList),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn avatars(
    State(state): State<CosmeticsState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<AvatarList>, Problem> {
    Ok(Json(service::my_avatars(&state, &principal).await?))
}

/// 装备系统头像
///
/// `avatarId` 为 null 表示卸下。装备后各处展示的头像以系统头像为准；设置自定义头像会卸下系统头像。
/// 不扣费，不要求通过考试。
#[utoipa::path(
    put,
    path = "/v1/me/avatars/equipped",
    tag = "cosmetics",
    request_body = EquipAvatarRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = EquippedAvatar),
        (status = 403, description = "ACCOUNT_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "头像不存在", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn equip_avatar(
    State(state): State<CosmeticsState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<EquipAvatarRequest>,
) -> Result<Json<EquippedAvatar>, Problem> {
    Ok(Json(
        service::equip_avatar(&state, &principal, request).await?,
    ))
}

/// 论坛名片
///
/// 分页（按分类、再按上架先后），可按分类过滤；附带自己装备的名片。
#[utoipa::path(
    get,
    path = "/v1/me/cards",
    tag = "cosmetics",
    params(CardQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = CardPage),
        (status = 400, description = "CURSOR_INVALID / VALIDATION_FAILED（分类不合法）", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn cards(
    State(state): State<CosmeticsState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<CardQuery>,
) -> Result<Json<CardPage>, Problem> {
    Ok(Json(service::my_cards(&state, &principal, query).await?))
}

/// 装备论坛名片
///
/// `cardId` 为 null 表示卸下。不扣费，不要求通过考试。
#[utoipa::path(
    put,
    path = "/v1/me/cards/equipped",
    tag = "cosmetics",
    request_body = EquipCardRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = EquippedCard),
        (status = 403, description = "ACCOUNT_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "名片不存在", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn equip_card(
    State(state): State<CosmeticsState>,
    CurrentUser(principal): CurrentUser,
    Json(request): Json<EquipCardRequest>,
) -> Result<Json<EquippedCard>, Problem> {
    Ok(Json(
        service::equip_card(&state, &principal, request).await?,
    ))
}
