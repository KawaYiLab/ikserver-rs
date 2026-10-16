//! 管理后台页面：`admin/` 构建出的静态文件编进可执行文件，挂在 `/admin`（ADR 0002）。
//!
//! - 发布构建把 `admin/.output/public` 整个嵌进二进制，部署只需要这一个文件；调试构建运行时从磁盘读
//!   同一个目录，后台重新 `pnpm build` 后刷新页面即可。
//! - 目录不存在也能编译（CI 的 lint / test 不构建后台），这时 `/admin` 下一律 404，启动时给出警告。
//! - 预渲染的路由是 `<路由>/index.html`，其余页面路径回退到 `200.html`，交给前端路由。
//! - `_nuxt/` 下带哈希的资源长期缓存；入口页与 `_nuxt/builds/`（前端检查新版本用的清单）每次协商。

use std::borrow::Cow;

use axum::Router;
use axum::body::Body;
use axum::extract::Path;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use ik_kernel::Problem;
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../admin/.output/public"]
#[allow_missing = true]
struct Assets;

/// 这个版本是否带了管理后台（发布前没先构建后台时为 `false`）。
pub fn is_bundled() -> bool {
    Assets::get(INDEX).is_some()
}

const INDEX: &str = "index.html";
const SPA_FALLBACK: &str = "200.html";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const REVALIDATE: &str = "no-cache";

pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/admin", get(|| async { Redirect::permanent("/admin/") }))
        .route("/admin/", get(|headers: HeaderMap| async move { serve("", &headers) }))
        .route(
            "/admin/{*path}",
            get(|Path(path): Path<String>, headers: HeaderMap| async move { serve(&path, &headers) }),
        )
}

fn serve(path: &str, headers: &HeaderMap) -> Response {
    let Some(name) = resolve(path, |name| Assets::get(name).is_some()) else {
        return Problem::not_found().into_response();
    };
    let Some(file) = Assets::get(&name) else {
        return Problem::not_found().into_response();
    };

    let etag = etag(&file.metadata.sha256_hash());
    let cache = if is_immutable(&name) {
        IMMUTABLE
    } else {
        REVALIDATE
    };
    let mut response = if if_none_match(headers, &etag) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        // 发布构建里是静态数据，直接借用，不复制。
        let body = match file.data {
            Cow::Borrowed(bytes) => Body::from(bytes),
            Cow::Owned(bytes) => Body::from(bytes),
        };
        let mut response = body.into_response();
        if let Ok(mime) = HeaderValue::from_str(file.metadata.mimetype()) {
            response.headers_mut().insert(header::CONTENT_TYPE, mime);
        }
        response
    };

    let out = response.headers_mut();
    if let Ok(etag) = HeaderValue::from_str(&etag) {
        out.insert(header::ETAG, etag);
    }
    out.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    out.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    out.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    out.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    response
}

/// 把 `/admin/` 之后的路径对应到要返回的文件；`None` 表示 404。
fn resolve(path: &str, exists: impl Fn(&str) -> bool) -> Option<String> {
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        return Some(INDEX.to_owned());
    }
    if path
        .split('/')
        .any(|segment| matches!(segment, "" | "." | "..") || segment.contains('\\'))
    {
        return None;
    }
    if exists(path) {
        return Some(path.to_owned());
    }
    let page = format!("{path}/{INDEX}");
    if exists(&page) {
        return Some(page);
    }
    // 缺失的脚本、样式、图片不能拿页面顶替，否则浏览器会把 HTML 当脚本执行，报错也更难查。
    let last = path.rsplit('/').next().unwrap_or(path);
    if path.starts_with("_nuxt/") || last.contains('.') {
        return None;
    }
    Some(SPA_FALLBACK.to_owned())
}

fn is_immutable(name: &str) -> bool {
    name.starts_with("_nuxt/") && !name.starts_with("_nuxt/builds/")
}

fn etag(hash: &[u8; 32]) -> String {
    let hex: String = hash[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("\"{hex}\"")
}

fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .map(|tag| tag.trim().trim_start_matches("W/"))
                .any(|tag| tag == etag || tag == "*")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILES: &[&str] = &[
        "index.html",
        "200.html",
        "reports/index.html",
        "_nuxt/entry.abc.css",
        "_nuxt/builds/latest.json",
    ];

    fn resolve_in(path: &str) -> Option<String> {
        resolve(path, |name| FILES.contains(&name))
    }

    #[test]
    fn pages_resolve_to_prerendered_html_or_spa_fallback() {
        assert_eq!(resolve_in("").as_deref(), Some("index.html"));
        assert_eq!(resolve_in("reports").as_deref(), Some("reports/index.html"));
        assert_eq!(
            resolve_in("reports/").as_deref(),
            Some("reports/index.html")
        );
        assert_eq!(resolve_in("users/42").as_deref(), Some("200.html"));
        assert_eq!(
            resolve_in("_nuxt/entry.abc.css").as_deref(),
            Some("_nuxt/entry.abc.css")
        );
    }

    #[test]
    fn missing_assets_and_odd_paths_are_not_found() {
        assert_eq!(resolve_in("_nuxt/gone.js"), None);
        assert_eq!(resolve_in("_nuxt/chunk"), None);
        assert_eq!(resolve_in("favicon.ico"), None);
        assert_eq!(resolve_in("../Cargo.toml"), None);
        assert_eq!(resolve_in("reports/../../x"), None);
        assert_eq!(resolve_in("a//b"), None);
        assert_eq!(resolve_in("a\\..\\b"), None);
    }

    #[test]
    fn only_hashed_assets_are_cached_forever() {
        assert!(is_immutable("_nuxt/entry.abc.css"));
        assert!(!is_immutable("_nuxt/builds/latest.json"));
        assert!(!is_immutable("index.html"));
        assert!(!is_immutable("200.html"));
    }

    #[test]
    fn conditional_requests_match_strong_and_weak_tags() {
        let tag = etag(&[0xab; 32]);
        let mut headers = HeaderMap::new();
        assert!(!if_none_match(&headers, &tag));
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_str(&format!("\"x\", W/{tag}")).unwrap(),
        );
        assert!(if_none_match(&headers, &tag));
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"x\""));
        assert!(!if_none_match(&headers, &tag));
    }
}
