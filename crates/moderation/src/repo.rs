use chrono::{DateTime, Utc};
use sqlx::PgConnection;

pub(crate) struct NewReport<'a> {
    pub id: i64,
    pub reporter_id: i64,
    pub target_type: &'a str,
    pub target_id: i64,
    pub owner_user_id: Option<i64>,
    pub reason: &'a str,
    pub detail: Option<&'a str>,
    pub snapshot: serde_json::Value,
    pub now: DateTime<Utc>,
}

pub(crate) async fn insert(conn: &mut PgConnection, r: NewReport<'_>) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO reports (id, reporter_id, target_type, target_id, owner_user_id, reason, detail,
                              snapshot, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        r.id,
        r.reporter_id,
        r.target_type,
        r.target_id,
        r.owner_user_id,
        r.reason,
        r.detail,
        r.snapshot,
        r.now
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// `ids` 中我有未结案举报的目标。
pub(crate) async fn open_targets_of(
    conn: &mut PgConnection,
    reporter_id: i64,
    target_type: &str,
    ids: &[i64],
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT target_id FROM reports
         WHERE reporter_id = $1 AND target_type = $2 AND target_id = ANY($3)
           AND status IN ('pending', 'reviewing')",
        reporter_id,
        target_type,
        ids
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct MyRow {
    pub id: i64,
    pub target_type: String,
    pub target_id: i64,
    pub reason: String,
    pub detail: Option<String>,
    pub status: String,
    pub resolution: Option<String>,
    pub created_at: DateTime<Utc>,
    pub handled_at: Option<DateTime<Utc>>,
    pub snapshot: serde_json::Value,
}

pub(crate) async fn mine(
    conn: &mut PgConnection,
    reporter_id: i64,
    before: Option<i64>,
    limit: i64,
) -> sqlx::Result<Vec<MyRow>> {
    sqlx::query_as!(
        MyRow,
        "SELECT id, target_type, target_id, reason, detail, status, resolution, created_at, handled_at,
                snapshot
         FROM reports WHERE reporter_id = $1 AND ($2::bigint IS NULL OR id < $2)
         ORDER BY id DESC LIMIT $3",
        reporter_id,
        before,
        limit
    )
    .fetch_all(conn)
    .await
}

/// 目标的待处理举报来自几个不同的人（自动隐藏阈值，RPT-052）。
pub(crate) async fn pending_reporters(
    conn: &mut PgConnection,
    target_type: &str,
    target_id: i64,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(DISTINCT reporter_id) AS "n!" FROM reports
           WHERE target_type = $1 AND target_id = $2 AND status = 'pending'"#,
        target_type,
        target_id
    )
    .fetch_one(conn)
    .await
}

/// 自动隐藏后，待处理的举报转入复核中（不写领取人，仍可领取）。
pub(crate) async fn pending_to_reviewing(
    conn: &mut PgConnection,
    target_type: &str,
    target_id: i64,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "UPDATE reports SET status = 'reviewing'
         WHERE target_type = $1 AND target_id = $2 AND status = 'pending'",
        target_type,
        target_id
    )
    .execute(conn)
    .await?
    .rows_affected())
}

pub(crate) struct OpenRow {
    pub reporter_id: i64,
    pub owner_user_id: Option<i64>,
    pub status: String,
    pub assigned_to: Option<i64>,
    pub snapshot: serde_json::Value,
}

/// 组锁（RPTADM-095）：锁住目标的全部未结案举报，按 id 升序加锁避免死锁。
pub(crate) async fn lock_open(
    conn: &mut PgConnection,
    target_type: &str,
    target_id: i64,
) -> sqlx::Result<Vec<OpenRow>> {
    sqlx::query_as!(
        OpenRow,
        "SELECT reporter_id, owner_user_id, status, assigned_to, snapshot FROM reports
         WHERE target_type = $1 AND target_id = $2 AND status IN ('pending', 'reviewing')
         ORDER BY id FOR UPDATE",
        target_type,
        target_id
    )
    .fetch_all(conn)
    .await
}

/// 有未结案举报的目标（在 `ids` 之中）。
pub(crate) async fn open_targets(
    conn: &mut PgConnection,
    target_type: &str,
    ids: &[i64],
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT DISTINCT target_id FROM reports
         WHERE target_type = $1 AND target_id = ANY($2) AND status IN ('pending', 'reviewing')",
        target_type,
        ids
    )
    .fetch_all(conn)
    .await
}

/// 领取：待处理的，以及复核中但没人领取的（自动隐藏转入的）。
pub(crate) async fn claim(
    conn: &mut PgConnection,
    target_type: &str,
    target_id: i64,
    staff_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "UPDATE reports SET status = 'reviewing', assigned_to = $3, assigned_at = $4
         WHERE target_type = $1 AND target_id = $2
           AND (status = 'pending' OR (status = 'reviewing' AND assigned_to IS NULL))",
        target_type,
        target_id,
        staff_id,
        now
    )
    .execute(conn)
    .await?
    .rows_affected())
}

/// 释放：自己领取的复核中举报改回待处理。
pub(crate) async fn unclaim(
    conn: &mut PgConnection,
    target_type: &str,
    target_id: i64,
    staff_id: i64,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "UPDATE reports SET status = 'pending', assigned_to = NULL, assigned_at = NULL
         WHERE target_type = $1 AND target_id = $2 AND status = 'reviewing' AND assigned_to = $3",
        target_type,
        target_id,
        staff_id
    )
    .execute(conn)
    .await?
    .rows_affected())
}

pub(crate) struct Closing<'a> {
    pub status: &'a str,
    pub resolution: &'a str,
    pub note: Option<&'a str>,
    pub target_note: Option<&'a str>,
    pub handled_by: i64,
    pub now: DateTime<Utc>,
}

/// 整组结案（RPTADM-055）。返回结案的条数。
pub(crate) async fn close(
    conn: &mut PgConnection,
    target_type: &str,
    target_id: i64,
    c: &Closing<'_>,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "UPDATE reports
         SET status = $3, resolution = $4, resolution_note = $5, target_resolution_note = $6,
             handled_by = $7, handled_at = $8
         WHERE target_type = $1 AND target_id = $2 AND status IN ('pending', 'reviewing')",
        target_type,
        target_id,
        c.status,
        c.resolution,
        c.note,
        c.target_note,
        c.handled_by,
        c.now
    )
    .execute(conn)
    .await?
    .rows_affected())
}

pub(crate) struct NewLog<'a> {
    pub id: i64,
    pub actor_id: Option<i64>,
    pub action: &'a str,
    pub target_type: &'a str,
    pub target_id: i64,
    pub report_count: i32,
    pub note: Option<&'a str>,
    /// 被处置的人（内容作者或被处置的用户）；查不到时为 None。
    pub subject_user_id: Option<i64>,
    /// 处置的是匿名内容：按人查询处罚记录时排除（AGENTS.md 规则 10）。
    pub subject_anonymous: bool,
    pub now: DateTime<Utc>,
}

pub(crate) async fn log(conn: &mut PgConnection, l: NewLog<'_>) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO moderation_log (id, actor_id, action, target_type, target_id, report_count, note,
                                     subject_user_id, subject_anonymous, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        l.id,
        l.actor_id,
        l.action,
        l.target_type,
        l.target_id,
        l.report_count,
        l.note,
        l.subject_user_id,
        l.subject_anonymous,
        l.now
    )
    .execute(conn)
    .await?;
    Ok(())
}

// ---------- 后台列表 ----------

pub(crate) struct GroupFilter<'a> {
    pub statuses: &'a [String],
    pub target_type: Option<&'a str>,
    pub target_id: Option<i64>,
    pub reason: Option<&'a str>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub assigned_to: Option<i64>,
    /// ILIKE 模式（调用方已转义 `%`、`_`、`\`）。
    pub pattern: Option<&'a str>,
    pub reporter_ids: &'a [i64],
    pub limit: i64,
    pub offset: i64,
}

pub(crate) struct GroupRow {
    pub target_type: String,
    pub target_id: i64,
    pub report_count: i64,
    pub first_at: DateTime<Utc>,
    pub latest_at: DateTime<Utc>,
    pub status: String,
    pub assigned_to: Option<i64>,
    pub snapshot: serde_json::Value,
    pub owner_user_id: Option<i64>,
    pub total: i64,
}

/// 按目标聚合（RPTADM-010～019），在数据库里 GROUP BY（旧系统取 5000 条到内存里聚合，RPTADM-102）。
pub(crate) async fn groups(
    conn: &mut PgConnection,
    f: &GroupFilter<'_>,
) -> sqlx::Result<Vec<GroupRow>> {
    sqlx::query_as!(
        GroupRow,
        r#"SELECT target_type AS "target_type!", target_id AS "target_id!",
                  count(*) AS "report_count!",
                  min(created_at) AS "first_at!", max(created_at) AS "latest_at!",
                  CASE WHEN bool_or(status = 'pending') THEN 'pending'
                       WHEN bool_or(status = 'reviewing') THEN 'reviewing'
                       ELSE (array_agg(status ORDER BY created_at DESC, id DESC))[1] END AS "status!",
                  (array_agg(assigned_to ORDER BY created_at DESC, id DESC)
                       FILTER (WHERE assigned_to IS NOT NULL))[1] AS assigned_to,
                  (array_agg(snapshot ORDER BY created_at DESC, id DESC))[1] AS "snapshot!",
                  (array_agg(owner_user_id ORDER BY created_at DESC, id DESC))[1] AS owner_user_id,
                  count(*) OVER () AS "total!"
           FROM reports
           WHERE status = ANY($1)
             AND ($2::text IS NULL OR target_type = $2)
             AND ($3::text IS NULL OR reason = $3)
             AND ($4::timestamptz IS NULL OR created_at >= $4)
             AND ($5::timestamptz IS NULL OR created_at < $5)
             AND ($6::bigint IS NULL OR assigned_to = $6)
             AND ($11::bigint IS NULL OR target_id = $11)
           GROUP BY target_type, target_id
           HAVING $7::text IS NULL OR bool_or(
               snapshot->>'title' ILIKE $7 OR snapshot->>'excerpt' ILIKE $7
               OR snapshot->>'username' ILIKE $7 OR reporter_id = ANY($8))
           ORDER BY max(created_at) DESC, target_id DESC
           LIMIT $9 OFFSET $10"#,
        f.statuses,
        f.target_type,
        f.reason,
        f.from,
        f.to,
        f.assigned_to,
        f.pattern,
        f.reporter_ids,
        f.limit,
        f.offset,
        f.target_id
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct ReportRowDb {
    pub id: i64,
    pub target_type: String,
    pub target_id: i64,
    pub reporter_id: i64,
    pub reason: String,
    pub detail: Option<String>,
    pub status: String,
    pub resolution: Option<String>,
    pub resolution_note: Option<String>,
    pub target_resolution_note: Option<String>,
    pub assigned_to: Option<i64>,
    pub assigned_at: Option<DateTime<Utc>>,
    pub handled_by: Option<i64>,
    pub handled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// 这些目标的举报（状态在 `statuses` 里），新的在前。
pub(crate) async fn rows_of(
    conn: &mut PgConnection,
    types: &[String],
    ids: &[i64],
    statuses: &[String],
) -> sqlx::Result<Vec<ReportRowDb>> {
    sqlx::query_as!(
        ReportRowDb,
        "SELECT r.id, r.target_type, r.target_id, r.reporter_id, r.reason, r.detail, r.status,
                r.resolution, r.resolution_note, r.target_resolution_note, r.assigned_to, r.assigned_at,
                r.handled_by, r.handled_at, r.created_at
         FROM reports r
         JOIN unnest($1::text[], $2::bigint[]) AS t(tt, tid) ON r.target_type = t.tt AND r.target_id = t.tid
         WHERE r.status = ANY($3)
         ORDER BY r.created_at DESC, r.id DESC",
        types,
        ids,
        statuses
    )
    .fetch_all(conn)
    .await
}

/// 待处理、复核中的举报目标数（按目标聚合，仪表盘）。
pub(crate) async fn open_target_counts(conn: &mut PgConnection) -> sqlx::Result<(i64, i64)> {
    let row = sqlx::query!(
        r#"SELECT count(DISTINCT (target_type, target_id)) FILTER (WHERE status = 'pending') AS "pending!",
                  count(DISTINCT (target_type, target_id)) FILTER (WHERE status = 'reviewing') AS "reviewing!"
           FROM reports WHERE status IN ('pending', 'reviewing')"#,
    )
    .fetch_one(conn)
    .await?;
    Ok((row.pending, row.reviewing))
}

// ---------- 管理后台：按人查处罚记录与收到的举报 ----------
// 匿名内容的处置与举报都排除（AGENTS.md 规则 10）：否则能从一个人的记录反推出他的匿名内容。

pub(crate) struct SubjectLogRow {
    pub actor_id: Option<i64>,
    pub action: String,
    pub target_type: String,
    pub target_id: i64,
    pub report_count: i32,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn subject_logs(
    conn: &mut PgConnection,
    user_id: i64,
    limit: i64,
) -> sqlx::Result<Vec<SubjectLogRow>> {
    sqlx::query_as!(
        SubjectLogRow,
        "SELECT actor_id, action, target_type, target_id, report_count, note, created_at
         FROM moderation_log
         WHERE subject_user_id = $1 AND NOT subject_anonymous
         ORDER BY created_at DESC, id DESC
         LIMIT $2",
        user_id,
        limit
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct ReceivedStatsRow {
    pub total: i64,
    pub open: i64,
    pub accepted: i64,
    pub rejected: i64,
}

pub(crate) async fn received_stats(
    conn: &mut PgConnection,
    user_id: i64,
) -> sqlx::Result<ReceivedStatsRow> {
    sqlx::query_as!(
        ReceivedStatsRow,
        r#"SELECT count(*) AS "total!",
                  count(*) FILTER (WHERE status IN ('pending', 'reviewing')) AS "open!",
                  count(*) FILTER (WHERE status = 'accepted') AS "accepted!",
                  count(*) FILTER (WHERE status = 'rejected') AS "rejected!"
           FROM reports
           WHERE owner_user_id = $1 AND NOT COALESCE((snapshot ->> 'anonymous')::boolean, FALSE)"#,
        user_id
    )
    .fetch_one(conn)
    .await
}

pub(crate) struct ReceivedRow {
    pub target_type: String,
    pub target_id: i64,
    pub reason: String,
    pub detail: Option<String>,
    pub status: String,
    pub resolution: Option<String>,
    pub snapshot: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn received_reports(
    conn: &mut PgConnection,
    user_id: i64,
    limit: i64,
) -> sqlx::Result<Vec<ReceivedRow>> {
    sqlx::query_as!(
        ReceivedRow,
        "SELECT target_type, target_id, reason, detail, status, resolution, snapshot, created_at
         FROM reports
         WHERE owner_user_id = $1 AND NOT COALESCE((snapshot ->> 'anonymous')::boolean, FALSE)
         ORDER BY created_at DESC, id DESC
         LIMIT $2",
        user_id,
        limit
    )
    .fetch_all(conn)
    .await
}

/// 每个业务日收到的举报条数（仪表盘趋势）。
pub(crate) async fn daily_reports(
    conn: &mut PgConnection,
    bounds: &[DateTime<Utc>],
) -> sqlx::Result<Vec<(i32, i64)>> {
    let (Some(first), Some(last)) = (bounds.first(), bounds.last()) else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query!(
        r#"SELECT width_bucket(created_at, $1::timestamptz[]) AS "bucket!", count(*) AS "n!"
           FROM reports
           WHERE created_at >= $2 AND created_at < $3
           GROUP BY 1"#,
        bounds,
        first,
        last,
    )
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|r| (r.bucket, r.n)).collect())
}

/// 某个版主领取了、还没结案的举报目标数。
pub(crate) async fn claimed_open_count(
    conn: &mut PgConnection,
    staff_id: i64,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(DISTINCT (target_type, target_id)) AS "n!"
           FROM reports WHERE status = 'reviewing' AND assigned_to = $1"#,
        staff_id
    )
    .fetch_one(conn)
    .await
}
