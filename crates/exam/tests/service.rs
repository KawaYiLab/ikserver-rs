//! 入站考试 service 层集成测试：真实 PostgreSQL，每个测试一个独立临时库。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use ik_auth::Principal;
use ik_auth::service::RequestMeta;
use ik_exam::ExamState;
use ik_exam::dto::{
    ExamOption, QuestionInput, QuestionKind, ReviewQuery, StartView, SubmitRequest,
};
use ik_exam::service;
use ik_kernel::config::Exam;
use ik_kernel::{Id, Role, Snowflake};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

fn state(db: PgPool, question_count: u32) -> ExamState {
    ExamState::new(
        db,
        Arc::new(Snowflake::new(1).unwrap()),
        Exam {
            question_count,
            ..Exam::default()
        },
    )
}

fn meta() -> RequestMeta {
    RequestMeta {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        user_agent: None,
    }
}

async fn newbie(db: &PgPool, id: i64) -> Principal {
    sqlx::query!(
        "INSERT INTO users (id, username, email) VALUES ($1, $2, $3)",
        id,
        format!("user{id}"),
        format!("user{id}@example.com"),
    )
    .execute(db)
    .await
    .unwrap();
    Principal {
        user_id: Id::new(id),
        session_id: Id::new(id),
        role: Role::User,
        exam_passed: false,
        is_ai_agent: false,
        level: 0,
    }
}

fn single(text: &str, correct: &str) -> QuestionInput {
    QuestionInput {
        question: text.into(),
        kind: QuestionKind::Single,
        options: ["A", "B", "C"]
            .iter()
            .map(|k| ExamOption {
                key: (*k).into(),
                text: format!("选项{k}"),
            })
            .collect(),
        correct_keys: vec![correct.into()],
        explanation: Some(format!("{text} 的解析")),
        weight: None,
        enabled: None,
        tag: None,
    }
}

async fn seed(st: &ExamState, n: usize) {
    for i in 0..n {
        service::create_question(st, single(&format!("第 {i} 题"), "A"))
            .await
            .unwrap();
    }
}

/// 每题都选 `key`。
fn answer_all(paper: &StartView, key: &str) -> SubmitRequest {
    SubmitRequest {
        answers: paper
            .questions
            .iter()
            .map(|q| (q.id.to_string(), vec![key.to_owned()]))
            .collect(),
    }
}

async fn pass(st: &ExamState, who: &Principal) {
    let paper = service::start(st, who, &meta()).await.unwrap();
    let result = service::submit(st, who, paper.attempt_id.get(), answer_all(&paper, "A"))
        .await
        .unwrap();
    assert!(result.passed);
}

async fn fail(st: &ExamState, who: &Principal) {
    let paper = service::start(st, who, &meta()).await.unwrap();
    let result = service::submit(st, who, paper.attempt_id.get(), answer_all(&paper, "B"))
        .await
        .unwrap();
    assert!(!result.passed);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn start_draws_and_resumes_without_leaking_answers(db: PgPool) {
    let st = state(db.clone(), 3);
    seed(&st, 5).await;
    let me = newbie(&db, 1).await;

    let first = service::start(&st, &me, &meta()).await.unwrap();
    assert!(!first.resumed);
    assert_eq!(first.questions.len(), 3, "题库 5 题，每场抽 3 题");
    let json = serde_json::to_value(&first).unwrap();
    assert!(!json.to_string().contains("correct"), "不下发正确答案");

    let again = service::start(&st, &me, &meta()).await.unwrap();
    assert!(again.resumed);
    assert_eq!(again.attempt_id, first.attempt_id);
    let order = |v: &StartView| -> Vec<(i64, Vec<String>)> {
        v.questions
            .iter()
            .map(|q| {
                (
                    q.id.get(),
                    q.options.iter().map(|o| o.key.clone()).collect(),
                )
            })
            .collect()
    };
    assert_eq!(order(&again), order(&first), "恢复时题目与选项顺序不变");

    let status = service::status(&st, &me).await.unwrap();
    assert!(!status.passed);
    assert_eq!(status.active_attempt.map(|a| a.id), Some(first.attempt_id));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn passing_grants_rewards_and_unlocks_level(db: PgPool) {
    let st = state(db.clone(), 100);
    seed(&st, 4).await;
    let me = newbie(&db, 1).await;
    let paper = service::start(&st, &me, &meta()).await.unwrap();
    let result = service::submit(&st, &me, paper.attempt_id.get(), answer_all(&paper, "A"))
        .await
        .unwrap();
    assert!(result.passed);
    assert_eq!(
        (result.score, result.total_score, result.score_percent),
        (4, 4, 100)
    );
    let reward = result.reward.unwrap();
    assert_eq!((reward.denny, reward.exp), (50, 50));

    let user = sqlx::query!("SELECT exam_passed_at, level FROM users WHERE id = 1")
        .fetch_one(&db)
        .await
        .unwrap();
    assert!(user.exam_passed_at.is_some());
    assert_eq!(user.level, 1);
    let econ = sqlx::query!("SELECT exp, denny_balance FROM user_economy WHERE user_id = 1")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!((econ.exp, econ.denny_balance), (50, 80));

    let err = service::start(&st, &me, &meta()).await.unwrap_err();
    assert_eq!(err.code(), "EXAM_ALREADY_PASSED");
    let status = service::status(&st, &me).await.unwrap();
    assert!(status.passed && status.passed_at.is_some());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn grading_uses_pass_line_and_weights(db: PgPool) {
    let st = state(db.clone(), 100);
    // 5 题各 1 分：答对 3 题 = 60%，刚好及格。
    seed(&st, 5).await;
    let me = newbie(&db, 1).await;
    let paper = service::start(&st, &me, &meta()).await.unwrap();
    let mut answers = BTreeMap::new();
    for (i, q) in paper.questions.iter().enumerate() {
        answers.insert(
            q.id.to_string(),
            vec![if i < 3 { "A" } else { "B" }.to_owned()],
        );
    }
    let result = service::submit(&st, &me, paper.attempt_id.get(), SubmitRequest { answers })
        .await
        .unwrap();
    assert_eq!((result.correct_count, result.score_percent), (3, 60));
    assert!(result.passed);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn three_failures_trigger_cooldown(db: PgPool) {
    let st = state(db.clone(), 100);
    seed(&st, 2).await;
    let me = newbie(&db, 1).await;
    fail(&st, &me).await;
    fail(&st, &me).await;
    let paper = service::start(&st, &me, &meta()).await.unwrap();
    let result = service::submit(&st, &me, paper.attempt_id.get(), answer_all(&paper, "B"))
        .await
        .unwrap();
    assert!(result.cooldown_remaining_secs > 1700);

    let err = service::start(&st, &me, &meta()).await.unwrap_err();
    assert_eq!(err.code(), "EXAM_COOLDOWN");
    assert!(err.retry_after_secs().unwrap() > 1700);
    let status = service::status(&st, &me).await.unwrap();
    assert!(status.cooldown_remaining_secs > 1700);

    // 冷却过后可以再考。
    sqlx::query!("UPDATE exam_attempts SET submitted_at = submitted_at - interval '31 minutes'")
        .execute(&db)
        .await
        .unwrap();
    pass(&st, &me).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expired_attempts_count_as_failures(db: PgPool) {
    let st = state(db.clone(), 100);
    seed(&st, 2).await;
    let me = newbie(&db, 1).await;

    // 超时交卷：记一次失败并返回 409，答对也不算。
    let paper = service::start(&st, &me, &meta()).await.unwrap();
    sqlx::query!(
        "UPDATE exam_attempts SET started_at = now() - interval '2 hours',
                                  expires_at = now() - interval '1 hour'"
    )
    .execute(&db)
    .await
    .unwrap();
    let err = service::submit(&st, &me, paper.attempt_id.get(), answer_all(&paper, "A"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "EXAM_ATTEMPT_EXPIRED");
    let passed: Option<bool> = sqlx::query_scalar!(
        "SELECT passed FROM exam_attempts WHERE id = $1",
        paper.attempt_id.get()
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(passed, Some(false));

    // 开考后放着不交、等它过期：下次开考时按失败收尾（D-060），不能无限刷题看题。
    for _ in 0..2 {
        service::start(&st, &me, &meta()).await.unwrap();
        sqlx::query!(
            "UPDATE exam_attempts SET started_at = now() - interval '2 hours',
                                      expires_at = now() - interval '1 second'
             WHERE submitted_at IS NULL"
        )
        .execute(&db)
        .await
        .unwrap();
    }
    let err = service::start(&st, &me, &meta()).await.unwrap_err();
    assert_eq!(err.code(), "EXAM_COOLDOWN");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn submit_rejects_double_foreign_and_bad_answers(db: PgPool) {
    let st = state(db.clone(), 100);
    seed(&st, 2).await;
    let me = newbie(&db, 1).await;
    let other = newbie(&db, 2).await;
    let paper = service::start(&st, &me, &meta()).await.unwrap();
    let id = paper.attempt_id.get();

    let err = service::submit(&st, &other, id, answer_all(&paper, "A"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "NOT_FOUND", "别人的场次");

    let bogus = SubmitRequest {
        answers: [("123".to_owned(), vec!["A".to_owned()])].into(),
    };
    let err = service::submit(&st, &me, id, bogus).await.unwrap_err();
    assert_eq!(err.code(), "VALIDATION_FAILED");

    service::submit(&st, &me, id, answer_all(&paper, "B"))
        .await
        .unwrap();
    let err = service::submit(&st, &me, id, answer_all(&paper, "A"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "EXAM_ATTEMPT_ALREADY_SUBMITTED");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_submits_reward_once(db: PgPool) {
    let st = state(db.clone(), 100);
    seed(&st, 2).await;
    let me = newbie(&db, 1).await;
    let paper = service::start(&st, &me, &meta()).await.unwrap();
    let tasks: Vec<_> = (0..5)
        .map(|_| {
            let st = st.clone();
            let me = me.clone();
            let request = answer_all(&paper, "A");
            let id = paper.attempt_id.get();
            tokio::spawn(async move { service::submit(&st, &me, id, request).await })
        })
        .collect();
    let mut ok = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(result) => {
                assert!(result.reward.is_some());
                ok += 1;
            }
            Err(e) => assert_eq!(e.code(), "EXAM_ATTEMPT_ALREADY_SUBMITTED"),
        }
    }
    assert_eq!(ok, 1);
    let balance: i32 =
        sqlx::query_scalar!("SELECT denny_balance FROM user_economy WHERE user_id = 1")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(balance, 80);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_starts_create_one_attempt(db: PgPool) {
    let st = state(db.clone(), 100);
    seed(&st, 2).await;
    let me = newbie(&db, 1).await;
    let tasks: Vec<_> = (0..5)
        .map(|_| {
            let st = st.clone();
            let me = me.clone();
            tokio::spawn(async move { service::start(&st, &me, &meta()).await })
        })
        .collect();
    let mut ids = Vec::new();
    for task in tasks {
        ids.push(task.await.unwrap().unwrap().attempt_id);
    }
    ids.dedup();
    assert_eq!(ids.len(), 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn empty_pool_is_unavailable(db: PgPool) {
    let st = state(db.clone(), 100);
    let me = newbie(&db, 1).await;
    let err = service::start(&st, &me, &meta()).await.unwrap_err();
    assert_eq!(err.code(), "EXAM_NOT_AVAILABLE");
    // 停用的题不进题池。
    let mut disabled = single("停用", "A");
    disabled.enabled = Some(false);
    service::create_question(&st, disabled).await.unwrap();
    let err = service::start(&st, &me, &meta()).await.unwrap_err();
    assert_eq!(err.code(), "EXAM_NOT_AVAILABLE");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn review_shows_answers_and_explanations(db: PgPool) {
    let st = state(db.clone(), 100);
    seed(&st, 2).await;
    let me = newbie(&db, 1).await;
    let err = service::review(&st, &me, ReviewQuery { attempt_id: None })
        .await
        .unwrap_err();
    assert_eq!(err.code(), "NOT_FOUND");

    let paper = service::start(&st, &me, &meta()).await.unwrap();
    let first = &paper.questions[0];
    let request = SubmitRequest {
        answers: [(first.id.to_string(), vec!["A".to_owned()])].into(),
    };
    service::submit(&st, &me, paper.attempt_id.get(), request)
        .await
        .unwrap();
    let review = service::review(&st, &me, ReviewQuery { attempt_id: None })
        .await
        .unwrap();
    assert_eq!(review.attempt_id, paper.attempt_id);
    assert_eq!((review.correct_count, review.question_count), (1, 2));
    let q = review.questions.iter().find(|q| q.id == first.id).unwrap();
    assert!(q.is_correct);
    assert_eq!(q.user_answer, vec!["A".to_owned()]);
    assert!(q.explanation.as_deref().unwrap().ends_with("的解析"));
    assert!(
        !serde_json::to_string(&review)
            .unwrap()
            .contains("correctKeys")
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn question_validation(db: PgPool) {
    let st = state(db, 100);
    let mut empty = single("  ", "A");
    empty.question = "  ".into();
    assert_eq!(
        service::create_question(&st, empty)
            .await
            .unwrap_err()
            .field_errors()[0]
            .field,
        "question"
    );
    let not_subset = single("题", "Z");
    assert_eq!(
        service::create_question(&st, not_subset)
            .await
            .unwrap_err()
            .field_errors()[0]
            .field,
        "correctKeys"
    );
    let mut multiple_one = single("题", "A");
    multiple_one.kind = QuestionKind::Multiple;
    assert_eq!(
        service::create_question(&st, multiple_one)
            .await
            .unwrap_err()
            .field_errors()[0]
            .field,
        "kind"
    );
    let mut boolean_three = single("题", "A");
    boolean_three.kind = QuestionKind::Boolean;
    assert!(service::create_question(&st, boolean_three).await.is_err());
    let mut zero_weight = single("题", "A");
    zero_weight.weight = Some(0);
    let id = service::create_question(&st, zero_weight).await.unwrap();
    let weight: i32 = sqlx::query_scalar!("SELECT weight FROM exam_questions WHERE id = $1", id)
        .fetch_one(&st.db)
        .await
        .unwrap();
    assert_eq!(weight, 1, "非法权重按 1");
}
