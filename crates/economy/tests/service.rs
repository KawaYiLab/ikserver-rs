//! 经济 service 层集成测试：真实 PostgreSQL，每个测试一个独立临时库。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use chrono::{Duration, Utc};
use ik_auth::Principal;
use ik_economy::EconomyState;
use ik_economy::service::{self, Dedup, DennyAction, ExpAction};
use ik_kernel::{Id, Role, Snowflake, time};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

fn state(db: PgPool) -> EconomyState {
    EconomyState::new(db, Arc::new(Snowflake::new(1).unwrap()))
}

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

async fn economy(db: &PgPool, id: i64) -> (i64, i32, i32) {
    let row = sqlx::query!(
        "SELECT exp, denny_balance, denny_given FROM user_economy WHERE user_id = $1",
        id
    )
    .fetch_one(db)
    .await
    .unwrap();
    (row.exp, row.denny_balance, row.denny_given)
}

async fn level(db: &PgPool, id: i64) -> i16 {
    sqlx::query_scalar!("SELECT level FROM users WHERE id = $1", id)
        .fetch_one(db)
        .await
        .unwrap()
}

async fn grant(
    st: &EconomyState,
    id: i64,
    action: ExpAction,
    amount: i32,
    dedup: Dedup,
) -> service::ExpOutcome {
    let mut tx = st.db.begin().await.unwrap();
    let out = service::grant_exp(&mut tx, &st.ids, id, action, amount, dedup, Utc::now())
        .await
        .unwrap()
        .expect("账号可用");
    tx.commit().await.unwrap();
    out
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn daily_dedup_grants_once_per_day(db: PgPool) {
    let st = state(db.clone());
    user(&db, 1, true).await;
    let first = grant(&st, 1, ExpAction::CreatePost, 4, Dedup::Daily).await;
    assert_eq!(first.added, 4);
    let second = grant(&st, 1, ExpAction::CreatePost, 4, Dedup::Daily).await;
    assert_eq!(second.added, 0);
    assert!(second.duplicate);
    assert_eq!(economy(&db, 1).await.0, 4);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn active_bucket_is_capped_at_50(db: PgPool) {
    let st = state(db.clone());
    user(&db, 1, true).await;
    for i in 0..4 {
        let out = grant(
            &st,
            1,
            ExpAction::CommentAccepted,
            10,
            Dedup::Ref(format!("comment:{i}")),
        )
        .await;
        assert_eq!(out.added, 10);
    }
    // 第 5 条只剩 10，第 6 条一点都不剩。
    let fifth = grant(&st, 1, ExpAction::LikeGive, 1, Dedup::Daily).await;
    assert_eq!(fifth.added, 1);
    let sixth = grant(
        &st,
        1,
        ExpAction::CommentAccepted,
        10,
        Dedup::Ref("comment:5".into()),
    )
    .await;
    assert_eq!(sixth.added, 9);
    let seventh = grant(&st, 1, ExpAction::CreatePost, 4, Dedup::Daily).await;
    assert_eq!(seventh.added, 0);
    assert!(!seventh.duplicate);
    // 被上限挡住的那次不写流水，不占「今日首次」名额。
    let logs: i64 = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM exp_logs WHERE user_id = 1 AND action = 'create_post'"#
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(logs, 0);
    assert_eq!(economy(&db, 1).await.0, 50);

    // 被动桶与考试奖励不受主动桶影响。
    let passive = grant(
        &st,
        1,
        ExpAction::DennyReceived,
        1,
        Dedup::Ref("gift:2:post:1".into()),
    )
    .await;
    assert_eq!(passive.added, 1);
    let exam = grant(
        &st,
        1,
        ExpAction::ExamPassed,
        50,
        Dedup::Ref("user:1".into()),
    )
    .await;
    assert_eq!(exam.added, 50);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn level_is_zero_until_exam_passed(db: PgPool) {
    let st = state(db.clone());
    user(&db, 1, false).await;
    user(&db, 2, true).await;
    for id in [1, 2] {
        let mut tx = st.db.begin().await.unwrap();
        service::grant_exp(
            &mut tx,
            &st.ids,
            id,
            ExpAction::ExamPassed,
            600,
            Dedup::Ref(format!("user:{id}")),
            Utc::now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(level(&db, 1).await, 0, "未通过考试：等级锁 0，经验照常累积");
    assert_eq!(economy(&db, 1).await.0, 600);
    assert_eq!(level(&db, 2).await, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn add_denny_is_idempotent_and_capped(db: PgPool) {
    let st = state(db.clone());
    user(&db, 1, true).await;
    let mut tx = st.db.begin().await.unwrap();
    let first = service::add_denny(
        &mut tx,
        &st.ids,
        1,
        DennyAction::ExamPassed,
        50,
        Some("user:1"),
        "通过入站考试奖励",
        Utc::now(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!((first.delta, first.balance), (50, 80));
    let again = service::add_denny(
        &mut tx,
        &st.ids,
        1,
        DennyAction::ExamPassed,
        50,
        Some("user:1"),
        "通过入站考试奖励",
        Utc::now(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(again.duplicate);
    assert_eq!(again.balance, 80);
    let big = service::add_denny(
        &mut tx,
        &st.ids,
        1,
        DennyAction::AdminGrant,
        100_000,
        None,
        "调账",
        Utc::now(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!((big.delta, big.balance), (9999 - 80, 9999));
    let none = service::add_denny(
        &mut tx,
        &st.ids,
        1,
        DennyAction::AdminGrant,
        5,
        None,
        "调账",
        Utc::now(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(none.delta, 0, "已封顶：不入账也不写流水");
    tx.commit().await.unwrap();
    let logs: i64 =
        sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM denny_logs WHERE user_id = 1"#)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(logs, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn spend_rejects_insufficient_balance(db: PgPool) {
    let st = state(db.clone());
    user(&db, 1, true).await;
    let mut tx = st.db.begin().await.unwrap();
    let err = service::spend_denny(
        &mut tx,
        &st.ids,
        1,
        DennyAction::SpendRename,
        31,
        "改名",
        Utc::now(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "INSUFFICIENT_DENNY");
    let ok = service::spend_denny(
        &mut tx,
        &st.ids,
        1,
        DennyAction::SpendRename,
        30,
        "改名",
        Utc::now(),
    )
    .await
    .unwrap();
    assert_eq!((ok.delta, ok.balance), (-30, 0));
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn check_in_once_per_day_with_rewards(db: PgPool) {
    let st = state(db.clone());
    let alice = user(&db, 1, true).await;
    let bob = user(&db, 2, true).await;

    let status = service::check_in_status(&st, &alice).await.unwrap();
    assert!(status.can_check_in);
    assert_eq!((status.next_streak, status.next_exp), (1, 6));

    let first = service::check_in(&st, &alice).await.unwrap();
    assert_eq!(first.streak, 1);
    assert_eq!(first.rank, 1);
    assert_eq!(first.exp_gained, 6);
    assert_eq!(first.denny_gained, 10);
    assert_eq!(first.balance, 40);
    assert_eq!(first.total_days, 1);

    let err = service::check_in(&st, &alice).await.unwrap_err();
    assert_eq!(err.code(), "CHECK_IN_ALREADY_TODAY");

    let second = service::check_in(&st, &bob).await.unwrap();
    assert_eq!(second.rank, 2);

    let status = service::check_in_status(&st, &alice).await.unwrap();
    assert!(!status.can_check_in);
    assert_eq!(status.rank, Some(1));
    assert!(status.next_eligible_at.is_some());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn check_in_streak_continues_from_yesterday(db: PgPool) {
    let st = state(db.clone());
    let alice = user(&db, 1, true).await;
    let yesterday = time::biz_day(Utc::now() - Duration::days(1));
    sqlx::query!(
        "INSERT INTO user_economy (user_id, last_check_in_day, check_in_streak, check_in_total)
         VALUES (1, $1, 4, 10)",
        yesterday,
    )
    .execute(&db)
    .await
    .unwrap();
    let result = service::check_in(&st, &alice).await.unwrap();
    assert_eq!(result.streak, 5);
    assert_eq!(result.exp_gained, 10);
    assert_eq!(result.total_days, 11);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn check_in_requires_exam(db: PgPool) {
    let st = state(db.clone());
    let newbie = user(&db, 1, false).await;
    let err = service::check_in(&st, &newbie).await.unwrap_err();
    assert_eq!(err.code(), "EXAM_REQUIRED");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn blocked_account_check_in_is_403(db: PgPool) {
    let st = state(db.clone());
    let alice = user(&db, 1, true).await;
    sqlx::query!("UPDATE users SET status = 'blocked' WHERE id = 1")
        .execute(&db)
        .await
        .unwrap();
    let err = service::check_in(&st, &alice).await.unwrap_err();
    assert_eq!(err.code(), "ACCOUNT_BLOCKED");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_check_ins_only_one_succeeds(db: PgPool) {
    let st = state(db.clone());
    let alice = user(&db, 1, true).await;
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let st = st.clone();
            let alice = alice.clone();
            tokio::spawn(async move { service::check_in(&st, &alice).await })
        })
        .collect();
    let mut ok = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => ok += 1,
            Err(e) => assert_eq!(e.code(), "CHECK_IN_ALREADY_TODAY"),
        }
    }
    assert_eq!(ok, 1);
    let (exp, balance, _) = economy(&db, 1).await;
    assert_eq!((exp, balance), (6, 40));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_exp_grants_respect_cap(db: PgPool) {
    let st = state(db.clone());
    user(&db, 1, true).await;
    let tasks: Vec<_> = (0..10)
        .map(|i| {
            let st = st.clone();
            tokio::spawn(async move {
                grant(
                    &st,
                    1,
                    ExpAction::CommentAccepted,
                    10,
                    Dedup::Ref(format!("comment:{i}")),
                )
                .await
                .added
            })
        })
        .collect();
    let mut total = 0;
    for task in tasks {
        total += task.await.unwrap();
    }
    assert_eq!(total, 50);
    assert_eq!(economy(&db, 1).await.0, 50);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn denny_logs_paginate_newest_first(db: PgPool) {
    let st = state(db.clone());
    let alice = user(&db, 1, true).await;
    let mut tx = st.db.begin().await.unwrap();
    for i in 0..3 {
        service::add_denny(
            &mut tx,
            &st.ids,
            1,
            DennyAction::AdminGrant,
            1,
            Some(&format!("t{i}")),
            &format!("第 {i} 笔"),
            Utc::now() + Duration::seconds(i),
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let page = service::denny_logs(
        &st,
        &alice,
        ik_economy::dto::PageQuery {
            cursor: None,
            limit: Some(2),
        },
    )
    .await
    .unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].description, "第 2 笔");
    let next = service::denny_logs(
        &st,
        &alice,
        ik_economy::dto::PageQuery {
            cursor: page.next_cursor,
            limit: Some(2),
        },
    )
    .await
    .unwrap();
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0].description, "第 0 笔");
    assert!(next.next_cursor.is_none());
}

#[test]
fn benefits_for_anonymous_and_levels() {
    let anonymous = service::benefits_for(None);
    assert_eq!(anonymous.level, 0);
    assert_eq!(anonymous.benefits.post_max_body_chars, 0);
    assert_eq!(anonymous.next_level, Some(1));
}
