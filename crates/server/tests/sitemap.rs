//! `GET /v1/sitemap.xml` 只列出公开帖子和公开主页。

#![allow(clippy::unwrap_used)]

use axum::body::Body;
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use ik_kernel::config::{Config, Secret};
use ik_server::{AppState, build_app};
use sqlx::PgPool;
use tower::ServiceExt;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

async fn get(db: &PgPool, site_url: &str) -> (StatusCode, Option<String>, String) {
    let mut config = Config::default();
    config.database.url = Secret::new("postgres://unused");
    config.hot.enabled = false;
    config.site_url = site_url.to_owned();
    let app = build_app(AppState::new(config, db.clone()).unwrap()).unwrap();
    let res = app
        .oneshot(Request::get("/v1/sitemap.xml").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_owned());
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, ct, String::from_utf8(body.to_vec()).unwrap())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn lists_public_posts_and_profiles(db: PgPool) {
    sqlx::query(
        "INSERT INTO users (id, username, email, status) VALUES
           (10, 'alice', 'a@example.com', 'active'),
           (11, 'hidden', 'h@example.com', 'active'),
           (12, 'gone', 'g@example.com', 'deleted'),
           (13, 'anon', 'n@example.com', 'active')",
    )
    .execute(&db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_profiles (user_id, hidden) VALUES (11, true)")
        .execute(&db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO posts (id, author_id, title, status, is_anonymous, anonymous_seed, published_at, first_published_at, bumped_at, hidden_at) VALUES
           (100, 10, '公开', 'published', false, NULL, now(), now(), now(), NULL),
           (101, 10, '被隐藏', 'published', false, NULL, now(), now(), now(), now()),
           (102, 10, '草稿', 'draft', false, NULL, NULL, NULL, NULL, NULL),
           (103, 11, '隐藏主页的人发的', 'published', false, NULL, now(), now(), now(), NULL),
           (104, 12, '注销的人发的', 'published', false, NULL, now(), now(), now(), NULL),
           (105, 13, '匿名', 'published', true, 's', now(), now(), now(), NULL)",
    )
    .execute(&db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO tags (id, name, slug, is_blocked) VALUES
           (1, '攻略', '攻略', false), (2, '单篇', 'single', false), (3, '屏蔽', 'blocked', true)",
    )
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO post_tags (post_id, tag_id, position) VALUES
           (100, 1, 0), (103, 1, 0), (101, 2, 0), (104, 2, 0), (100, 3, 1), (105, 3, 0)",
    )
    .execute(&db)
    .await
    .unwrap();

    let (status, ct, xml) = get(&db, "https://interk.net/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct.as_deref(), Some("application/xml; charset=utf-8"));
    assert!(xml.contains("<loc>https://interk.net/</loc>"));
    for id in [100, 103, 104, 105] {
        assert!(
            xml.contains(&format!("<loc>https://interk.net/post/{id}</loc>")),
            "post {id}"
        );
    }
    for id in [101, 102] {
        assert!(!xml.contains(&format!("/post/{id}<")), "post {id}");
    }
    assert!(xml.contains("<loc>https://interk.net/profile/10</loc>"));
    // 「single」只有一篇公开帖（101 被隐藏），「blocked」被屏蔽。
    assert!(xml.contains("<loc>https://interk.net/tags/%E6%94%BB%E7%95%A5</loc>"));
    assert!(!xml.contains("/tags/single<"));
    assert!(!xml.contains("/tags/blocked<"));
    for id in [11, 12, 13] {
        assert!(!xml.contains(&format!("/profile/{id}<")), "profile {id}");
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn not_found_without_site_url(db: PgPool) {
    let (status, _, _) = get(&db, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
