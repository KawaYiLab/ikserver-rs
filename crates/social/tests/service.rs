//! 关注 / 拉黑 service 层集成测试：真实 PostgreSQL，每个测试一个独立临时库。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ik_auth::Principal;
use ik_kernel::{Id, Role};
use ik_social::SocialState;
use ik_social::dto::PageQuery;
use ik_social::{interop, service};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

async fn user(db: &PgPool, id: i64, exam: bool) -> Principal {
    sqlx::query!(
        "INSERT INTO users (id, username, email, level, exam_passed_at)
         VALUES ($1, $2, $3, $4, CASE WHEN $5 THEN now() END)",
        id,
        format!("user{id}"),
        format!("user{id}@example.com"),
        if exam { 1i16 } else { 0 },
        exam,
    )
    .execute(db)
    .await
    .unwrap();
    Principal {
        user_id: Id::new(id),
        session_id: Id::new(id),
        role: Role::User,
        exam_passed: exam,
        is_ai_agent: false,
        level: if exam { 1 } else { 0 },
    }
}

fn code(err: &ik_kernel::Problem) -> &str {
    err.code()
}

fn first_page() -> PageQuery {
    PageQuery {
        cursor: None,
        limit: None,
    }
}

async fn follows(db: &PgPool, follower: i64, followee: i64) -> bool {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM follows WHERE follower_id = $1 AND followee_id = $2) AS "e!""#,
        follower,
        followee,
    )
    .fetch_one(db)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn follow_and_unfollow_are_idempotent(db: PgPool) {
    let st = SocialState::new(db.clone());
    let a = user(&db, 1, true).await;
    let b = user(&db, 2, true).await;
    user(&db, 3, true).await;

    let first = service::follow(&st, &a, 3).await.unwrap();
    assert!(first.following);
    assert_eq!(first.follower_count, 1);
    let again = service::follow(&st, &a, 3).await.unwrap();
    assert_eq!(again.follower_count, 1);
    assert_eq!(service::follow(&st, &b, 3).await.unwrap().follower_count, 2);

    let off = service::unfollow(&st, &a, 3).await.unwrap();
    assert!(!off.following);
    assert_eq!(off.follower_count, 1);
    assert_eq!(
        service::unfollow(&st, &a, 3).await.unwrap().follower_count,
        1
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn follow_rules(db: PgPool) {
    let st = SocialState::new(db.clone());
    let newbie = user(&db, 1, false).await;
    let me = user(&db, 2, true).await;
    user(&db, 3, true).await;
    sqlx::query!("UPDATE users SET status = 'deleted' WHERE id = 3")
        .execute(&db)
        .await
        .unwrap();

    assert_eq!(
        code(&service::follow(&st, &newbie, 2).await.unwrap_err()),
        "EXAM_REQUIRED"
    );
    assert_eq!(
        code(&service::follow(&st, &me, 2).await.unwrap_err()),
        "FOLLOW_SELF"
    );
    assert_eq!(
        code(&service::follow(&st, &me, 3).await.unwrap_err()),
        "NOT_FOUND"
    );
    assert_eq!(
        code(&service::follow(&st, &me, 999).await.unwrap_err()),
        "NOT_FOUND"
    );
    // 取消关注不要求通过考试。
    assert!(!service::unfollow(&st, &newbie, 2).await.unwrap().following);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn block_removes_follows_both_ways_and_blocks_new_follows(db: PgPool) {
    let st = SocialState::new(db.clone());
    let a = user(&db, 1, true).await;
    let b = user(&db, 2, true).await;
    service::follow(&st, &a, 2).await.unwrap();
    service::follow(&st, &b, 1).await.unwrap();

    assert!(service::block(&st, &a, 2).await.unwrap().blocked);
    assert!(!follows(&db, 1, 2).await);
    assert!(!follows(&db, 2, 1).await);

    // 任一方向的拉黑都拦截新的关注。
    assert_eq!(
        code(&service::follow(&st, &a, 2).await.unwrap_err()),
        "USER_BLOCKED"
    );
    assert_eq!(
        code(&service::follow(&st, &b, 1).await.unwrap_err()),
        "USER_BLOCKED"
    );

    // 取消拉黑不恢复关注；之后可以重新关注。
    assert!(!service::unblock(&st, &a, 2).await.unwrap().blocked);
    assert!(!follows(&db, 1, 2).await);
    assert!(service::follow(&st, &a, 2).await.unwrap().following);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn block_rules(db: PgPool) {
    let st = SocialState::new(db.clone());
    // 拉黑不要求通过考试。
    let newbie = user(&db, 1, false).await;
    user(&db, 2, true).await;
    sqlx::query!("UPDATE users SET is_ai_agent = TRUE WHERE id = 2")
        .execute(&db)
        .await
        .unwrap();
    user(&db, 3, true).await;

    assert_eq!(
        code(&service::block(&st, &newbie, 1).await.unwrap_err()),
        "BLOCK_SELF"
    );
    assert_eq!(
        code(&service::block(&st, &newbie, 2).await.unwrap_err()),
        "BLOCK_SYSTEM_ACCOUNT"
    );
    assert_eq!(
        code(&service::block(&st, &newbie, 999).await.unwrap_err()),
        "NOT_FOUND"
    );
    assert!(service::block(&st, &newbie, 3).await.unwrap().blocked);
    assert!(service::block(&st, &newbie, 3).await.unwrap().blocked);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn block_is_rate_limited_per_user(db: PgPool) {
    let st = SocialState::new(db.clone());
    let me = user(&db, 1, true).await;
    let other = user(&db, 2, true).await;
    user(&db, 3, true).await;
    // 限流在参数校验之前计数（BLOCK-006），非法请求同样消耗额度。用拉黑自己来耗额度：
    // 不访问数据库，10 次请求远小于 6 秒的回填间隔，测试不受远程库延迟影响。
    for _ in 0..10 {
        let err = service::block(&st, &me, 1).await.unwrap_err();
        assert_eq!(code(&err), "BLOCK_SELF");
    }
    let err = service::block(&st, &me, 3).await.unwrap_err();
    assert_eq!(code(&err), "RATE_LIMITED");
    // 每个用户独立计数。
    service::block(&st, &other, 3).await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn relations_only_expose_my_side(db: PgPool) {
    let st = SocialState::new(db.clone());
    let a = user(&db, 1, true).await;
    let b = user(&db, 2, true).await;
    user(&db, 3, true).await;
    service::follow(&st, &a, 3).await.unwrap();
    service::block(&st, &b, 1).await.unwrap();

    let views = service::relations(&st, &a, "3, 2,3").await.unwrap();
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].user_id.get(), 3);
    assert!(views[0].following && !views[0].blocking);
    // b 拉黑了 a，但 a 看不到这一点（D-053）。
    assert_eq!(views[1].user_id.get(), 2);
    assert!(!views[1].following && !views[1].blocking);

    let views = service::relations(&st, &b, "1").await.unwrap();
    assert!(views[0].blocking);
    assert!(service::relations(&st, &a, " ").await.unwrap().is_empty());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn interop_sees_both_directions(db: PgPool) {
    let st = SocialState::new(db.clone());
    let a = user(&db, 1, true).await;
    user(&db, 2, true).await;
    user(&db, 3, true).await;
    service::block(&st, &a, 2).await.unwrap();

    let mut conn = db.acquire().await.unwrap();
    assert!(interop::any_block(&mut conn, 1, 2).await.unwrap());
    assert!(interop::any_block(&mut conn, 2, 1).await.unwrap());
    assert!(!interop::any_block(&mut conn, 1, 3).await.unwrap());
    assert_eq!(
        interop::ensure_no_block(&mut conn, 2, 1)
            .await
            .unwrap_err()
            .code(),
        "USER_BLOCKED"
    );
    assert_eq!(
        interop::excluded_authors(&mut conn, Some(2)).await.unwrap(),
        vec![1]
    );
    assert!(
        interop::excluded_authors(&mut conn, None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn following_list_pages_and_skips_deleted_accounts(db: PgPool) {
    let st = SocialState::new(db.clone());
    let me = user(&db, 1, true).await;
    for id in 2..=6 {
        user(&db, id, true).await;
        service::follow(&st, &me, id).await.unwrap();
    }
    sqlx::query!("UPDATE users SET status = 'deleted' WHERE id = 4")
        .execute(&db)
        .await
        .unwrap();

    let page = service::following(
        &st,
        &me,
        PageQuery {
            cursor: None,
            limit: Some(2),
        },
    )
    .await
    .unwrap();
    let ids: Vec<i64> = page.items.iter().map(|u| u.id.get()).collect();
    assert_eq!(ids, vec![6, 5]);
    let page = service::following(
        &st,
        &me,
        PageQuery {
            cursor: page.next_cursor,
            limit: Some(2),
        },
    )
    .await
    .unwrap();
    // 第二页原始行是 4、3；4 已注销，不展示，但游标照常推进。
    let ids: Vec<i64> = page.items.iter().map(|u| u.id.get()).collect();
    assert_eq!(ids, vec![3]);
    let page = service::following(
        &st,
        &me,
        PageQuery {
            cursor: page.next_cursor,
            limit: Some(2),
        },
    )
    .await
    .unwrap();
    let ids: Vec<i64> = page.items.iter().map(|u| u.id.get()).collect();
    assert_eq!(ids, vec![2]);
    assert!(page.next_cursor.is_none());

    service::block(&st, &me, 2).await.unwrap();
    let blocked = service::blocks(&st, &me, first_page()).await.unwrap();
    assert_eq!(blocked.items.len(), 1);
    assert_eq!(blocked.items[0].username, "user2");
}

/// 「A 关注 B」与「B 拉黑 A」并发：无论谁先提交，最后都不能留下拉黑双方之间的关注。
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_follow_and_block_never_leave_a_follow(db: PgPool) {
    let st = SocialState::new(db.clone());
    let a = user(&db, 1, true).await;
    let b = user(&db, 2, true).await;
    for _ in 0..4 {
        let (followed, blocked) =
            tokio::join!(service::follow(&st, &a, 2), service::block(&st, &b, 1),);
        blocked.unwrap();
        if let Err(err) = followed {
            assert_eq!(err.code(), "USER_BLOCKED");
        }
        assert!(!follows(&db, 1, 2).await);
        service::unblock(&st, &b, 1).await.unwrap();
    }
}
