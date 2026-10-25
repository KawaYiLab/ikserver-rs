use std::time::Duration;

use anyhow::Context;
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Request};
use axum::response::IntoResponse;
use http::{HeaderName, HeaderValue, Method, StatusCode, header};
use ik_kernel::Problem;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::compression::{CompressionLayer, CompressionLevel};
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
    components(schemas(
        ik_kernel::error::ProblemBody,
        ik_kernel::error::FieldError,
        ik_realtime::protocol::ClientFrame,
        ik_realtime::protocol::ServerFrame,
    )),
    modifiers(&SecuritySchemes),
    tags(
        (name = "system", description = "运维探针"),
        (name = "auth", description = "验证码、注册、登录、重置密码、登出"),
        (name = "me", description = "当前用户与登录设备"),
        (name = "content", description = "分区、标签、帖子与信息流"),
        (name = "comments", description = "评论、回复、置顶与采纳"),
        (name = "economy", description = "经验、等级、丁尼、签到与等级权益"),
        (name = "exam", description = "入站考试：开考、交卷、回顾"),
        (name = "social", description = "关注与拉黑"),
        (name = "notify", description = "通知（敲敲）与已读"),
        (name = "messaging", description = "私信：会话、消息、编辑与撤回"),
        (name = "inbox", description = "统一收件箱：私信会话与通知分组混排、已读"),
        (name = "realtime", description = "实时通道（WebSocket）"),
        (name = "profile", description = "个人资料、作者主页与用户搜索"),
        (name = "feed", description = "首页、热门榜、关注流、未看流与阅读记录"),
        (name = "search", description = "帖子与评论搜索、标题联想"),
        (name = "media", description = "直传上传与我的图库"),
        (name = "cosmetics", description = "装扮：系统头像与论坛名片"),
        (name = "moderation", description = "举报，以及审核（版主、管理员）"),
        (name = "ai", description = "AI 助手：角色、模型、额度"),
        (name = "story", description = "绝区零剧情库（MCP）"),
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
        .merge(ik_comments::router())
        .merge(ik_economy::router())
        .merge(ik_exam::router())
        .merge(ik_social::router())
        .merge(ik_notify::router())
        .merge(ik_messaging::router())
        .merge(ik_realtime::router())
        .merge(ik_profile::router())
        .merge(ik_feed::router())
        .merge(ik_search::router())
        .merge(ik_media::router())
        .merge(ik_cosmetics::router())
        .merge(ik_mihoyo::router())
        .merge(ik_account::router())
        .merge(ik_moderation::router())
        .merge(ik_ai::router())
        .merge(ik_admin::router())
        .merge(ik_story::router())
        .merge(ik_qqbot::router())
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
    router = router.merge(crate::admin_ui::router());

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
    let rate_limit = axum::middleware::from_fn_with_state(state.clone(), crate::rate_limit::layer);
    Ok(router
        .fallback(|| async { Problem::not_found() })
        // 幂等层在超时层之内：超时只丢弃等待，处理任务照样完成并记下结果。
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::idempotency::layer,
        ))
        .with_state(state)
        .layer(axum::middleware::from_fn(middleware::timeout(
            Duration::from_secs(config.http.request_timeout_secs),
        )))
        .layer(DefaultBodyLimit::max(config.http.body_limit_bytes))
        // 动态响应用中低档：brotli 默认是最高档 11，每个响应多花几十毫秒 CPU；4 档体积只大一点、快一个数量级。
        .layer(CompressionLayer::new().quality(CompressionLevel::Precise(4)))
        .layer(csrf)
        // 在 CORS 之内：429 也带 CORS 头，浏览器才读得到 Retry-After。
        .layer(rate_limit)
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
            header::LOCATION,
            HeaderName::from_static(crate::idempotency::REPLAYED_HEADER),
            header::RETRY_AFTER,
            HeaderName::from_static(middleware::REQUEST_ID_HEADER),
            HeaderName::from_static("ratelimit-limit"),
            HeaderName::from_static("ratelimit-remaining"),
            HeaderName::from_static("ratelimit-reset"),
        ])
        .max_age(Duration::from_secs(600)))
}
