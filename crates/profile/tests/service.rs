//! 个人资料 / 作者主页 service 层集成测试：真实 PostgreSQL，每个测试一个独立临时库。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use ik_auth::Principal;
use ik_comments::CommentsState;
use ik_comments::dto::CreateCommentRequest;
use ik_content::ContentState;
use ik_content::dto::PostInput;
use ik_kernel::bus::LocalBus;
use ik_kernel::{Id, Role, Snowflake};
use ik_profile::ProfileState;
use ik_profile::dto::{
    PageQuery, PinnedPostsRequest, PostsScope, ProfilePostsQuery, RenameRequest, SearchQuery,
    UpdateProfileRequest,
};
use ik_profile::service;
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

struct Harness {
    state: ProfileState,
    db: PgPool,
}

fn harness(db: PgPool) -> Harness {
    let bus = Arc::new(LocalBus::new(256));
    let ids = Arc::new(Snowflake::new(1).unwrap());
    let media = ik_media::MediaState::for_tests(db.clone(), ids.clone());
    let content = ContentState::new(db.clone(), ids.clone(), bus.clone(), media.clone());
    let comments = CommentsState::new(db.clone(), ids.clone(), bus, media);
    Harness {
        state: ProfileState::new(
            db.clone(),
            ids.clone(),
            content,
            comments,
            ik_media::MediaState::for_tests(db.clone(), ids),
        ),
        db,
    }
}

impl Harness {
    async fn user(&self, id: i64, name: &str) -> Principal {
        sqlx::query!(
            "INSERT INTO users (id, username, email, level, exam_passed_at)
             VALUES ($1, $2, $3, 1, now())",
            id,
            name,
            format!("{id}@example.com"),
        )
        .execute(&self.db)
        .await
        .unwrap();
        Principal {
            user_id: Id::new(id),
            session_id: Id::new(id),
            role: Role::User,
            exam_passed: true,
            is_ai_agent: false,
            level: 1,
        }
    }

    async fn post(&self, author: &Principal, title: &str, anonymous: bool) -> i64 {
        let input = PostInput {
            title: title.into(),
            body: "正文".into(),
            category_id: None,
            tags: Vec::new(),
            is_anonymous: anonymous,
            external_videos: Vec::new(),
            editor_state: None,
            cover_ids: Vec::new(),
        };
        ik_content::service::create(&self.state.content, author, input, true)
            .await
            .unwrap()
            .id
            .get()
    }

    async fn comment(&self, author: &Principal, post: i64, anonymous: bool) -> i64 {
        ik_comments::service::create(
            &self.state.comments,
            author,
            post,
            CreateCommentRequest {
                body: "评论".into(),
                parent_id: None,
                is_anonymous: anonymous,
                image_ids: Vec::new(),
            },
        )
        .await
        .unwrap()
        .id
        .get()
    }

    /// 测试里准备数据用的固定 SQL（字面量拼上数字 id），不含外部输入。
    async fn sql(&self, statement: &str) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement.to_owned()))
            .execute(&self.db)
            .await
            .unwrap();
    }

    async fn balance(&self, id: i64) -> i32 {
        sqlx::query_scalar!(
            "SELECT denny_balance FROM user_economy WHERE user_id = $1",
            id
        )
        .fetch_optional(&self.db)
        .await
        .unwrap()
        .unwrap_or(30)
    }

    async fn username(&self, id: i64) -> String {
        sqlx::query_scalar!("SELECT username FROM users WHERE id = $1", id)
            .fetch_one(&self.db)
            .await
            .unwrap()
    }
}

fn all_posts() -> ProfilePostsQuery {
    ProfilePostsQuery {
        scope: PostsScope::All,
        cursor: None,
        limit: None,
    }
}

fn default_posts() -> ProfilePostsQuery {
    ProfilePostsQuery {
        scope: PostsScope::Default,
        cursor: None,
        limit: None,
    }
}

fn page() -> PageQuery {
    PageQuery {
        cursor: None,
        limit: None,
    }
}

fn search(q: &str) -> SearchQuery {
    SearchQuery {
        q: q.into(),
        limit: None,
    }
}

const IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

#[sqlx::test(migrator = "MIGRATOR")]
async fn profile_counts_only_public_named_content(db: PgPool) {
    let h = harness(db);
    let author = h.user(1, "author").await;
    let reader = h.user(2, "reader").await;
    let named = h.post(&author, "实名", false).await;
    h.post(&author, "匿名", true).await;
    h.comment(&author, named, false).await;
    h.comment(&author, named, true).await;
    h.comment(&reader, named, false).await;
    service::update_profile(
        &h.state,
        &author,
        UpdateProfileRequest {
            bio: Some("  你好  ".into()),
            hidden: None,
        },
    )
    .await
    .unwrap();
    sqlx::query!("INSERT INTO follows (follower_id, followee_id) VALUES (2, 1)")
        .execute(&h.db)
        .await
        .unwrap();

    let view = service::profile(&h.state, Some(&reader), 1).await.unwrap();
    assert_eq!(view.bio, "你好");
    assert!(!view.is_self && !view.is_hidden && view.is_following);
    assert_eq!((view.follower_count, view.following_count), (1, 0));
    let stats = view.stats.unwrap();
    assert_eq!(stats.post_count, 1, "匿名帖不计入");
    assert_eq!(stats.comment_count, 1, "匿名评论不计入");
    assert_eq!(stats.total_comments, 3, "帖子下的评论总数含他人与匿名评论");
    assert!(view.exp.is_some());

    let posts = service::posts(&h.state, None, 1, all_posts())
        .await
        .unwrap();
    assert_eq!(posts.items.len(), 1);
    assert!(!posts.pinned);
    let comments = service::comments(&h.state, None, 1, page()).await.unwrap();
    assert_eq!(comments.items.len(), 1);
    assert_eq!(comments.items[0].post_title, "实名");

    assert_eq!(
        service::profile(&h.state, None, 999)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn blocked_and_hidden_profiles_are_reduced(db: PgPool) {
    let h = harness(db);
    let author = h.user(1, "author").await;
    let blocker = h.user(2, "blocker").await;
    let stranger = h.user(3, "stranger").await;
    h.post(&author, "帖子", false).await;
    service::update_profile(
        &h.state,
        &author,
        UpdateProfileRequest {
            bio: Some("签名".into()),
            hidden: None,
        },
    )
    .await
    .unwrap();
    h.sql("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES (2, 1)")
        .await;
    h.sql("INSERT INTO follows (follower_id, followee_id) VALUES (3, 1)")
        .await;

    let view = service::profile(&h.state, Some(&blocker), 1).await.unwrap();
    assert!(view.is_blocked_by_me && !view.has_blocked_me);
    assert!(view.bio.is_empty() && view.stats.is_none() && view.exp.is_none());
    assert_eq!(view.follower_count, 0);
    // 被拉黑的一方看到的是另一个方向。
    let reverse = service::profile(&h.state, Some(&author), 2).await.unwrap();
    assert!(reverse.has_blocked_me && !reverse.is_blocked_by_me);
    let posts = service::posts(&h.state, Some(&blocker), 1, all_posts())
        .await
        .unwrap();
    assert!(posts.items.is_empty());

    // 装备系统头像与名片。
    h.sql(&format!(
        "INSERT INTO media_files (id, object_key, mime, size_bytes, sha256, receipt)
         VALUES (900, 'c/900.png', 'image/png', 1, '{}', 'r900');
         INSERT INTO system_avatars (id, name, image_media_id) VALUES (1, '头像', 900);
         INSERT INTO business_cards (id, name, image_media_id) VALUES (1, '名片', 900);
         UPDATE users SET equipped_avatar_id = 1, equipped_avatar_media_id = 900, equipped_card_id = 1
         WHERE id = 1;
         INSERT INTO mihoyo_bindings (user_id, aid, zzz_uid, zzz_nickname, zzz_level, zzz_region_name)
         VALUES (1, 'aid-1', '10000001', '邦布', 45, '新艾利都');",
        "f".repeat(64)
    ))
    .await;
    let view = service::profile(&h.state, Some(&stranger), 1)
        .await
        .unwrap();
    assert_eq!(view.equipped_avatar.unwrap().name, "头像");
    assert_eq!(view.equipped_card.unwrap().name, "名片");
    let zzz = view.zzz.unwrap();
    assert_eq!((zzz.uid.as_str(), zzz.level), ("10000001", 45));

    service::update_profile(
        &h.state,
        &author,
        UpdateProfileRequest {
            bio: None,
            hidden: Some(true),
        },
    )
    .await
    .unwrap();
    let view = service::profile(&h.state, Some(&stranger), 1)
        .await
        .unwrap();
    assert!(view.is_hidden && view.bio.is_empty() && view.stats.is_none());
    assert!(
        view.equipped_avatar.is_none() && view.equipped_card.is_none(),
        "隐藏主页不给装扮（PROFILE-022）"
    );
    assert!(view.zzz.is_none());
    assert_eq!(view.follower_count, 1, "隐藏主页仍返回关注数");
    assert!(view.is_following);
    assert!(
        service::posts(&h.state, None, 1, all_posts())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    // 本人永远看到完整视图；签名在隐藏期间保留。
    let own = service::profile(&h.state, Some(&author), 1).await.unwrap();
    assert!(own.is_self && !own.is_hidden);
    assert_eq!(own.bio, "签名");
    assert!(own.equipped_card.is_some());
    // 拉黑优先于隐藏。
    let view = service::profile(&h.state, Some(&blocker), 1).await.unwrap();
    assert!(!view.is_hidden && view.is_blocked_by_me);
    assert!(
        view.equipped_card.is_some(),
        "拉黑时保留名片（PROFILE-021）"
    );
    let mine = service::my_profile(&h.state, &author).await.unwrap();
    assert_eq!(mine.equipped_avatar.unwrap().name, "头像");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bio_is_limited_to_100_chars(db: PgPool) {
    let h = harness(db);
    let me = h.user(1, "me1").await;
    let err = service::update_profile(
        &h.state,
        &me,
        UpdateProfileRequest {
            bio: Some("字".repeat(101)),
            hidden: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "VALIDATION_FAILED");
    let view = service::update_profile(
        &h.state,
        &me,
        UpdateProfileRequest {
            bio: Some("字".repeat(100)),
            hidden: Some(true),
        },
    )
    .await
    .unwrap();
    assert!(view.hidden);
    assert!(view.pinned_post_ids.is_none());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rename_costs_10_denny_and_rolls_back_on_failure(db: PgPool) {
    let h = harness(db);
    let me = h.user(1, "original").await;
    h.user(2, "taken").await;

    // 同名：不扣费。
    let same = service::rename(
        &h.state,
        &me,
        RenameRequest {
            username: " original ".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(same.balance, 30);

    let renamed = service::rename(
        &h.state,
        &me,
        RenameRequest {
            username: "新名字abc".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (renamed.username.as_str(), renamed.balance),
        ("新名字abc", 20)
    );
    assert_eq!(h.username(1).await, "新名字abc");

    // 被占用：不扣费、不改名。区分大小写（D-018），`Taken` 不算冲突。
    let err = service::rename(
        &h.state,
        &me,
        RenameRequest {
            username: "taken".into(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "USERNAME_TAKEN");
    assert_eq!(h.balance(1).await, 20);
    service::rename(
        &h.state,
        &me,
        RenameRequest {
            username: "Taken".into(),
        },
    )
    .await
    .unwrap();

    // 余额不足：名字不变。
    h.sql("UPDATE user_economy SET denny_balance = 9 WHERE user_id = 1")
        .await;
    let err = service::rename(
        &h.state,
        &me,
        RenameRequest {
            username: "再改一次".into(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "INSUFFICIENT_DENNY");
    assert_eq!(h.username(1).await, "Taken");
    assert_eq!(h.balance(1).await, 9);

    for bad in ["ab", "a@bc", "名字\n换行"] {
        let err = service::rename(
            &h.state,
            &me,
            RenameRequest {
                username: bad.into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "VALIDATION_FAILED", "{bad:?}");
    }
}

/// 两个人同时抢同一个名字：只有一个成功，失败的一方不扣费（PROFILE-006）。
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_renames_to_the_same_name(db: PgPool) {
    let h = harness(db);
    let a = h.user(1, "alice").await;
    let b = h.user(2, "bob").await;
    let rename = |who: Principal| {
        let state = h.state.clone();
        tokio::spawn(async move {
            service::rename(
                &state,
                &who,
                RenameRequest {
                    username: "抢手的名字".into(),
                },
            )
            .await
        })
    };
    let (ra, rb) = tokio::join!(rename(a), rename(b));
    let results = [ra.unwrap(), rb.unwrap()];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    let loser = results.iter().find_map(|r| r.as_ref().err()).unwrap();
    assert_eq!(loser.code(), "USERNAME_TAKEN");
    assert_eq!(
        h.balance(1).await + h.balance(2).await,
        50,
        "只有赢家扣了 10"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn pinned_posts_are_filtered_ordered_and_capped(db: PgPool) {
    let h = harness(db);
    let me = h.user(1, "author").await;
    let other = h.user(2, "other").await;
    let a = h.post(&me, "A", false).await;
    let b = h.post(&me, "B", false).await;
    let c = h.post(&me, "C", false).await;
    let anonymous = h.post(&me, "匿名", true).await;
    let not_mine = h.post(&other, "别人的", false).await;
    h.sql(&format!(
        "UPDATE posts SET hidden_at = now() WHERE id = {c}"
    ))
    .await;

    let ids = |list: &[i64]| list.iter().map(|id| format!(" {id} ")).collect();
    let saved = service::set_pinned(
        &h.state,
        &me,
        PinnedPostsRequest {
            post_ids: Some(ids(&[b, a, b, anonymous, not_mine, c, 12345])),
        },
    )
    .await
    .unwrap();
    let saved: Vec<i64> = saved
        .pinned_post_ids
        .unwrap()
        .iter()
        .map(|id| id.get())
        .collect();
    assert_eq!(
        saved,
        vec![b, a, c],
        "去重、保序，剔除匿名 / 别人的 / 不存在的，保留被隐藏的"
    );

    let visitor = service::posts(&h.state, Some(&other), 1, default_posts())
        .await
        .unwrap();
    assert!(visitor.pinned);
    let order: Vec<i64> = visitor.items.iter().map(|p| p.id.get()).collect();
    assert_eq!(order, vec![b, a], "访客看不到被隐藏的精选");
    let own = service::posts(&h.state, Some(&me), 1, default_posts())
        .await
        .unwrap();
    assert_eq!(own.items.len(), 3);
    let everything = service::posts(&h.state, Some(&other), 1, all_posts())
        .await
        .unwrap();
    assert!(!everything.pinned);
    assert_eq!(everything.items.len(), 2);

    let too_many: Vec<String> = (1..=7).map(|i| i.to_string()).collect();
    let err = service::set_pinned(
        &h.state,
        &me,
        PinnedPostsRequest {
            post_ids: Some(too_many),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "VALIDATION_FAILED");

    // 清空 → 主页精选模式但没有帖子；null → 恢复默认。
    let cleared = service::set_pinned(
        &h.state,
        &me,
        PinnedPostsRequest {
            post_ids: Some(Vec::new()),
        },
    )
    .await
    .unwrap();
    assert_eq!(cleared.pinned_post_ids.unwrap().len(), 0);
    let page = service::posts(&h.state, None, 1, default_posts())
        .await
        .unwrap();
    assert!(page.pinned && page.items.is_empty());
    service::set_pinned(&h.state, &me, PinnedPostsRequest { post_ids: None })
        .await
        .unwrap();
    let page = service::posts(&h.state, None, 1, default_posts())
        .await
        .unwrap();
    assert!(!page.pinned);
    assert_eq!(page.items.len(), 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn search_ranks_matches_and_skips_hidden_and_blocked(db: PgPool) {
    let h = harness(db);
    let me = h.user(1, "searcher").await;
    h.user(2, "xmeow").await;
    h.user(3, "meow").await;
    h.user(4, "meowmeow").await;
    h.user(5, "meow_hidden").await;
    h.user(6, "meow_blocked").await;
    h.sql("INSERT INTO user_profiles (user_id, hidden) VALUES (5, TRUE)")
        .await;
    h.sql("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES (6, 1)")
        .await;
    h.sql("INSERT INTO follows (follower_id, followee_id) VALUES (1, 4)")
        .await;

    let found = service::search(&h.state, Some(&me), IP, search("MEOW"))
        .await
        .unwrap();
    let names: Vec<&str> = found.iter().map(|u| u.username.as_str()).collect();
    assert_eq!(
        names,
        vec!["meow", "meowmeow", "xmeow"],
        "完全匹配、前缀、其他"
    );
    assert!(found[1].is_following);
    assert_eq!(found[1].follower_count, 1);

    // 游客能看到被拉黑者（拉黑只对双方生效），但看不到隐藏主页的人。
    let guest = service::search(&h.state, None, IP, search("meow"))
        .await
        .unwrap();
    assert_eq!(guest.len(), 4);
    assert!(
        service::search(&h.state, None, IP, search("  "))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        service::search(&h.state, Some(&me), IP, search(&"m".repeat(33)))
            .await
            .unwrap()
            .is_empty()
    );

    let mentions = service::mention_candidates(&h.state, &me, search("meow"))
        .await
        .unwrap();
    assert_eq!(mentions.len(), 3);
    assert_eq!(
        service::mention_candidates(&h.state, &me, search(""))
            .await
            .unwrap_err()
            .code(),
        "VALIDATION_FAILED"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn search_like_metacharacters_are_literal(db: PgPool) {
    let h = harness(db);
    h.user(1, "a_b_c").await;
    h.user(2, "axbxc").await;
    let found = service::search(&h.state, None, IP, search("a_b"))
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].username, "a_b_c");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn custom_avatar_costs_10_denny_and_must_be_own_upload(db: PgPool) {
    let h = harness(db);
    let me = h.user(1, "avatarer").await;
    h.user(2, "someone").await;
    let sha = "a".repeat(64);
    for (id, owner) in [(501, 1), (502, 1), (503, 2)] {
        h.sql(&format!(
            "INSERT INTO media_files (id, object_key, mime, size_bytes, sha256, receipt)
             VALUES ({id}, 'k/{id}.png', 'image/png', 100, '{sha}', 'r{id}');
             INSERT INTO media_uploaders (media_id, user_id) VALUES ({id}, {owner});"
        ))
        .await;
    }
    let set = |id: i64| {
        let state = h.state.clone();
        let me = me.clone();
        async move {
            service::set_avatar(
                &state,
                &me,
                ik_profile::dto::SetAvatarRequest {
                    media_id: Id::new(id),
                },
            )
            .await
        }
    };

    let first = set(501).await.unwrap();
    assert_eq!(first.balance, 20);
    assert_eq!(first.avatar.unwrap().id.get(), 501);
    assert_eq!(set(501).await.unwrap().balance, 20, "同一张不扣费");
    assert_eq!(set(503).await.unwrap_err().code(), "NOT_FOUND", "别人的图");
    assert_eq!(set(999).await.unwrap_err().code(), "NOT_FOUND");
    assert_eq!(h.balance(1).await, 20);

    assert_eq!(set(502).await.unwrap().balance, 10);
    assert_eq!(set(501).await.unwrap().balance, 0);
    assert_eq!(set(502).await.unwrap_err().code(), "INSUFFICIENT_DENNY");
    // 余额不足整体回滚：头像还是 501。
    let mine = service::my_profile(&h.state, &me).await.unwrap();
    assert_eq!(mine.avatar.unwrap().id.get(), 501);
    let public = service::profile(&h.state, None, 1).await.unwrap();
    assert_eq!(public.avatar.unwrap().id.get(), 501);

    // 用户搜索结果带头像。
    let found = service::search(&h.state, None, IP, search("avatarer"))
        .await
        .unwrap();
    assert!(
        found[0]
            .avatar_url
            .as_deref()
            .unwrap()
            .starts_with("https://cdn.test/k/501.png")
    );

    let cleared = service::clear_avatar(&h.state, &me).await.unwrap();
    assert!(cleared.avatar.is_none());
    assert_eq!(cleared.balance, 0);
    assert!(
        service::my_profile(&h.state, &me)
            .await
            .unwrap()
            .avatar
            .is_none()
    );
}
