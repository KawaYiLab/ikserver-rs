//! `GET /v1/sitemap.xml`：给搜索引擎的站点地图。主站（Cloudflare Pages）的 `/sitemap.xml`
//! 由边缘函数代理到这里。地址用 `IK_SITE_URL` 拼，没有配置时 404。

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

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(sitemap))
}

/// 站点地图
///
/// 公开帖子（已发布、未隐藏）和有公开帖子的作者主页（未隐藏、未注销）。按最近更新倒序。
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
    drop(conn);

    let mut xml = String::with_capacity(128 + (posts.len() + authors.len()) * 120);
    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push_str(r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#);
    push_url(&mut xml, &format!("{site}/"), posts.first().map(|p| p.1));
    for (id, lastmod) in &posts {
        push_url(&mut xml, &format!("{site}/post/{id}"), Some(*lastmod));
    }
    for (id, lastmod) in &authors {
        push_url(&mut xml, &format!("{site}/profile/{id}"), Some(*lastmod));
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
