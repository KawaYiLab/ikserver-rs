//! 媒体与内容导入：草稿行 / 发布行合并、待发布修改、封面、标签合并、评论楼层与楼中楼、采纳、精选。

mod common;

use common::{at, import, setup};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[allow(clippy::unwrap_used)]
async fn seed(db: &PgPool) {
    // 测试里的固定数据（只拼进常量哈希）。
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        r#"
        INSERT INTO legacy.up_users (id, document_id, username, email, account_status, created_at) VALUES
          (1, 'u1', 'alice', 'alice@example.com', 'active', '2025-01-01 00:00:00'),
          (2, 'u2', 'bob', 'bob@example.com', 'active', '2025-01-01 00:00:00');
        INSERT INTO legacy.authors (id, document_id, name, profile_pinned_articles) VALUES
          (11, 'a1', 'alice', '["p1", "p3", "nope"]'), (12, 'a2', 'bob', NULL), (13, 'a3', 'ghost', NULL);
        INSERT INTO legacy.up_users_author_lnk (id, user_id, author_id) VALUES (1, 1, 11), (2, 2, 12);

        INSERT INTO legacy.files (id, name, url, width, height, created_at) VALUES
          (100, 'cover.png', 'https://im.tiwat.cn/uploads/cover.png', 800, 600, '2025-01-02 00:00:00'),
          (101, 'gone.png', 'https://im.tiwat.cn/uploads/gone.png', 10, 10, '2025-01-02 00:00:00'),
          (102, 'pic.webp', 'https://im.tiwat.cn/uploads/pic.webp', NULL, NULL, '2025-01-02 00:00:00'),
          (103, 'face.jpg', 'https://im.tiwat.cn/uploads/face.jpg', 64, 64, '2025-01-02 00:00:00');
        INSERT INTO legacy.ik_media_digests (file_id, object_key, sha256, mime, size_bytes, thumbhash, error) VALUES
          (100, 'uploads/cover.png', '{SHA}', 'image/png', 1000, 'th', NULL),
          (101, 'uploads/gone.png', NULL, NULL, NULL, NULL, '对象不存在'),
          (102, 'uploads/pic.webp', '{SHA}', 'image/webp', 2000, NULL, NULL),
          (103, 'uploads/face.jpg', '{SHA}', 'image/jpeg', 3000, NULL, NULL);
        INSERT INTO legacy.files_uploaders_lnk (id, file_id, user_id) VALUES (1, 100, 1), (2, 101, 1);

        INSERT INTO legacy.categories (id, document_id, name, slug, "order", is_active, created_at) VALUES
          (1, 'cat1', '攻略', 'guides', 2, true, '2025-01-01 00:00:00'),
          (2, 'cat2', '闲聊', 'Bad Slug!', 1, NULL, '2025-01-01 00:00:00');
        INSERT INTO legacy.tags (id, document_id, name, slug, created_at) VALUES
          (1, 't1', 'zzz', 'zzz', '2025-01-01 00:00:00'),
          (2, 't2', 'ZZZ', 'zzz', '2025-01-01 00:00:00'),
          (3, 't3', 'tips', 'tips', '2025-01-01 00:00:00');

        -- p1：草稿行 10（有未发布的修改）+ 发布行 12；p2：只有草稿行；p3：作者没有账号。
        INSERT INTO legacy.articles (id, document_id, title, text, external_videos, views, is_anonymous,
                                     is_hidden, is_pinned, pinned_at, accepted_comment_document_id,
                                     bumped_at, published_at, first_published_at, created_at, updated_at) VALUES
          (10, 'p1', '新标题', '改过的正文', '[]', 0, false, false, true, NULL, 'c1',
           NULL, NULL, NULL, '2025-02-01 00:00:00', '2025-02-05 00:00:00'),
          (12, 'p1', '标题', '图 ![](http://image.tiwat.cn/x.png)',
           '[{{"url":"https://v/1"}}, {{"url":"https://v/2"}}]', 42, false, NULL, true, NULL, 'c1',
           NULL, '2025-02-02 00:00:00', NULL, '2025-02-02 00:00:00', '2025-02-02 00:00:00'),
          (20, 'p2', repeat('长', 60), '草稿', NULL, NULL, true, NULL, false, NULL, NULL,
           NULL, NULL, NULL, '2025-03-01 00:00:00', '2025-03-01 00:00:00'),
          (30, 'p3', '孤儿', '没人认领', NULL, 5, false, false, false, NULL, NULL,
           '2025-04-01 00:00:00', '2025-04-01 00:00:00', '2025-04-01 00:00:00',
           '2025-04-01 00:00:00', '2025-04-01 00:00:00');
        INSERT INTO legacy.articles_author_lnk (id, article_id, author_id) VALUES
          (1, 10, 11), (2, 12, 11), (3, 20, 12), (4, 30, 13);
        INSERT INTO legacy.articles_category_lnk (id, article_id, category_id) VALUES
          (1, 10, 2), (2, 12, 1), (3, 20, 2);
        INSERT INTO legacy.articles_pinned_by_lnk (id, article_id, user_id) VALUES (1, 12, 2);
        INSERT INTO legacy.articles_tags_lnk (id, article_id, tag_id, tag_ord) VALUES
          (1, 12, 3, 1), (2, 12, 1, 2), (3, 12, 2, 3), (4, 10, 2, 1);
        INSERT INTO legacy.files_related_mph (id, file_id, related_id, related_type, field, "order") VALUES
          (1, 101, 12, 'api::article.article', 'cover', 1),
          (2, 100, 12, 'api::article.article', 'cover', 2),
          (3, 102, 103, 'api::comment.comment', 'images', 1),
          (4, 103, 11, 'api::author.author', 'avatar', 1);
        INSERT INTO legacy.articles_cmps (id, entity_id, cmp_id, component_type, field, "order") VALUES
          (1, 12, 1, 'shared.quote', 'blocks', 1);

        -- 评论：100 顶层（挂在草稿行上）、101 回复 100、102 回复 101（回复的回复）、
        -- 103 顶层（被隐藏、带图）、104 在作者不存在的帖子下。
        INSERT INTO legacy.comments (id, document_id, content, is_anonymous, is_hidden, accepted_at,
                                     created_at, updated_at) VALUES
          (100, 'c1', '沙发', false, false, '2025-02-03 00:00:00', '2025-02-02 01:00:00', '2025-02-02 01:00:00'),
          (101, 'c2', '回复', false, NULL, NULL, '2025-02-02 02:00:00', '2025-02-02 02:00:00'),
          (102, 'c3', '再回复', true, false, NULL, '2025-02-02 03:00:00', '2025-02-02 03:00:00'),
          (103, 'c4', repeat('评', 2100), false, true, NULL, '2025-02-02 04:00:00', '2025-02-04 00:00:00'),
          (104, 'c5', '无主', false, false, NULL, '2025-04-02 00:00:00', '2025-04-02 00:00:00');
        INSERT INTO legacy.comments_article_lnk (id, comment_id, article_id) VALUES
          (1, 100, 10), (2, 101, 12), (3, 102, 12), (4, 103, 12), (5, 104, 30);
        INSERT INTO legacy.comments_author_lnk (id, comment_id, author_id) VALUES
          (1, 100, 12), (2, 101, 11), (3, 102, 12), (4, 103, 11), (5, 104, 11);
        INSERT INTO legacy.comments_parent_lnk (id, comment_id, inv_comment_id) VALUES
          (1, 101, 100), (2, 102, 101);
        INSERT INTO legacy.comments_accepted_by_lnk (id, comment_id, user_id) VALUES (1, 100, 1);
        "#
    )))
    .execute(db)
    .await
    .unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn content_is_merged_and_remapped(db: PgPool) {
    setup(&db).await;
    seed(&db).await;

    let findings = ik_legacy::check(&db).await.unwrap();
    let count = |key: &str| {
        findings
            .iter()
            .find(|f| f.key == key)
            .map_or(0, |f| f.count)
    };
    assert_eq!(count("media.failed"), 1);
    assert_eq!(count("posts.no_author"), 1);
    assert_eq!(count("posts.title_truncated"), 1);
    assert_eq!(count("posts.extra_videos"), 1);
    assert_eq!(count("posts.blocks_dropped"), 1);
    assert_eq!(count("tags.merged"), 1);
    assert_eq!(count("categories.fixed"), 1);
    assert_eq!(count("comments.dropped"), 1);
    assert_eq!(count("comments.reparented"), 1);
    assert_eq!(count("comments.body_truncated"), 1);

    import(&db, "UTC").await;

    // 媒体：读取失败的文件不导入；图库与头像。
    let media: Vec<i64> = sqlx::query_scalar!("SELECT id FROM media_files ORDER BY id")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(media, vec![100, 102, 103]);
    let uploads: Vec<i64> = sqlx::query_scalar!("SELECT media_id FROM media_uploaders")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(uploads, vec![100]);
    let avatar = sqlx::query_scalar!("SELECT avatar_media_id FROM users WHERE id = 1")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(avatar, Some(103));

    let categories = sqlx::query!("SELECT id, slug, is_active FROM categories ORDER BY id")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(categories[0].slug, "guides");
    assert_eq!(
        (categories[1].slug.as_str(), categories[1].is_active),
        ("c-2", true)
    );
    let tags: Vec<i64> = sqlx::query_scalar!("SELECT id FROM tags ORDER BY id")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(tags, vec![1, 3], "slug 相同的 2 号合并进 1 号");

    // 帖子：p1 用最小的行 id，内容取发布行；p2 是草稿；p3 作者不存在不导入。
    let posts = sqlx::query!(
        r#"SELECT id, author_id, category_id, status, title, body, external_videos, is_anonymous,
                  anonymous_seed, published_at, first_published_at, bumped_at, pinned_at, pinned_by,
                  hidden_at, view_count, legacy_id, cover_ids, accepted_comment_id
           FROM posts ORDER BY id"#
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(posts.len(), 2);
    let p1 = &posts[0];
    assert_eq!((p1.id, p1.author_id, p1.category_id), (10, 1, Some(1)));
    assert_eq!(
        (p1.status.as_str(), p1.title.as_str()),
        ("published", "标题")
    );
    assert_eq!(
        p1.body, "图 ![](https://im.tiwat.cn/x.png)",
        "旧图床域名改写"
    );
    assert_eq!(
        p1.external_videos,
        serde_json::json!([{ "url": "https://v/1" }])
    );
    let published = at(2025, 2, 2, 0, 0);
    assert_eq!(p1.published_at, Some(published));
    assert_eq!(p1.first_published_at, Some(published));
    assert_eq!(
        p1.bumped_at,
        Some(published),
        "没有顶帖时间时用发布时间（POST-061）"
    );
    assert_eq!((p1.pinned_at, p1.pinned_by), (Some(published), Some(2)));
    assert_eq!(p1.hidden_at, None, "isHidden 为 NULL 视为可见");
    assert_eq!(p1.view_count, 42);
    assert_eq!(p1.legacy_id.as_deref(), Some("p1"));
    assert_eq!(p1.cover_ids, vec![100], "读取失败的封面去掉");
    assert_eq!(p1.accepted_comment_id, Some(100));

    let p2 = &posts[1];
    assert_eq!((p2.id, p2.status.as_str()), (20, "draft"));
    assert_eq!(p2.title.chars().count(), 50);
    assert!(p2.is_anonymous && p2.anonymous_seed.as_deref().is_some_and(|s| s.len() == 32));
    assert_eq!((p2.published_at, p2.bumped_at), (None, None));

    // 待发布修改来自草稿行。
    let draft = sqlx::query!("SELECT post_id, title, body, category_id, tag_ids FROM post_drafts")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(
        (draft.post_id, draft.title.as_str(), draft.body.as_str()),
        (10, "新标题", "改过的正文")
    );
    assert_eq!((draft.category_id, draft.tag_ids), (Some(2), vec![1]));

    let post_tags =
        sqlx::query!("SELECT tag_id, position FROM post_tags WHERE post_id = 10 ORDER BY position")
            .fetch_all(&db)
            .await
            .unwrap();
    let post_tags: Vec<(i64, i16)> = post_tags.iter().map(|t| (t.tag_id, t.position)).collect();
    assert_eq!(
        post_tags,
        vec![(3, 0), (1, 1)],
        "保留原顺序，合并后的重复标签只留一个"
    );

    // 评论：楼层、回复的回复挂到顶层、截断、隐藏、配图；无主帖子下的评论不导入。
    let comments = sqlx::query!(
        r#"SELECT id, post_id, parent_id, author_id, floor, char_length(body) AS "len!",
                  is_anonymous, anonymous_seed, accepted_at, accepted_by, hidden_at, image_ids, legacy_id
           FROM comments ORDER BY id"#
    )
    .fetch_all(&db)
    .await
    .unwrap();
    let shape: Vec<(i64, Option<i64>, Option<i32>)> = comments
        .iter()
        .map(|c| (c.id, c.parent_id, c.floor))
        .collect();
    assert_eq!(
        shape,
        vec![
            (100, None, Some(1)),
            (101, Some(100), None),
            (102, Some(100), None),
            (103, None, Some(2))
        ]
    );
    assert!(comments.iter().all(|c| c.post_id == 10));
    assert_eq!(
        (comments[0].accepted_at, comments[0].accepted_by),
        (Some(at(2025, 2, 3, 0, 0)), Some(1))
    );
    assert!(comments[2].is_anonymous && comments[2].anonymous_seed.is_some());
    assert_eq!(comments[3].len, 2000);
    assert_eq!(comments[3].hidden_at, Some(at(2025, 2, 4, 0, 0)));
    assert_eq!(comments[3].image_ids, vec![102]);

    // 主页精选：只保留本人仍存在的帖子。
    let pinned = sqlx::query_scalar!("SELECT pinned_post_ids FROM user_profiles WHERE user_id = 1")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(pinned, Some(vec![10]));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn reactions_are_deduplicated_and_counted(db: PgPool) {
    setup(&db).await;
    seed(&db).await;
    sqlx::raw_sql(
        r#"
        -- 1 号对 p1 点了两次（草稿行、发布行各一次），2 号点了 p3（作者不存在，不导入）；
        -- 9 号账号不存在；评论点赞挂了帖子和评论两个关系。
        INSERT INTO legacy.likes (id, created_at) VALUES
          (1, '2025-02-03 00:00:00'), (2, '2025-02-04 00:00:00'), (3, '2025-02-03 00:00:00'),
          (4, '2025-02-03 00:00:00'), (5, '2025-02-05 00:00:00');
        INSERT INTO legacy.likes_article_lnk (id, like_id, article_id) VALUES
          (1, 1, 10), (2, 2, 12), (3, 3, 30), (4, 4, 12), (5, 5, 12);
        INSERT INTO legacy.likes_comment_lnk (id, like_id, comment_id) VALUES (1, 5, 100);
        INSERT INTO legacy.likes_user_lnk (id, like_id, user_id) VALUES
          (1, 1, 1), (2, 2, 1), (3, 3, 2), (4, 4, 9), (5, 5, 2);

        INSERT INTO legacy.favorites (id, created_at) VALUES (1, '2025-02-03 00:00:00');
        INSERT INTO legacy.favorites_article_lnk (id, favorite_id, article_id) VALUES (1, 1, 12);
        INSERT INTO legacy.favorites_user_lnk (id, favorite_id, user_id) VALUES (1, 1, 2);

        INSERT INTO legacy.denny_givens (id, amount, message, created_at) VALUES
          (1, 3, repeat('谢', 120), '2025-02-03 00:00:00');
        INSERT INTO legacy.denny_givens_article_lnk (id, denny_given_id, article_id) VALUES (1, 1, 12);
        INSERT INTO legacy.denny_givens_user_lnk (id, denny_given_id, user_id) VALUES (1, 1, 2);

        INSERT INTO legacy.article_reads (id, is_read, created_at) VALUES (1, false, '2025-02-03 00:00:00');
        INSERT INTO legacy.article_reads_article_lnk (id, article_read_id, article_id) VALUES (1, 1, 10);
        INSERT INTO legacy.article_reads_user_lnk (id, article_read_id, user_id) VALUES (1, 1, 2);

        -- 1 号关注作者 a2（2 号）两次、关注自己一次；2 号拉黑 1 号。
        INSERT INTO legacy.follows (id, created_at) VALUES
          (1, '2025-02-03 00:00:00'), (2, '2025-02-04 00:00:00'), (3, '2025-02-04 00:00:00');
        INSERT INTO legacy.follows_follower_lnk (id, follow_id, user_id) VALUES (1, 1, 1), (2, 2, 1), (3, 3, 1);
        INSERT INTO legacy.follows_following_lnk (id, follow_id, author_id) VALUES (1, 1, 12), (2, 2, 12), (3, 3, 11);
        INSERT INTO legacy.user_blocks (id, created_at) VALUES (1, '2025-02-03 00:00:00');
        INSERT INTO legacy.user_blocks_blocker_lnk (id, user_block_id, user_id) VALUES (1, 1, 2);
        INSERT INTO legacy.user_blocks_blocked_user_lnk (id, user_block_id, user_id) VALUES (1, 1, 1);
        "#,
    )
    .execute(&db)
    .await
    .unwrap();

    import(&db, "UTC").await;

    let likes = sqlx::query!("SELECT post_id, user_id, created_at FROM post_likes")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(likes.len(), 1);
    assert_eq!((likes[0].post_id, likes[0].user_id), (10, 1));
    assert_eq!(
        likes[0].created_at,
        at(2025, 2, 3, 0, 0),
        "重复的只留最早一条"
    );

    let comment_likes = sqlx::query!("SELECT comment_id, user_id FROM comment_likes")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!((comment_likes.len(), comment_likes[0].comment_id), (1, 100));

    let gift =
        sqlx::query!("SELECT amount, char_length(message) AS \"len!\" FROM post_denny_gifts")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!((gift.amount, gift.len), (1, 100));

    let reads =
        sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM post_reads WHERE post_id = 10"#)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(reads, 1, "isRead = false 的记录也算已读");

    let follows = sqlx::query!("SELECT follower_id, followee_id FROM follows")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(follows.len(), 1);
    assert_eq!((follows[0].follower_id, follows[0].followee_id), (1, 2));
    let blocks = sqlx::query!("SELECT blocker_id, blocked_id FROM user_blocks")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!((blocks.blocker_id, blocks.blocked_id), (2, 1));

    let counters = sqlx::query!(
        "SELECT like_count, favorite_count, denny_count, comment_count FROM posts WHERE id = 10"
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(
        (
            counters.like_count,
            counters.favorite_count,
            counters.denny_count,
            counters.comment_count
        ),
        (1, 1, 1, 4)
    );
    let comment_like_count = sqlx::query_scalar!("SELECT like_count FROM comments WHERE id = 100")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(comment_like_count, 1);
}
