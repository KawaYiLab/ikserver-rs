//! HTTP 接口：`POST /v1/story/mcp`（MCP Streamable HTTP，无状态模式）。
//!
//! 本服务从不主动推送，所以请求总是直接回 `application/json`，不开 SSE 流、不发 `Mcp-Session-Id`；
//! 只有通知时回 202。路由只注册 POST，`GET`（服务端推送流）与 `DELETE`（结束会话）由 axum 回 405。

use axum::body::Bytes;
use axum::extract::{FromRef, State};
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use ik_auth::AuthState;
use ik_kernel::Problem;
use ik_kernel::error::ProblemBody;
use subtle::ConstantTimeEq;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::StoryState;
use crate::dto::{McpRequest, McpResponse};
use crate::mcp::PROTOCOL_VERSIONS;

/// 只含 MCP 端点（不需要登录体系）。
pub fn mcp_router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    StoryState: FromRef<S>,
{
    OpenApiRouter::new().routes(routes!(mcp))
}

/// MCP 端点 + 管理后台的语料上传与版本管理。
pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    StoryState: FromRef<S>,
    AuthState: FromRef<S>,
{
    mcp_router().merge(crate::admin::router())
}

/// 剧情库 MCP 端点
///
/// 给剧情总结 AI 用的 MCP 服务（Streamable HTTP，无状态）：`ik_catalog` 目录、`ik_search` 检索、
/// `ik_read` 顺读原文、`ik_character` 角色速览、`ik_guide` 方法指引。客户端配置
/// `{"type": "streamable-http", "url": "https://<域名>/v1/story/mcp"}`，另加
/// `Authorization: Bearer <IK_STORY__ACCESS_KEY>` 头（`IK_STORY__PUBLIC=true` 时不需要）。
/// 语料由管理员用 `PUT /v1/admin/story/corpus` 上传。工具参数错误以 `result.isError = true` 返回，协议错误为 JSON-RPC `error`。
#[utoipa::path(
    post,
    path = "/v1/story/mcp",
    tag = "story",
    request_body = McpRequest,
    responses(
        (status = 200, description = "JSON-RPC 响应（批量请求时为数组）", body = McpResponse),
        (status = 202, description = "只有通知或响应，无响应体"),
        (status = 400, description = "MCP_PROTOCOL_UNSUPPORTED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 401, description = "STORY_ACCESS_DENIED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "STORY_DISABLED：还没有上传语料", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn mcp(
    State(state): State<StoryState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Problem> {
    let loaded = state.current().ok_or_else(|| {
        Problem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "STORY_DISABLED",
            "剧情库暂不可用",
        )
    })?;
    if !authorized(&state, &headers) {
        let mut response = Problem::new(
            StatusCode::UNAUTHORIZED,
            "STORY_ACCESS_DENIED",
            "剧情库访问密钥无效",
        )
        .into_response();
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return Ok(response);
    }
    if let Some(version) = headers
        .get("mcp-protocol-version")
        .and_then(|v| v.to_str().ok())
        && !PROTOCOL_VERSIONS.contains(&version.trim())
    {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "MCP_PROTOCOL_UNSUPPORTED",
            "不支持的 MCP 协议版本",
        ));
    }
    Ok(match state.server().handle(&loaded.corpus, &body) {
        Some(json) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            json,
        )
            .into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    })
}

/// `IK_STORY__PUBLIC=true` 时放行；否则要求 `Authorization: Bearer <IK_STORY__ACCESS_KEY>`，
/// 常数时间比较。两者都没配置时谁也进不来（默认不公开游戏文本）。
fn authorized(state: &StoryState, headers: &HeaderMap) -> bool {
    if state.is_public() {
        return true;
    }
    let expected = state.access_key();
    if expected.is_empty() {
        return false;
    }
    let Some(given) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
    else {
        return false;
    };
    expected
        .expose()
        .as_bytes()
        .ct_eq(given.trim().as_bytes())
        .into()
}
