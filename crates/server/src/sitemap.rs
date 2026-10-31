//! `GET /v1/sitemap.xml`：给搜索引擎的站点地图。主站（Cloudflare Pages）的 `/sitemap.xml`
//! 由边缘函数代理到这里。地址用 `IK_SITE_URL` 拼，没有配置时 404。
//! 收录帖子、作者主页和至少有 [`MIN_TAG_POSTS`] 篇公开帖子的标签页。

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, SecondsFormat, Utc};
use http::{HeaderValue, header};
use ik_kernel::Problem;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AppState;

/// 单个 sitemap 文件最多 50000 条（sitemaps.org 协议上限），首页占一条。
const MAX_URLS: i64 = 49_999;
/// 标签至少有这么多篇公开帖子才收录，太少的标签页内容单薄。
const MIN_TAG_POSTS: i64 = 2;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(sitemap))
}

/// 站点地图
///
/// 公开帖子（已发布、未隐藏）、有公开帖子的作者主页（未隐藏、未注销）和有足够公开帖子的标签（未屏蔽）。
/// 各自按最近更新倒序。
#[utoipa::path(
    get,
    path = "/v1/sitemap.xml",
    tag = "system",
    responses(
        (status = 200, description = "sitemaps.org 格式", body = String, content_type = "application/xml"),
        (status = 404, description = "没有配置 IK_SITE_URL"),
    )
)]
async fn sitemap(State(state): State<AppState>) -> Result<Response, Problem> {
    let site = state.config.site_url.trim_end_matches('/');
    if site.is_empty() {
        return Err(Problem::not_found());
    }
    let mut conn = state.db.acquire().await?;

    let posts: Vec<(i64, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, GREATEST(COALESCE(edited_at, published_at), bumped_at) AS lastmod
         FROM posts
         WHERE status = 'published' AND hidden_at IS NULL AND published_at IS NOT NULL
         ORDER BY lastmod DESC
         LIMIT $1",
    )
    .bind(MAX_URLS)
    .fetch_all(&mut *conn)
    .await?;

    let remaining = MAX_URLS - posts.len() as i64;
    let authors: Vec<(i64, DateTime<Utc>)> = if remaining > 0 {
        sqlx::query_as(
            "SELECT p.author_id, MAX(COALESCE(p.edited_at, p.published_at)) AS lastmod
             FROM posts p
             JOIN users u ON u.id = p.author_id
             LEFT JOIN user_profiles up ON up.user_id = p.author_id
             WHERE p.status = 'published' AND p.hidden_at IS NULL AND NOT p.is_anonymous
               AND u.status NOT IN ('deleted', 'merged')
               AND NOT COALESCE(up.hidden, false)
             GROUP BY p.author_id
             ORDER BY lastmod DESC
             LIMIT $1",
        )
        .bind(remaining)
        .fetch_all(&mut *conn)
        .await?
    } else {
        Vec::new()
    };
    let remaining = remaining - authors.len() as i64;
    let tags: Vec<(String, DateTime<Utc>)> = if remaining > 0 {
        sqlx::query_as(
            "SELECT t.slug, MAX(COALESCE(p.edited_at, p.published_at)) AS lastmod
             FROM tags t
             JOIN post_tags pt ON pt.tag_id = t.id
             JOIN posts p ON p.id = pt.post_id
             WHERE NOT t.is_blocked AND p.status = 'published' AND p.hidden_at IS NULL
             GROUP BY t.id, t.slug
             HAVING COUNT(*) >= $2
             ORDER BY lastmod DESC
             LIMIT $1",
        )
        .bind(remaining)
        .bind(MIN_TAG_POSTS)
        .fetch_all(&mut *conn)
        .await?
    } else {
        Vec::new()
    };
    drop(conn);

    let mut xml = String::with_capacity(128 + (posts.len() + authors.len() + tags.len()) * 120);
    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push_str(r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#);
    push_url(&mut xml, &format!("{site}/"), posts.first().map(|p| p.1));
    for (id, lastmod) in &posts {
        push_url(&mut xml, &format!("{site}/post/{id}"), Some(*lastmod));
    }
    for (id, lastmod) in &authors {
        push_url(&mut xml, &format!("{site}/profile/{id}"), Some(*lastmod));
    }
    for (slug, lastmod) in &tags {
        push_url(
            &mut xml,
            &format!("{site}/tags/{}", encode_component(slug)),
            Some(*lastmod),
        );
    }
    xml.push_str("</urlset>");

    let mut res = xml.into_response();
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    Ok(res)
}

/// 地址只含站点地址和数字 id；站点地址来自配置，仍按 XML 转义。
fn push_url(xml: &mut String, loc: &str, lastmod: Option<DateTime<Utc>>) {
    xml.push_str("<url><loc>");
    xml.push_str(&escape(loc));
    xml.push_str("</loc>");
    if let Some(t) = lastmod {
        xml.push_str("<lastmod>");
        xml.push_str(&t.to_rfc3339_opts(SecondsFormat::Secs, true));
        xml.push_str("</lastmod>");
    }
    xml.push_str("</url>");
}

/// 与浏览器 `encodeURIComponent` 一致：字母数字和 `-_.!~*'()` 原样，其余按 UTF-8 百分号编码。
/// 标签 slug 可能含中文、`+`、`#`。
fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_slug_is_percent_encoded() {
        assert_eq!(encode_component("攻略"), "%E6%94%BB%E7%95%A5");
        assert_eq!(encode_component("c#"), "c%23");
        assert_eq!(encode_component("c++"), "c%2B%2B");
        assert_eq!(encode_component("a-b_c"), "a-b_c");
    }

    #[test]
    fn url_entry_is_escaped_and_utc() {
        let mut xml = String::new();
        let t = DateTime::parse_from_rfc3339("2026-10-30T08:00:00+08:00")
            .unwrap()
            .with_timezone(&Utc);
        push_url(&mut xml, "https://a.b/?x=1&y=2", Some(t));
        assert_eq!(
            xml,
            "<url><loc>https://a.b/?x=1&amp;y=2</loc><lastmod>2026-10-30T00:00:00Z</lastmod></url>"
        );
    }
}
