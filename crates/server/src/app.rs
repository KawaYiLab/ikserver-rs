use std::time::Duration;

use anyhow::Context;
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Request};
use axum::response::IntoResponse;
use http::{HeaderName, HeaderValue, Method, StatusCode, header};
use ik_kernel::Problem;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::csrf::CsrfLayer;
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::trace::TraceLayer;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_scalar::{Scalar, Servable};

use crate::middleware::{self, RequestId};
use crate::{AppState, system};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "InterKnot API",
        version = "1",
        description = "绳网论坛后端接口。错误统一为 RFC 9457 Problem Details（application/problem+json）。"
    ),
    components(schemas(ik_kernel::error::ProblemBody, ik_kernel::error::FieldError)),
    modifiers(&SecuritySchemes),
    tags(
        (name = "system", description = "运维探针"),
        (name = "auth", description = "验证码、注册、登录、重置密码、登出"),
        (name = "me", description = "当前用户与登录设备"),
        (name = "content", description = "分区、标签、帖子与信息流"),
    )
)]
pub struct ApiDoc;

/// 网页端用 HttpOnly 会话 Cookie，App 端用 `Authorization: Bearer`。
struct SecuritySchemes;

impl Modify for SecuritySchemes {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "session_cookie",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                "ik_session",
                "网页端登录后由服务端写入的 HttpOnly Cookie",
            ))),
        );
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some("App 端登录响应里的 token"))
                    .build(),
            ),
        );
    }
}

/// 汇总所有模块的路由与 OpenAPI 文档。新业务域在这里 `.merge(...)`。
fn api_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(system::router())
        .merge(ik_auth::router())
        .merge(ik_content::router())
}

/// 完整 OpenAPI 文档（`ik-server openapi` 与 `/v1/openapi.json` 共用）。
pub fn openapi() -> utoipa::openapi::OpenApi {
    api_router().into_openapi()
}

pub fn build_app(state: AppState) -> anyhow::Result<Router> {
    let config = state.config.clone();
    let (router, doc) = api_router().split_for_parts();

    let mut router = router.route(
        "/v1/openapi.json",
        axum::routing::get({
            let doc = doc.clone();
            move || async move { axum::Json(doc) }
        }),
    );
    if !config.env.is_prod() {
        router = router.merge(Scalar::with_url("/docs", doc));
    }

    let mut csrf = CsrfLayer::new();
    for origin in &config.http.allowed_origins {
        csrf = csrf
            .add_trusted_origin(origin)
            .with_context(|| format!("CSRF 白名单来源不合法：{origin}"))?;
    }
    let csrf = csrf.with_rejection_response(|_error| {
        Problem::new(StatusCode::FORBIDDEN, "CSRF_REJECTED", "跨站请求被拒绝").into_response()
    });

    // Router::layer 每调用一次就包在外层，所以这里从最内层往外写。
    // 逐层调用（而不是一个 ServiceBuilder）让 axum 在每层之间统一响应体类型。
    Ok(router
        .fallback(|| async { Problem::not_found() })
        .with_state(state)
        .layer(axum::middleware::from_fn(middleware::timeout(
            Duration::from_secs(config.http.request_timeout_secs),
        )))
        .layer(DefaultBodyLimit::max(config.http.body_limit_bytes))
        .layer(CompressionLayer::new())
        .layer(csrf)
        .layer(cors(&config.http.allowed_origins)?)
        .layer(CatchPanicLayer::custom(|_panic| {
            tracing::error!("handler panicked");
            Problem::internal().into_response()
        }))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<Body>| {
                let request_id = request
                    .extensions()
                    .get::<RequestId>()
                    .map_or("", |id| id.0.as_str());
                tracing::info_span!(
                    "http",
                    method = %request.method(),
                    path = %request.uri().path(),
                    request_id = %request_id,
                )
            }),
        )
        .layer(axum::middleware::from_fn(middleware::request_id))
        .layer(SetSensitiveRequestHeadersLayer::new([
            header::AUTHORIZATION,
            header::COOKIE,
        ])))
}

fn cors(origins: &[String]) -> anyhow::Result<CorsLayer> {
    let origins = origins
        .iter()
        .map(|origin| {
            HeaderValue::from_str(origin).with_context(|| format!("CORS 来源不合法：{origin}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            header::IF_MATCH,
            header::IF_NONE_MATCH,
            HeaderName::from_static("idempotency-key"),
            HeaderName::from_static(middleware::REQUEST_ID_HEADER),
        ])
        .expose_headers([
            header::ETAG,
            header::RETRY_AFTER,
            HeaderName::from_static(middleware::REQUEST_ID_HEADER),
            HeaderName::from_static("ratelimit-limit"),
            HeaderName::from_static("ratelimit-remaining"),
            HeaderName::from_static("ratelimit-reset"),
        ])
        .max_age(Duration::from_secs(600)))
}
