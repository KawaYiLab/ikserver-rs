//! SQL。只被本 crate 的 service 调用。

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

// ---------- 题库 ----------

pub(crate) struct QuestionRow {
    pub id: i64,
    pub question: String,
    pub kind: String,
    pub options: serde_json::Value,
    pub correct_keys: Vec<String>,
    pub weight: i32,
}

pub(crate) async fn enabled_questions(conn: &mut PgConnection) -> sqlx::Result<Vec<QuestionRow>> {
    sqlx::query_as!(
        QuestionRow,
        "SELECT id, question, kind, options, correct_keys, weight
         FROM exam_questions WHERE enabled ORDER BY id",
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct NewQuestion<'a> {
    pub id: i64,
    pub question: &'a str,
    pub kind: &'a str,
    pub options: serde_json::Value,
    pub correct_keys: &'a [String],
    pub explanation: Option<&'a str>,
    pub weight: i32,
    pub enabled: bool,
    pub tag: Option<&'a str>,
    pub now: DateTime<Utc>,
}

pub(crate) async fn insert_question(
    conn: &mut PgConnection,
    q: &NewQuestion<'_>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO exam_questions (id, question, kind, options, correct_keys, explanation, weight,
                                     enabled, tag, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)",
        q.id,
        q.question,
        q.kind,
        q.options,
        q.correct_keys,
        q.explanation,
        q.weight,
        q.enabled,
        q.tag,
        q.now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct ExplanationRow {
    pub id: i64,
    pub explanation: Option<String>,
}

pub(crate) async fn explanations(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<ExplanationRow>> {
    sqlx::query_as!(
        ExplanationRow,
        "SELECT id, explanation FROM exam_questions WHERE id = ANY($1)",
        ids,
    )
    .fetch_all(conn)
    .await
}

// ---------- 场次 ----------

pub(crate) struct AttemptRow {
    pub id: i64,
    pub questions: serde_json::Value,
    pub answers: Option<serde_json::Value>,
    pub passed: Option<bool>,
    pub score: Option<i32>,
    pub total_score: Option<i32>,
    pub score_percent: Option<i16>,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub submitted_at: Option<DateTime<Utc>>,
}

/// 过期未交卷的场次按失败收尾，交卷时间记为过期时间（D-060 方案 A）。
pub(crate) async fn close_expired(
    conn: &mut PgConnection,
    user_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        "UPDATE exam_attempts a
         SET submitted_at = a.expires_at, passed = FALSE, score = 0, score_percent = 0,
             total_score = (SELECT COALESCE(SUM((q ->> 'weight')::int), 0)
                            FROM jsonb_array_elements(a.questions) q)
         WHERE a.user_id = $1 AND a.submitted_at IS NULL AND a.expires_at <= $2",
        user_id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

pub(crate) struct ResultRow {
    pub passed: bool,
    pub at: DateTime<Utc>,
}

/// 最近 `limit` 次考试结果（新的在前）。过期未交卷的场次即使还没收尾也按失败计（D-060 方案 A）。
pub(crate) async fn recent_results(
    conn: &mut PgConnection,
    user_id: i64,
    now: DateTime<Utc>,
    limit: i64,
) -> sqlx::Result<Vec<ResultRow>> {
    sqlx::query_as!(
        ResultRow,
        r#"SELECT passed AS "passed!", at AS "at!" FROM (
               SELECT passed, submitted_at AS at FROM exam_attempts
               WHERE user_id = $1 AND submitted_at IS NOT NULL
               UNION ALL
               SELECT FALSE, expires_at FROM exam_attempts
               WHERE user_id = $1 AND submitted_at IS NULL AND expires_at <= $2
           ) r
           ORDER BY at DESC
           LIMIT $3"#,
        user_id,
        now,
        limit,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn open_attempt(
    conn: &mut PgConnection,
    user_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<Option<AttemptRow>> {
    sqlx::query_as!(
        AttemptRow,
        "SELECT id, questions, answers, passed, score, total_score, score_percent, started_at,
                expires_at, submitted_at
         FROM exam_attempts
         WHERE user_id = $1 AND submitted_at IS NULL AND expires_at > $2",
        user_id,
        now,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct NewAttempt<'a> {
    pub id: i64,
    pub user_id: i64,
    pub questions: serde_json::Value,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub request_ip: &'a str,
}

pub(crate) async fn insert_attempt(
    conn: &mut PgConnection,
    a: &NewAttempt<'_>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO exam_attempts (id, user_id, questions, started_at, expires_at, request_ip)
         VALUES ($1, $2, $3, $4, $5, $6)",
        a.id,
        a.user_id,
        a.questions,
        a.started_at,
        a.expires_at,
        a.request_ip,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 锁住本人的某场考试。
pub(crate) async fn lock_attempt(
    conn: &mut PgConnection,
    id: i64,
    user_id: i64,
) -> sqlx::Result<Option<AttemptRow>> {
    sqlx::query_as!(
        AttemptRow,
        "SELECT id, questions, answers, passed, score, total_score, score_percent, started_at,
                expires_at, submitted_at
         FROM exam_attempts WHERE id = $1 AND user_id = $2
         FOR UPDATE",
        id,
        user_id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct Finish {
    pub answers: serde_json::Value,
    pub score: i32,
    pub total_score: i32,
    pub score_percent: i16,
    pub passed: bool,
    pub submitted_at: DateTime<Utc>,
}

pub(crate) async fn finish_attempt(
    conn: &mut PgConnection,
    id: i64,
    f: &Finish,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE exam_attempts
         SET answers = $2, score = $3, total_score = $4, score_percent = $5, passed = $6,
             submitted_at = $7
         WHERE id = $1",
        id,
        f.answers,
        f.score,
        f.total_score,
        f.score_percent,
        f.passed,
        f.submitted_at,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 本人已交卷的场次：指定 id，或最近一次。
pub(crate) async fn submitted_attempt(
    conn: &mut PgConnection,
    user_id: i64,
    id: Option<i64>,
) -> sqlx::Result<Option<AttemptRow>> {
    sqlx::query_as!(
        AttemptRow,
        "SELECT id, questions, answers, passed, score, total_score, score_percent, started_at,
                expires_at, submitted_at
         FROM exam_attempts
         WHERE user_id = $1 AND submitted_at IS NOT NULL AND ($2::bigint IS NULL OR id = $2)
         ORDER BY submitted_at DESC, id DESC
         LIMIT 1",
        user_id,
        id,
    )
    .fetch_optional(conn)
    .await
}

// ---------- 题库后台（EXAMADM） ----------

pub(crate) struct AdminQuestionRow {
    pub id: i64,
    pub question: String,
    pub kind: String,
    pub options: serde_json::Value,
    pub correct_keys: Vec<String>,
    pub explanation: Option<String>,
    pub weight: i32,
    pub enabled: bool,
    pub tag: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) struct QuestionFilter {
    pub kind: Option<String>,
    pub enabled: Option<bool>,
    /// ILIKE 模式（已转义）。
    pub pattern: Option<String>,
}

pub(crate) async fn count_questions(
    conn: &mut PgConnection,
    f: &QuestionFilter,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM exam_questions
           WHERE ($1::text IS NULL OR kind = $1)
             AND ($2::bool IS NULL OR enabled = $2)
             AND ($3::text IS NULL OR question ILIKE $3 OR tag ILIKE $3)"#,
        f.kind,
        f.enabled,
        f.pattern,
    )
    .fetch_one(conn)
    .await
}

/// 按更新时间倒序（EXAMADM-003）。
pub(crate) async fn list_questions(
    conn: &mut PgConnection,
    f: &QuestionFilter,
    limit: i64,
    offset: i64,
) -> sqlx::Result<Vec<AdminQuestionRow>> {
    sqlx::query_as!(
        AdminQuestionRow,
        r#"SELECT id, question, kind, options, correct_keys, explanation, weight, enabled, tag,
                  created_at, updated_at
           FROM exam_questions
           WHERE ($1::text IS NULL OR kind = $1)
             AND ($2::bool IS NULL OR enabled = $2)
             AND ($3::text IS NULL OR question ILIKE $3 OR tag ILIKE $3)
           ORDER BY updated_at DESC, id DESC
           LIMIT $4 OFFSET $5"#,
        f.kind,
        f.enabled,
        f.pattern,
        limit,
        offset,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn admin_question(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<AdminQuestionRow>> {
    sqlx::query_as!(
        AdminQuestionRow,
        "SELECT id, question, kind, options, correct_keys, explanation, weight, enabled, tag,
                created_at, updated_at
         FROM exam_questions WHERE id = $1",
        id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct QuestionStatsRow {
    pub total: i64,
    pub enabled: i64,
    pub single: i64,
    pub multiple: i64,
    pub boolean: i64,
}

pub(crate) async fn question_stats(conn: &mut PgConnection) -> sqlx::Result<QuestionStatsRow> {
    sqlx::query_as!(
        QuestionStatsRow,
        r#"SELECT count(*) AS "total!",
                  count(*) FILTER (WHERE enabled) AS "enabled!",
                  count(*) FILTER (WHERE kind = 'single') AS "single!",
                  count(*) FILTER (WHERE kind = 'multiple') AS "multiple!",
                  count(*) FILTER (WHERE kind = 'boolean') AS "boolean!"
           FROM exam_questions"#,
    )
    .fetch_one(conn)
    .await
}

/// 整体覆盖（EXAMADM-004）。返回是否存在。
pub(crate) async fn update_question(
    conn: &mut PgConnection,
    q: &NewQuestion<'_>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE exam_questions SET question = $2, kind = $3, options = $4, correct_keys = $5,
             explanation = $6, weight = $7, enabled = $8, tag = $9, updated_at = $10
         WHERE id = $1",
        q.id,
        q.question,
        q.kind,
        q.options,
        q.correct_keys,
        q.explanation,
        q.weight,
        q.enabled,
        q.tag,
        q.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 启用 / 停用取反（EXAMADM-006）。返回新状态，不存在时 None。
pub(crate) async fn toggle_question(
    conn: &mut PgConnection,
    id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<Option<bool>> {
    sqlx::query_scalar!(
        "UPDATE exam_questions SET enabled = NOT enabled, updated_at = $2 WHERE id = $1
         RETURNING enabled",
        id,
        now,
    )
    .fetch_optional(conn)
    .await
}

/// 物理删除（EXAMADM-007：历史场次有快照，不受影响）。返回实际删除的条数。
pub(crate) async fn delete_questions(conn: &mut PgConnection, ids: &[i64]) -> sqlx::Result<u64> {
    Ok(
        sqlx::query!("DELETE FROM exam_questions WHERE id = ANY($1)", ids)
            .execute(conn)
            .await?
            .rows_affected(),
    )
}
