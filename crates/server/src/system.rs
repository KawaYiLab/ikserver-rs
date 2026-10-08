//! 运维探针：`/healthz` 只表示进程存活；`/readyz` 额外确认数据库可用，供负载均衡与蓝绿发布判断。

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use sqlx::Connection;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use ik_kernel::Problem;

use crate::AppState;

#[derive(Debug, Serialize, ToSchema)]
pub struct Health {
    pub status: &'static str,
    pub version: &'static str,
}

const HEALTHY: Health = Health {
    status: "ok",
    version: env!("CARGO_PKG_VERSION"),
};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(healthz))
        .routes(routes!(readyz))
}

/// 存活检查
#[utoipa::path(
    get,
    path = "/healthz",
    tag = "system",
    responses((status = 200, description = "进程存活", body = Health))
)]
async fn healthz() -> Json<Health> {
    Json(HEALTHY)
}

/// 就绪检查（含数据库连通性）
#[utoipa::path(
    get,
    path = "/readyz",
    tag = "system",
    responses(
        (status = 200, description = "可以接收流量", body = Health),
        (status = 503, description = "依赖不可用", body = ik_kernel::error::ProblemBody,
            content_type = "application/problem+json")
    )
)]
async fn readyz(State(state): State<AppState>) -> Result<Json<Health>, Problem> {
    let mut conn = state.db.acquire().await.map_err(|error| {
        tracing::warn!(%error, "readyz: 获取数据库连接失败");
        Problem::service_unavailable()
    })?;
    conn.ping().await.map_err(|error| {
        tracing::warn!(%error, "readyz: 数据库 ping 失败");
        Problem::service_unavailable()
    })?;
    Ok(Json(HEALTHY))
}
