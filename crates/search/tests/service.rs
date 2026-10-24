//! 搜索集成测试：真实 PostgreSQL（每个测试独立临时库）。
//!
//! 数据库兜底的用例总是运行；搜索引擎的用例需要 Meilisearch：设置 `IK_TEST_MEILI_URL`
//! （与可选的 `IK_TEST_MEILI_KEY`），每个测试用独立的索引前缀，结束时删除索引。没有设置时这些用例直接通过。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{Duration, Utc};
use ik_auth::Principal;
use ik_comments::CommentsState;
use ik_content::ContentState;
use ik_kernel::bus::{EventBus, LocalBus};
use ik_kernel::config::{Search as SearchConfig, Secret};
use ik_kernel::{Id, Role, Snowflake};
use ik_search::SearchState;
use ik_search::dto::{CommentSearchQuery, PostSearchQuery, SuggestQuery};
use ik_search::service;
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

fn meili() -> Option<(String, String)> {
    let url = std::env::var("IK_TEST_MEILI_URL").ok()?;
    Some((url, std::env::var("IK_TEST_MEILI_KEY").unwrap_or_default()))
}

fn config(url: Option<String>, key: &str) -> SearchConfig {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let unique = format!(
        "t{}_{}_{}_",
        std::process::id(),
        Utc::now().timestamp_micros(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    SearchConfig {
        url,
        api_key: Secret::new(key),
        index_prefix: unique,
        ..SearchConfig::default()
    }
}

struct Harness {
    search: SearchState,
    db: PgPool,
}

fn state(db: &PgPool, config: &SearchConfig) -> SearchState {
    let bus = Arc::new(LocalBus::new(64));
    let ids = Arc::new(Snowflake::new(1).unwrap());
    let media = ik_media::MediaState::for_tests(db.clone(), ids.clone());
    let content = ContentState::new(db.clone(), ids.clone(), bus.clone(), media.clone());
    let comments = CommentsState::new(db.clone(), ids, bus, media);
    SearchState::new(content, comments, config).unwrap()
}

fn principal(id: i64) -> Principal {
    Principal {
        user_id: Id::new(id),
        session_id: Id::new(id),
        role: Role::User,
        exam_passed: true,
        is_ai_agent: false,
        level: 1,
    }
}

fn posts(q: &str) -> PostSearchQuery {
    PostSearchQuery {
        q: Some(q.to_owned()),
        category: None,
        sort: None,
        cursor: None,
        limit: None,
    }
}

fn comments(q: &str) -> CommentSearchQuery {
    CommentSearchQuery {
        q: Some(q.to_owned()),
        post_id: None,
        cursor: None,
        limit: None,
    }
}

fn suggest(q: &str) -> SuggestQuery {
    SuggestQuery {
        q: Some(q.to_owned()),
        category: None,
        limit: None,
    }
}

struct NewPost<'a> {
    id: i64,
    author: i64,
    title: &'a str,
    body: &'a str,
    category: Option<i64>,
    anonymous: bool,
    published: bool,
    likes: i32,
    /// 信息流时间：现在往前多少分钟。
    age_min: i64,
}

impl Default for NewPost<'_> {
    fn default() -> Self {
        Self {
            id: 0,
            author: 1,
            title: "标题",
            body: "",
            category: None,
            anonymous: false,
            published: true,
            likes: 0,
            age_min: 0,
        }
    }
}

impl Harness {
    async fn new(db: PgPool, config: &SearchConfig) -> Self {
        let h = Self {
            search: state(&db, config),
            db,
        };
        for (id, name) in [(1, "alice"), (2, "bob"), (3, "carol"), (4, "dave")] {
            h.user(id, name).await;
        }
        sqlx::query!(
            "INSERT INTO categories (id, slug, name) VALUES (100, 'guide', '攻略'), (101, 'chat', '闲聊')"
        )
        .execute(&h.db)
        .await
        .unwrap();
        h
    }

    async fn user(&self, id: i64, name: &str) {
        sqlx::query!(
            "INSERT INTO users (id, username, email, level, exam_passed_at)
             VALUES ($1, $2, $3, 1, now())",
            id,
            name,
            format!("{name}@example.com"),
        )
        .execute(&self.db)
        .await
        .unwrap();
    }

    async fn post(&self, p: NewPost<'_>) {
        let at = Utc::now() - Duration::minutes(p.age_min);
        sqlx::query!(
            "INSERT INTO posts (id, author_id, category_id, status, title, body, is_anonymous,
                                anonymous_seed, published_at, first_published_at, bumped_at, like_count)
             VALUES ($1, $2, $3, CASE WHEN $4 THEN 'published' ELSE 'draft' END, $5, $6, $7,
                     CASE WHEN $7 THEN 'seed' END,
                     CASE WHEN $4 THEN $8::timestamptz END, CASE WHEN $4 THEN $8::timestamptz END,
                     CASE WHEN $4 THEN $8::timestamptz END, $9)",
            p.id,
            p.author,
            p.category,
            p.published,
            p.title,
            p.body,
            p.anonymous,
            at,
            p.likes,
        )
        .execute(&self.db)
        .await
        .unwrap();
    }

    async fn comment(
        &self,
        id: i64,
        post: i64,
        parent: Option<i64>,
        author: i64,
        body: &str,
        anonymous: bool,
    ) {
        sqlx::query!(
            "INSERT INTO comments (id, post_id, reply_to_id, reply_to_floor, author_id, floor, body,
                                   is_anonymous, anonymous_seed)
             VALUES ($1, $2, $3, (SELECT floor FROM comments WHERE id = $3), $4, $7, $5, $6,
                     CASE WHEN $6 THEN 'seed' END)",
            id,
            post,
            parent,
            author,
            body,
            anonymous,
            i32::try_from(id).unwrap(),
        )
        .execute(&self.db)
        .await
        .unwrap();
    }

    async fn block(&self, blocker: i64, blocked: i64) {
        sqlx::query!(
            "INSERT INTO user_blocks (blocker_id, blocked_id) VALUES ($1, $2)",
            blocker,
            blocked
        )
        .execute(&self.db)
        .await
        .unwrap();
    }

    async fn post_titles(&self, viewer: Option<&Principal>, query: PostSearchQuery) -> Vec<String> {
        service::search_posts(&self.search, viewer, query)
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|c| c.title)
            .collect()
    }

    async fn comment_bodies(
        &self,
        viewer: Option<&Principal>,
        query: CommentSearchQuery,
    ) -> Vec<String> {
        service::search_comments(&self.search, viewer, query)
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|c| c.body)
            .collect()
    }

    /// 公共数据：
    /// 10 alice「绳网委托攻略」(guide, 5 赞) / 11 bob「委托闲聊」(chat, 9 赞, 正文含「绳网」) /
    /// 12 匿名（作者 carol）「匿名的委托」/ 13 alice 草稿「委托草稿」/ 14 dave「dave 的委托」。
    async fn seed(&self) {
        self.post(NewPost {
            id: 10,
            author: 1,
            title: "绳网委托攻略",
            body: "**新手**必看",
            category: Some(100),
            likes: 5,
            age_min: 30,
            ..Default::default()
        })
        .await;
        self.post(NewPost {
            id: 11,
            author: 2,
            title: "委托闲聊",
            body: "聊聊绳网的事",
            category: Some(101),
            likes: 9,
            age_min: 20,
            ..Default::default()
        })
        .await;
        self.post(NewPost {
            id: 12,
            author: 3,
            title: "匿名的委托",
            body: "匿名写的",
            anonymous: true,
            age_min: 10,
            ..Default::default()
        })
        .await;
        self.post(NewPost {
            id: 13,
            author: 1,
            title: "委托草稿",
            published: false,
            ..Default::default()
        })
        .await;
        self.post(NewPost {
            id: 14,
            author: 4,
            title: "dave 的委托",
            age_min: 5,
            ..Default::default()
        })
        .await;
    }
}

// ---------- 数据库兜底（没有搜索引擎）----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn fallback_matches_title_and_body_with_visibility_and_blocks(db: PgPool) {
    let h = Harness::new(db, &config(None, "")).await;
    h.seed().await;

    // 「最新」：按信息流时间倒序；草稿不出现。
    let mut latest = posts("委托");
    latest.sort = Some("latest".into());
    assert_eq!(
        h.post_titles(None, latest).await,
        ["dave 的委托", "匿名的委托", "委托闲聊", "绳网委托攻略"]
    );
    // 正文也能搜到；忽略大小写。
    assert_eq!(
        h.post_titles(None, posts("绳网")).await,
        ["委托闲聊", "绳网委托攻略"]
    );
    assert_eq!(h.post_titles(None, posts("DAVE")).await, ["dave 的委托"]);
    // 分区。
    let mut guide = posts("委托");
    guide.category = Some("guide".into());
    assert_eq!(h.post_titles(None, guide).await, ["绳网委托攻略"]);
    // 热门：点赞倒序。
    let mut hot = posts("委托");
    hot.sort = Some("hot".into());
    assert_eq!(
        h.post_titles(None, hot).await[..2],
        ["委托闲聊", "绳网委托攻略"]
    );

    // 拉黑：bob 拉黑了 dave → bob 看不到 dave 的帖子；匿名帖照常出现（作者 carol 被拉黑也一样）。
    h.block(2, 4).await;
    h.block(3, 2).await;
    let bob = principal(2);
    let seen = h.post_titles(Some(&bob), posts("委托")).await;
    assert!(!seen.contains(&"dave 的委托".to_owned()));
    assert!(
        seen.contains(&"匿名的委托".to_owned()),
        "匿名内容不按拉黑过滤"
    );

    // 单字查询不走数据库兜底（SRCH-014）；空查询返回空。
    assert!(h.post_titles(None, posts("委")).await.is_empty());
    assert!(h.post_titles(None, posts("   ")).await.is_empty());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn fallback_pagination_is_offset_based_and_total_is_stable(db: PgPool) {
    let h = Harness::new(db, &config(None, "")).await;
    for i in 0..7 {
        let title = format!("委托 {i}");
        h.post(NewPost {
            id: 100 + i,
            title: &title,
            age_min: i,
            ..Default::default()
        })
        .await;
    }
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = service::search_posts(
            &h.search,
            None,
            PostSearchQuery {
                cursor,
                limit: Some(3),
                ..posts("委托")
            },
        )
        .await
        .unwrap();
        assert_eq!(page.total, 7, "total 与翻页位置无关（EDGE-008）");
        seen.extend(page.items.into_iter().map(|c| c.title));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    let expected: Vec<String> = (0..7).map(|i| format!("委托 {i}")).collect();
    assert_eq!(seen, expected);
    let err = service::search_posts(
        &h.search,
        None,
        PostSearchQuery {
            cursor: Some("garbage".into()),
            ..posts("委托")
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "CURSOR_INVALID");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn fallback_suggest_matches_titles_and_escapes(db: PgPool) {
    let h = Harness::new(db, &config(None, "")).await;
    h.post(NewPost {
        id: 10,
        title: "<b>委托</b> 指南",
        ..Default::default()
    })
    .await;
    h.post(NewPost {
        id: 11,
        title: "无关",
        body: "正文里有委托",
        ..Default::default()
    })
    .await;
    let items = service::suggest(&h.search, None, suggest("委托"))
        .await
        .unwrap();
    assert_eq!(items.len(), 1, "联想只匹配标题");
    assert_eq!(items[0].title_highlighted, "&lt;b&gt;委托&lt;/b&gt; 指南");
    assert_eq!(items[0].excerpt, "");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn fallback_comment_search_applies_visibility(db: PgPool) {
    let h = Harness::new(db, &config(None, "")).await;
    h.seed().await;
    h.comment(1, 10, None, 2, "绳网好评", false).await;
    h.comment(2, 10, Some(1), 4, "绳网回复", false).await;
    h.comment(3, 13, None, 2, "草稿下的绳网", false).await; // 帖子未发布
    h.comment(4, 11, None, 3, "匿名的绳网评论", true).await;
    h.comment(5, 11, None, 4, "被隐藏的绳网评论", false).await;
    // 回复被隐藏的评论：回复本身照常可见（扁平楼层，ADR 0006）。
    h.comment(6, 11, Some(5), 1, "回复隐藏评论的绳网回复", false)
        .await;
    sqlx::query!("UPDATE comments SET hidden_at = now() WHERE id = 5")
        .execute(&h.db)
        .await
        .unwrap();

    let mut all = h.comment_bodies(None, comments("绳网")).await;
    all.sort();
    assert_eq!(
        all,
        [
            "匿名的绳网评论",
            "回复隐藏评论的绳网回复",
            "绳网回复",
            "绳网好评"
        ]
    );

    // alice 拉黑 dave：dave 的评论消失；carol 的匿名评论不受影响。
    h.block(1, 4).await;
    h.block(1, 3).await;
    let mut alice = h
        .comment_bodies(Some(&principal(1)), comments("绳网"))
        .await;
    alice.sort();
    assert_eq!(
        alice,
        ["匿名的绳网评论", "回复隐藏评论的绳网回复", "绳网好评"]
    );

    // 限定帖子。
    let mut in_post = comments("绳网");
    in_post.post_id = Some(Id::new(11));
    assert_eq!(
        h.comment_bodies(None, in_post).await,
        ["回复隐藏评论的绳网回复", "匿名的绳网评论"]
    );

    // 匿名评论对所有人都显示匿名名。
    let page = service::search_comments(&h.search, Some(&principal(3)), comments("匿名的绳网"))
        .await
        .unwrap();
    assert!(page.items[0].author.anonymous);
    assert!(page.items[0].author.id.is_none());
}

// ---------- 搜索引擎 ----------

async fn engine_harness(db: PgPool) -> Option<Harness> {
    let Some((url, key)) = meili() else {
        eprintln!("未设置 IK_TEST_MEILI_URL，跳过搜索引擎用例");
        return None;
    };
    Some(Harness::new(db, &config(Some(url), &key)).await)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn engine_results_are_rechecked_and_private(db: PgPool) {
    let Some(h) = engine_harness(db).await else {
        return;
    };
    h.seed().await;
    h.search.reindex().await.unwrap();

    let titles = h.post_titles(None, posts("委托")).await;
    assert_eq!(titles.len(), 4, "草稿不进索引：{titles:?}");
    // 按作者名能搜到实名帖，搜不到匿名帖（PRIV-005）。
    assert_eq!(h.post_titles(None, posts("alice")).await, ["绳网委托攻略"]);
    assert!(
        h.post_titles(None, posts("carol")).await.is_empty(),
        "匿名帖的作者名不进索引"
    );
    // 单字查询走引擎。
    assert!(!h.post_titles(None, posts("委")).await.is_empty());

    // 拉黑在引擎里过滤。
    h.block(1, 2).await;
    let seen = h.post_titles(Some(&principal(1)), posts("委托")).await;
    assert!(!seen.contains(&"委托闲聊".to_owned()));

    // 索引残留：帖子下线但索引还没同步 → 回查过滤掉，整页为空时改走数据库。
    sqlx::query!("UPDATE posts SET status = 'draft' WHERE id = 10")
        .execute(&h.db)
        .await
        .unwrap();
    assert!(h.post_titles(None, posts("攻略")).await.is_empty());
    let mut guide = posts("新手");
    guide.category = Some("guide".into());
    assert!(h.post_titles(None, guide).await.is_empty());
    // 同步后文档被删除；引擎没有命中时直接返回空（不查数据库）。
    h.search.sync_now(&[10], &[]).await.unwrap();
    let page = service::search_posts(&h.search, None, posts("攻略"))
        .await
        .unwrap();
    assert_eq!(page.total, 0);

    h.search.drop_indexes().await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_hits_fall_back_to_the_database(db: PgPool) {
    let Some(h) = engine_harness(db).await else {
        return;
    };
    h.seed().await;
    h.search.reindex().await.unwrap();
    // 绕过同步直接改库：索引里还是旧标题。
    sqlx::query!("UPDATE posts SET status = 'draft' WHERE id IN (10, 11)")
        .execute(&h.db)
        .await
        .unwrap();
    h.post(NewPost {
        id: 20,
        title: "绳网新帖",
        ..Default::default()
    })
    .await;
    // 引擎命中 10、11（都已下线）→ 回查为空 → 数据库兜底找到 20。
    assert_eq!(h.post_titles(None, posts("绳网")).await, ["绳网新帖"]);
    h.search.drop_indexes().await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn reindex_removes_orphans_and_rename_updates_author_names(db: PgPool) {
    let Some(h) = engine_harness(db).await else {
        return;
    };
    h.seed().await;
    h.comment(1, 10, None, 2, "绳网好评", false).await;
    h.comment(2, 10, Some(1), 4, "绳网回复", false).await;
    h.search.reindex().await.unwrap();
    assert_eq!(h.comment_bodies(None, comments("绳网")).await.len(), 2);

    // 删除帖子（级联删除评论）后全量对账：残留被清掉，引擎无命中 → 空。
    sqlx::query!("DELETE FROM posts WHERE id = 10")
        .execute(&h.db)
        .await
        .unwrap();
    h.search.reindex().await.unwrap();
    let page = service::search_comments(&h.search, None, comments("绳网"))
        .await
        .unwrap();
    assert_eq!(page.total, 0);
    let page = service::search_posts(&h.search, None, posts("攻略"))
        .await
        .unwrap();
    assert_eq!(page.total, 0);

    // 改名：旧名搜不到，新名搜得到（D-039）。
    sqlx::query!("UPDATE users SET username = 'bobby' WHERE id = 2")
        .execute(&h.db)
        .await
        .unwrap();
    h.search.rename_now(&[2]).await.unwrap();
    assert_eq!(h.post_titles(None, posts("bobby")).await, ["委托闲聊"]);

    h.search.drop_indexes().await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn engine_comment_search_and_removal(db: PgPool) {
    let Some(h) = engine_harness(db).await else {
        return;
    };
    h.seed().await;
    h.comment(1, 11, None, 2, "绳网好评 @[alice](1)", false)
        .await;
    h.comment(2, 11, Some(1), 3, "匿名回复绳网", true).await;
    h.search.reindex().await.unwrap();
    let mut bodies = h.comment_bodies(None, comments("绳网")).await;
    bodies.sort();
    assert_eq!(bodies, ["匿名回复绳网", "绳网好评 @[alice](1)"]);
    assert!(
        h.comment_bodies(None, comments("carol")).await.is_empty(),
        "匿名评论的作者名不进索引"
    );

    // 删除被回复的评论：只从索引删它自己，回复它的评论保留（扁平楼层，ADR 0006）。
    sqlx::query!("DELETE FROM comments WHERE id = 1")
        .execute(&h.db)
        .await
        .unwrap();
    h.search.remove_comments_now(&[1]).await.unwrap();
    assert_eq!(
        h.comment_bodies(None, comments("绳网")).await,
        ["匿名回复绳网"]
    );
    h.search.drop_indexes().await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn engine_suggest_highlights_safely(db: PgPool) {
    let Some(h) = engine_harness(db).await else {
        return;
    };
    h.post(NewPost {
        id: 10,
        title: "<i>绳网</i>委托指南",
        body: "很长的正文，讲绳网委托怎么接",
        ..Default::default()
    })
    .await;
    h.search.reindex().await.unwrap();
    let items = service::suggest(&h.search, None, suggest("绳网"))
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert!(
        items[0].title_highlighted.contains("<mark>绳网</mark>"),
        "{}",
        items[0].title_highlighted
    );
    assert!(
        items[0].title_highlighted.contains("&lt;i&gt;"),
        "标题里的 HTML 被转义"
    );
    assert!(!items[0].excerpt.is_empty());
    h.search.drop_indexes().await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unreachable_engine_falls_back(db: PgPool) {
    let mut cfg = config(Some("http://127.0.0.1:9".into()), "");
    cfg.timeout_ms = 500;
    let h = Harness::new(db, &cfg).await;
    h.seed().await;
    assert_eq!(h.post_titles(None, posts("攻略")).await, ["绳网委托攻略"]);
}

/// 后台同步循环：启动时全量对账；事件触发的同步；没有事件时由增量扫描补上。
#[sqlx::test(migrator = "MIGRATOR")]
async fn background_indexer_follows_events_and_sweeps(db: PgPool) {
    let Some((url, key)) = meili() else { return };
    let mut cfg = config(Some(url), &key);
    cfg.sweep_secs = 1;
    let h = Harness::new(db, &cfg).await;
    h.post(NewPost {
        id: 10,
        title: "启动前的委托",
        ..Default::default()
    })
    .await;
    let bus = LocalBus::new(64);
    h.search.start_indexer(&bus, &cfg).unwrap();

    async fn eventually(h: &Harness, q: &str, want: usize) {
        for _ in 0..100 {
            let page = service::search_posts(&h.search, None, posts(q))
                .await
                .unwrap();
            if page.total == u64::try_from(want).unwrap() && page.items.len() == want {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("等不到「{q}」的 {want} 条结果");
    }
    // 引擎在启动对账完成之前没有命中，查询会走数据库兜底，也能找到；这里等的是索引本身。
    eventually(&h, "启动前", 1).await;

    // 事件：发布 → 很快可搜；下线事件 → 文档删除。
    h.post(NewPost {
        id: 11,
        title: "事件触发的委托",
        ..Default::default()
    })
    .await;
    bus.publish(
        ik_kernel::bus::Event::new(
            "post.published",
            ik_kernel::bus::Audience::Broadcast,
            &serde_json::json!({ "postId": "11" }),
        )
        .unwrap(),
    );
    eventually(&h, "事件触发", 1).await;

    // 没有事件：靠增量扫描（updated_at）。
    h.post(NewPost {
        id: 12,
        title: "扫描发现的委托",
        ..Default::default()
    })
    .await;
    eventually(&h, "扫描发现", 1).await;

    sqlx::query!("DELETE FROM posts WHERE id = 11")
        .execute(&h.db)
        .await
        .unwrap();
    bus.publish(
        ik_kernel::bus::Event::new(
            "post.removed",
            ik_kernel::bus::Audience::Broadcast,
            &serde_json::json!({ "postId": "11" }),
        )
        .unwrap(),
    );
    eventually(&h, "事件触发", 0).await;
    h.search.drop_indexes().await.unwrap();
}
