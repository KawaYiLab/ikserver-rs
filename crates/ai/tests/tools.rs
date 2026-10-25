//! 论坛工具：只读公开内容、拉黑与匿名口径（D-084 方案 A）、精确找人（D-085 方案 A）、固定文案。
//! 搜索不连搜索引擎，走数据库兜底。

// clippy.toml 的测试豁免只覆盖 #[test] 函数本身，不覆盖集成测试里的辅助函数。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use chrono::Utc;
use ik_ai::AiState;
use ik_ai::tools::{self, ToolContext, ToolMeta};
use ik_auth::Principal;
use ik_comments::dto::CreateCommentRequest;
use ik_content::dto::PostInput;
use ik_kernel::bus::{Effects, LocalBus};
use ik_kernel::config::Ai;
use ik_kernel::{Id, Role, Snowflake};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

const ALICE: i64 = 101;
const BOB: i64 = 102;
const CAROL: i64 = 103;
const DAVE: i64 = 104;

fn principal(id: i64) -> Principal {
    Principal {
        user_id: Id::new(id),
        session_id: Id::new(id),
        role: Role::User,
        exam_passed: true,
        is_ai_agent: false,
        level: 3,
    }
}

struct World {
    db: PgPool,
    state: AiState,
    guide: i64,
    anonymous: i64,
    carols: i64,
    hidden: i64,
}

async fn post(
    state: &AiState,
    author: i64,
    title: &str,
    body: &str,
    category: Option<Id<ik_kernel::id::marker::Category>>,
    anonymous: bool,
) -> i64 {
    ik_content::service::create(
        &state.forum.content,
        &principal(author),
        PostInput {
            title: title.into(),
            body: body.into(),
            category_id: category,
            tags: Vec::new(),
            is_anonymous: anonymous,
            external_videos: Vec::new(),
            editor_state: None,
            cover_ids: Vec::new(),
        },
        true,
    )
    .await
    .unwrap()
    .id
    .get()
}

async fn comment(
    state: &AiState,
    who: i64,
    post: i64,
    parent: Option<i64>,
    body: &str,
    anonymous: bool,
) -> i64 {
    ik_comments::service::create(
        &state.forum.comments,
        &principal(who),
        post,
        CreateCommentRequest {
            body: body.into(),
            image_ids: Vec::new(),
            reply_to_id: parent.map(Id::new),
            is_anonymous: anonymous,
        },
    )
    .await
    .unwrap()
    .id
    .get()
}

async fn world(db: PgPool) -> World {
    let ids = Arc::new(Snowflake::new(1).unwrap());
    let state =
        AiState::for_tests(db.clone(), ids, Arc::new(LocalBus::new(64)), Ai::default()).unwrap();
    for (id, name) in [
        (ALICE, "alice"),
        (BOB, "bob"),
        (CAROL, "carol"),
        (DAVE, "dave"),
    ] {
        sqlx::query!(
            "INSERT INTO users (id, username, email, role, level, exam_passed_at)
             VALUES ($1, $2, $3, 'user', 3, now())",
            id,
            name,
            format!("{name}@example.com"),
        )
        .execute(&db)
        .await
        .unwrap();
    }
    sqlx::query!(
        "INSERT INTO user_profiles (user_id, bio, hidden, updated_at) VALUES ($1, '简介  第一行\n第二行', FALSE, now()),
                                                                     ($2, '', TRUE, now())",
        BOB,
        DAVE,
    )
    .execute(&db)
    .await
    .unwrap();
    // Alice 拉黑了 Carol。
    sqlx::query!(
        "INSERT INTO user_blocks (blocker_id, blocked_id, created_at) VALUES ($1, $2, now())",
        ALICE,
        CAROL,
    )
    .execute(&db)
    .await
    .unwrap();
    let category = ik_content::service::create_category(
        &state.forum.content,
        ik_content::service::NewCategoryInput {
            slug: "guide".into(),
            name: "攻略区".into(),
            description: "攻略与心得".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ik_content::service::create_category(
        &state.forum.content,
        ik_content::service::NewCategoryInput {
            slug: "staff".into(),
            name: "管理公告".into(),
            admin_only: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let guide = post(
        &state,
        BOB,
        "零号空洞攻略",
        "先清怪，再开箱。",
        Some(category.id),
        false,
    )
    .await;
    let anonymous = post(
        &state,
        BOB,
        "零号空洞匿名爆料",
        "听说有隐藏关卡。",
        None,
        true,
    )
    .await;
    let carols = post(
        &state,
        CAROL,
        "零号空洞心得",
        "多带回复道具。",
        Some(category.id),
        false,
    )
    .await;
    let hidden = post(&state, BOB, "零号空洞被隐藏", "不该被看到。", None, false).await;
    let mut conn = db.acquire().await.unwrap();
    ik_content::interop::hide_post(
        &mut conn,
        hidden,
        None,
        "moderator",
        Utc::now(),
        &mut Effects::new(),
    )
    .await
    .unwrap();
    drop(conn);
    World {
        db,
        state,
        guide,
        anonymous,
        carols,
        hidden,
    }
}

impl World {
    async fn ctx(&self) -> ToolContext {
        ToolContext::new(&self.state, ALICE).await.unwrap()
    }

    async fn run(&self, name: &str, args: serde_json::Value) -> tools::ToolOutput {
        let ctx = self.ctx().await;
        tools::execute(&self.state, &ctx, name, &args.to_string()).await
    }

    async fn text(&self, name: &str, args: serde_json::Value) -> String {
        let out = self.run(name, args).await;
        assert!(!out.is_error, "{name}: {}", out.text);
        out.text
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn search_returns_public_named_unblocked_posts_as_links(db: PgPool) {
    let w = world(db).await;
    let out = w
        .run(
            "search_posts",
            serde_json::json!({ "query": "有没有零号空洞的帖子？" }),
        )
        .await;
    assert!(
        out.text
            .contains(&format!("[零号空洞攻略](/post/{})", w.guide)),
        "{}",
        out.text
    );
    // 匿名帖、隐藏帖、被拉黑的人的帖都不出现。
    for id in [w.anonymous, w.hidden, w.carols] {
        assert!(!out.text.contains(&format!("/post/{id})")), "{}", out.text);
    }
    assert!(out.text.contains("先清怪，再开箱。"));
    match out.meta {
        ToolMeta::Search { query, hits, items } => {
            assert_eq!(query, "有没有零号空洞的帖子？");
            assert_eq!(hits, 1);
            assert_eq!(items, vec![(w.guide, "零号空洞攻略".to_owned())]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        w.text("search_posts", serde_json::json!({ "query": "帖子" }))
            .await,
        "（未提供有效检索词）"
    );
    assert_eq!(
        w.text(
            "search_posts",
            serde_json::json!({ "query": "不存在的东西" })
        )
        .await,
        "没有找到与「不存在的东西」相关的帖子。"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn search_can_filter_keywords_by_public_category(db: PgPool) {
    let w = world(db).await;
    let outside = post(
        &w.state,
        DAVE,
        "零号空洞实录",
        "另一分区的相同关键词。",
        None,
        false,
    )
    .await;
    let out = w
        .run(
            "search_posts",
            serde_json::json!({ "query": "零号空洞", "category": "guide", "limit": 1 }),
        )
        .await;
    assert!(out.text.contains(&format!("/post/{})", w.guide)));
    assert!(!out.text.contains(&format!("/post/{outside})")));
    assert!(
        matches!(out.meta, ToolMeta::Search { items, .. } if items.len() == 1 && items[0].0 == w.guide)
    );
    let out = w
        .run(
            "search_posts",
            serde_json::json!({ "query": "零号空洞", "category": "staff" }),
        )
        .await;
    assert_eq!(out.text, "没有找到分类「staff」。");
}

fn page_body(text: &str) -> &str {
    text.split_once("\n帖子正文：")
        .unwrap()
        .1
        .rsplit_once("\n读取范围：")
        .unwrap()
        .0
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn post_content_continues_unicode_body_without_losing_statistics(db: PgPool) {
    let w = world(db).await;
    let body = "甲🙂乙é".repeat(1300);
    let id = post(&w.state, BOB, "长篇测试", &body, None, false).await;
    let first = w
        .run("get_post_content", serde_json::json!({ "document_id": id }))
        .await;
    assert!(!first.is_error);
    assert!(first.text.chars().count() <= tools::MAX_RESULT_CHARS);
    assert!(first.text.contains("\n互动：阅读 "));
    assert!(first.text.contains("\ntruncated: true\n"));
    let offset: usize = first
        .text
        .lines()
        .last()
        .unwrap()
        .strip_prefix("next_offset: ")
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(offset, page_body(&first.text).chars().count());
    let second = w
        .run(
            "get_post_content",
            serde_json::json!({ "document_id": id, "offset": offset }),
        )
        .await;
    assert!(!second.is_error);
    assert!(second.text.chars().count() <= tools::MAX_RESULT_CHARS);
    assert!(second.text.ends_with("truncated: false\nnext_offset: null"));
    assert_eq!(
        format!("{}{}", page_body(&first.text), page_body(&second.text)),
        body
    );
    for offset in [
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!("no"),
        serde_json::json!(body.chars().count() + 1),
    ] {
        assert!(
            w.run(
                "get_post_content",
                serde_json::json!({ "document_id": id, "offset": offset })
            )
            .await
            .is_error
        );
    }
}

async fn out_images(w: &World, id: i64) -> Vec<String> {
    w.run("get_post_content", serde_json::json!({ "document_id": id }))
        .await
        .images
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn post_content_hides_author_and_respects_visibility(db: PgPool) {
    let w = world(db).await;
    // 匿名帖可读，不输出作者。
    let out = w
        .run(
            "get_post_content",
            serde_json::json!({ "document_id": w.anonymous.to_string() }),
        )
        .await;
    assert!(out.text.starts_with("帖子标题：零号空洞匿名爆料\n"));
    assert!(out.text.contains("\n帖子正文：听说有隐藏关卡。\n"));
    assert!(out.text.contains("\n互动：阅读 ") && out.text.contains(" · 收藏 "));
    assert!(out.text.ends_with("truncated: false\nnext_offset: null"));
    assert!(!out.text.contains("bob"));
    assert_eq!(
        out.meta,
        ToolMeta::Read {
            post: Some((w.anonymous, "零号空洞匿名爆料".to_owned()))
        }
    );
    // 隐藏帖、被拉黑的人的帖当作不存在。
    for id in [w.hidden, w.carols] {
        let out = w
            .run("get_post_content", serde_json::json!({ "document_id": id }))
            .await;
        assert_eq!(out.text, format!("没有找到帖子（document_id: {id}）。"));
    }
    assert_eq!(
        w.text("get_post_content", serde_json::json!({})).await,
        "（未提供帖子 document_id）"
    );
    // 正文插图随结果带回，识图模型另收一条带图的消息（LLM-015）。
    assert!(out_images(&w, w.anonymous).await.is_empty());
    let pictured = post(
        &w.state,
        ALICE,
        "看图",
        "配图 ![x](https://cdn.test/p.png)",
        None,
        false,
    )
    .await;
    assert_eq!(
        out_images(&w, pictured).await,
        vec!["https://cdn.test/p.png".to_owned()]
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn comments_are_hot_first_with_pseudonyms_and_block_filter(db: PgPool) {
    let w = world(db).await;
    let top = comment(&w.state, ALICE, w.guide, None, "@bob 写得好", false).await;
    comment(&w.state, DAVE, w.guide, Some(top), "同意", false).await;
    let popular = comment(&w.state, DAVE, w.guide, None, "收藏了", false).await;
    // 被拉黑的人：实名评论不出现，匿名评论照常出现（匿名不做拉黑过滤）。
    comment(&w.state, CAROL, w.guide, None, "卡罗尔实名", false).await;
    comment(&w.state, CAROL, w.guide, None, "卡罗尔匿名", true).await;
    let hidden = comment(&w.state, DAVE, w.guide, None, "被隐藏", false).await;
    let mut conn = w.db.acquire().await.unwrap();
    ik_comments::interop::hide_comment(
        &mut conn,
        hidden,
        None,
        "moderator",
        Utc::now(),
        &mut Effects::new(),
    )
    .await
    .unwrap();
    sqlx::query!("UPDATE comments SET like_count = 5 WHERE id = $1", popular)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let out = w
        .run(
            "get_post_comments",
            serde_json::json!({ "document_id": w.guide }),
        )
        .await;
    let lines: Vec<&str> = out.text.lines().collect();
    assert_eq!(
        lines[0],
        "帖子《零号空洞攻略》共有 4 条评论，按热度展示前 4 条："
    );
    assert_eq!(lines[1], "#3 dave：收藏了（赞5）");
    assert_eq!(lines[2], "#1 alice：@bob 写得好（赞0）");
    assert_eq!(lines[3], "#2 dave：回复 #1 alice，同意（赞0）");
    assert!(
        lines[4].starts_with("#5 绳网用户") && lines[4].ends_with("：卡罗尔匿名（赞0）"),
        "{}",
        lines[4]
    );
    assert!(!out.text.contains("卡罗尔实名") && !out.text.contains("被隐藏"));
    assert_eq!(
        out.meta,
        ToolMeta::Comments {
            post: Some((w.guide, "零号空洞攻略".to_owned()))
        }
    );
    let empty = w
        .text(
            "get_post_comments",
            serde_json::json!({ "document_id": w.anonymous }),
        )
        .await;
    assert_eq!(empty, "帖子《零号空洞匿名爆料》目前还没有评论。");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn users_are_found_exactly_and_hidden_ones_are_not(db: PgPool) {
    let w = world(db).await;
    let info = w
        .text("get_user_info", serde_json::json!({ "username": "@bob" }))
        .await;
    let lines: Vec<&str> = info.lines().collect();
    assert_eq!(lines[0], "用户：bob（@bob）");
    assert!(
        lines[1].starts_with("等级：Lv.") && lines[2].starts_with("丁尼："),
        "{info}"
    );
    assert!(info.contains("\n简介：简介 第一行 第二行\n"));
    // 只做精确匹配（D-085 方案 A）；隐藏主页、与查看者有拉黑关系的当作查无此人。
    for name in ["bo", "dave", "carol"] {
        assert_eq!(
            w.text("get_user_info", serde_json::json!({ "username": name }))
                .await,
            format!("没有找到用户「{name}」。")
        );
    }
    assert_eq!(
        w.text("get_user_info", serde_json::json!({})).await,
        "（未提供用户名）"
    );
    assert!(info.contains("\n发帖数：1\n"), "{info}");
    assert!(info.ends_with("粉丝数：0\n关注数：0"));

    let posts = w
        .text(
            "get_user_activity",
            serde_json::json!({ "username": "bob", "kind": "posts" }),
        )
        .await;
    assert!(posts.starts_with("@bob 的近期动态：\n最新发帖：\n"));
    assert!(posts.contains(&format!("[零号空洞攻略](/post/{})", w.guide)));
    assert!(!posts.contains("匿名爆料") && !posts.contains("被隐藏"));

    comment(&w.state, BOB, w.guide, None, "补充一点", false).await;
    comment(&w.state, BOB, w.guide, None, "匿名补充", true).await;
    let activity = w
        .text(
            "get_user_activity",
            serde_json::json!({ "username": "bob", "limit": 3 }),
        )
        .await;
    assert!(activity.starts_with("@bob 的近期动态：\n最新发帖："));
    assert!(
        activity.contains(&format!(
            "\n最新评论：\n1. 在[零号空洞攻略](/post/{})中：补充一点（赞0）",
            w.guide
        )),
        "{activity}"
    );
    assert!(!activity.contains("匿名补充"));
    let comments = w
        .text(
            "get_user_activity",
            serde_json::json!({ "username": "bob", "kind": "comments" }),
        )
        .await;
    assert!(comments.contains("补充一点") && !comments.contains("最新发帖"));
    let posts = w
        .text(
            "get_user_activity",
            serde_json::json!({ "username": "bob", "kind": "posts" }),
        )
        .await;
    assert!(!posts.contains("补充一点") && !posts.contains("最新评论"));
    comment(&w.state, BOB, w.carols, None, "拉黑作者帖子上的评论", false).await;
    let comments = w
        .text(
            "get_user_activity",
            serde_json::json!({ "username": "bob", "kind": "comments" }),
        )
        .await;
    assert!(!comments.contains("拉黑作者帖子上的评论"));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn categories_hot_and_related(db: PgPool) {
    let w = world(db).await;
    let list = w.text("get_category_list", serde_json::json!({})).await;
    assert_eq!(list, "1. 攻略区（/c/guide） - 攻略与心得");
    let by_category = w
        .text("search_posts", serde_json::json!({ "category": "攻略" }))
        .await;
    assert!(by_category.starts_with(&format!(
        "分类「攻略区」下的最新帖子：\n1. [零号空洞攻略](/post/{})\n",
        w.guide
    )));
    assert!(by_category.contains("分区：攻略区 · 发布："));
    assert!(by_category.ends_with("先清怪，再开箱。"));
    assert_eq!(
        w.text("search_posts", serde_json::json!({ "category": "staff" }))
            .await,
        "没有找到分类「staff」。"
    );

    w.state.forum.feed.refresh().await.unwrap();
    let hot = w
        .text("get_hot_posts", serde_json::json!({ "limit": 3 }))
        .await;
    assert!(!hot.contains(&format!("/post/{})", w.carols)), "{hot}");

    let related = w
        .text(
            "search_posts",
            serde_json::json!({ "seed_post_id": w.guide }),
        )
        .await;
    assert_eq!(related, "没有找到相关帖子。");
    let more = post(
        &w.state,
        DAVE,
        "防御配装细节",
        "第二层怎么打。",
        None,
        false,
    )
    .await;
    let category = ik_content::service::detail(&w.state.forum.content, None, w.guide)
        .await
        .unwrap()
        .category
        .unwrap()
        .id;
    for _ in 0..10 {
        post(
            &w.state,
            BOB,
            "匿名推荐候选",
            "同分区候选排在实名帖前面。",
            Some(category),
            true,
        )
        .await;
    }
    // 游客推荐的前 10 条全被匿名候选占满，AI 仍应找到后面的实名帖。
    let guest = ik_content::service::related(&w.state.forum.content, None, w.guide, Some(10))
        .await
        .unwrap();
    assert_eq!(guest.len(), 10);
    assert!(guest.iter().all(|card| card.is_anonymous));
    let related = w
        .text(
            "search_posts",
            serde_json::json!({ "seed_post_id": w.guide }),
        )
        .await;
    // 标题毫无关键词重合，仍可由论坛推荐算法找到。
    assert!(related.contains(&format!("[防御配装细节](/post/{more})")));
    assert!(related.ends_with("第二层怎么打。"));
    let own = post(
        &w.state,
        ALICE,
        "我的攻略",
        "自己的帖不推荐给自己。",
        None,
        false,
    )
    .await;
    let related = w
        .run(
            "search_posts",
            serde_json::json!({ "seed_post_id": w.guide, "viewerUserId": DAVE }),
        )
        .await;
    for id in [w.guide, w.anonymous, w.carols, w.hidden, own] {
        assert!(
            !related.text.contains(&format!("/post/{id})")),
            "{}",
            related.text
        );
    }
    assert!(
        matches!(related.meta, ToolMeta::Search { items, .. } if items.iter().any(|(id, _)| *id == more))
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unknown_tools_and_bad_arguments(db: PgPool) {
    let w = world(db).await;
    let ctx = w.ctx().await;
    let out = tools::execute(&w.state, &ctx, "delete_post", "{}").await;
    assert!(out.is_error);
    assert_eq!(out.text, "未知工具：delete_post");
    for removed in [
        "get_user_stats",
        "get_posts_by_author",
        "search_posts_by_category",
        "get_related_posts",
        "get_post_engagement",
    ] {
        let out = tools::execute(&w.state, &ctx, removed, "{}").await;
        assert!(out.is_error, "{removed}");
    }
    for args in [
        serde_json::json!({ "seed_post_id": w.guide, "query": "攻略" }),
        serde_json::json!({ "seed_post_id": w.guide, "category": "guide" }),
    ] {
        assert!(w.run("search_posts", args).await.is_error);
    }
    assert!(
        w.run(
            "get_user_activity",
            serde_json::json!({ "username": "bob", "kind": "likes" })
        )
        .await
        .is_error
    );
    // 参数不是 JSON：按空参数。
    let out = tools::execute(&w.state, &ctx, "get_user_info", "not json").await;
    assert_eq!(out.text, "（未提供用户名）");
    // 参数里塞的身份被忽略（TOOL-004）：查看者仍是 Alice，Carol 的帖不出现。
    let out = tools::execute(
        &w.state,
        &ctx,
        "get_post_content",
        &serde_json::json!({ "document_id": w.carols, "viewerUserId": CAROL.to_string() })
            .to_string(),
    )
    .await;
    assert_eq!(
        out.text,
        format!("没有找到帖子（document_id: {}）。", w.carols)
    );
}
