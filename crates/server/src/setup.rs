//! 首次安装：站里还没有管理员时，在 `/admin/setup` 创建第一个管理员账号。
//!
//! 装好以后接口一律返回 409 `ALREADY_SET_UP`。不想开网页时用 `ik-server setup`。

use axum::Json;
use axum::extract::State;
use http::StatusCode;
use ik_kernel::Problem;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AppState;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(setup_status, setup))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    /// 还没有管理员，需要先安装。
    pub needed: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupInput {
    pub email: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupResult {
    pub user_id: String,
}

/// 是否需要首次安装
///
/// 公开接口，不需要登录。
#[utoipa::path(
    get,
    path = "/v1/setup",
    tag = "system",
    responses((status = 200, body = SetupStatus))
)]
async fn setup_status(State(state): State<AppState>) -> Result<Json<SetupStatus>, Problem> {
    let needed = !ik_auth::service::admin_exists(&state.db).await?;
    Ok(Json(SetupStatus { needed }))
}

/// 创建第一个管理员
///
/// 只在还没有管理员时可用。成功后用同样的邮箱和密码正常登录。
#[utoipa::path(
    post,
    path = "/v1/setup",
    tag = "system",
    request_body = SetupInput,
    responses(
        (status = 201, body = SetupResult),
        (status = 409, description = "ALREADY_SET_UP / EMAIL_TAKEN / USERNAME_TAKEN", body = ik_kernel::error::ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn setup(
    State(state): State<AppState>,
    ik_kernel::extract::Json(input): ik_kernel::extract::Json<SetupInput>,
) -> Result<(StatusCode, Json<SetupResult>), Problem> {
    if ik_auth::service::admin_exists(&state.db).await? {
        return Err(ik_auth::service::already_set_up());
    }
    let username = ik_profile::service::validate_username(&input.username)?;
    let user_id =
        ik_auth::service::create_first_admin(&state.auth, &input.email, &username, input.password)
            .await?;
    tracing::info!(user_id, "首次安装完成，已创建管理员");
    Ok((
        StatusCode::CREATED,
        Json(SetupResult {
            user_id: user_id.to_string(),
        }),
    ))
}

/// 启动时调用：还没有管理员就在日志里提醒一下。
pub async fn announce(state: &AppState) -> Result<(), Problem> {
    if ik_auth::service::admin_exists(&state.db).await? {
        return Ok(());
    }
    tracing::warn!("还没有管理员：打开 /admin/setup 创建第一个管理员（或运行 ik-server setup）");
    Ok(())
}
