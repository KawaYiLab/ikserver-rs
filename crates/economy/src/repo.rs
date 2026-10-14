//! SQL。只被本 crate 的 service 调用。

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgConnection;

// ---------- 钱包行 ----------

pub(crate) struct EconomyRow {
    pub exp: i64,
    pub denny_balance: i32,
    pub denny_given: i32,
    pub last_check_in_day: Option<NaiveDate>,
    pub check_in_streak: i32,
    pub check_in_total: i32,
}

/// 确保用户有一行经济数据。
pub(crate) async fn ensure_row(conn: &mut PgConnection, user_id: i64) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO user_economy (user_id) VALUES ($1) ON CONFLICT (user_id) DO NOTHING",
        user_id,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 锁住经济行。调用方必须先 [`ensure_row`]。
pub(crate) async fn lock_row(conn: &mut PgConnection, user_id: i64) -> sqlx::Result<EconomyRow> {
    sqlx::query_as!(
        EconomyRow,
        "SELECT exp, denny_balance, denny_given, last_check_in_day, check_in_streak, check_in_total
         FROM user_economy WHERE user_id = $1 FOR UPDATE",
        user_id,
    )
    .fetch_one(conn)
    .await
}

pub(crate) async fn get_row(
    conn: &mut PgConnection,
    user_id: i64,
) -> sqlx::Result<Option<EconomyRow>> {
    sqlx::query_as!(
        EconomyRow,
        "SELECT exp, denny_balance, denny_given, last_check_in_day, check_in_streak, check_in_total
         FROM user_economy WHERE user_id = $1",
        user_id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn set_exp(
    conn: &mut PgConnection,
    user_id: i64,
    exp: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE user_economy SET exp = $2, updated_at = $3 WHERE user_id = $1",
        user_id,
        exp,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 余额变动；`given_delta` 同时累加累计投出数。
pub(crate) async fn set_balance(
    conn: &mut PgConnection,
    user_id: i64,
    balance: i32,
    given_delta: i32,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE user_economy SET denny_balance = $2, denny_given = denny_given + $3, updated_at = $4
         WHERE user_id = $1",
        user_id,
        balance,
        given_delta,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

// ---------- 经验流水 ----------

pub(crate) async fn exp_log_exists(
    conn: &mut PgConnection,
    user_id: i64,
    action: &str,
    dedup_key: &str,
) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM exp_logs WHERE user_id = $1 AND action = $2 AND dedup_key = $3
           ) AS "exists!""#,
        user_id,
        action,
        dedup_key,
    )
    .fetch_one(conn)
    .await
}

/// 某个业务日某个桶已发放的经验。
pub(crate) async fn exp_gained_in_bucket(
    conn: &mut PgConnection,
    user_id: i64,
    day: NaiveDate,
    bucket: &str,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT COALESCE(SUM(amount), 0)::bigint AS "n!"
           FROM exp_logs WHERE user_id = $1 AND biz_day = $2 AND bucket = $3"#,
        user_id,
        day,
        bucket,
    )
    .fetch_one(conn)
    .await
}

pub(crate) struct NewExpLog<'a> {
    pub id: i64,
    pub user_id: i64,
    pub action: &'a str,
    pub amount: i32,
    pub requested: i32,
    pub bucket: &'a str,
    pub biz_day: NaiveDate,
    pub dedup_key: &'a str,
    pub now: DateTime<Utc>,
}

pub(crate) async fn insert_exp_log(
    conn: &mut PgConnection,
    log: &NewExpLog<'_>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO exp_logs (id, user_id, action, amount, requested, bucket, biz_day, dedup_key,
                               created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        log.id,
        log.user_id,
        log.action,
        log.amount,
        log.requested,
        log.bucket,
        log.biz_day,
        log.dedup_key,
        log.now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct ActionSum {
    pub action: String,
    pub amount: i64,
}

/// 某个业务日各动作发放的经验。
pub(crate) async fn exp_by_action(
    conn: &mut PgConnection,
    user_id: i64,
    day: NaiveDate,
) -> sqlx::Result<Vec<ActionSum>> {
    sqlx::query_as!(
        ActionSum,
        r#"SELECT action, COALESCE(SUM(amount), 0)::bigint AS "amount!"
           FROM exp_logs WHERE user_id = $1 AND biz_day = $2
           GROUP BY action"#,
        user_id,
        day,
    )
    .fetch_all(conn)
    .await
}

// ---------- 丁尼流水 ----------

pub(crate) async fn denny_log_exists(
    conn: &mut PgConnection,
    user_id: i64,
    action: &str,
    ref_key: &str,
) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM denny_logs WHERE user_id = $1 AND action = $2 AND ref_key = $3
           ) AS "exists!""#,
        user_id,
        action,
        ref_key,
    )
    .fetch_one(conn)
    .await
}

pub(crate) struct NewDennyLog<'a> {
    pub id: i64,
    pub user_id: i64,
    pub action: &'a str,
    pub amount: i32,
    pub balance: i32,
    pub ref_key: Option<&'a str>,
    pub description: &'a str,
    pub now: DateTime<Utc>,
}

pub(crate) async fn insert_denny_log(
    conn: &mut PgConnection,
    log: &NewDennyLog<'_>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO denny_logs (id, user_id, action, amount, balance, ref_key, description, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        log.id,
        log.user_id,
        log.action,
        log.amount,
        log.balance,
        log.ref_key,
        log.description,
        log.now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct DennyLogRow {
    pub id: i64,
    pub action: String,
    pub amount: i32,
    pub balance: i32,
    pub description: String,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn denny_logs_page(
    conn: &mut PgConnection,
    user_id: i64,
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<DennyLogRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        DennyLogRow,
        "SELECT id, action, amount, balance, description, created_at
         FROM denny_logs
         WHERE user_id = $1
           AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3::bigint))
         ORDER BY created_at DESC, id DESC
         LIMIT $4",
        user_id,
        after_at,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

// ---------- 签到 ----------

pub(crate) async fn record_check_in_state(
    conn: &mut PgConnection,
    user_id: i64,
    day: NaiveDate,
    streak: i32,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE user_economy
         SET last_check_in_day = $2, check_in_streak = $3, check_in_total = check_in_total + 1,
             updated_at = $4
         WHERE user_id = $1",
        user_id,
        day,
        streak,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 当日签到人数 +1，返回自己的排名。
pub(crate) async fn next_check_in_rank(
    conn: &mut PgConnection,
    day: NaiveDate,
) -> sqlx::Result<i32> {
    sqlx::query_scalar!(
        "INSERT INTO check_in_days (day, count) VALUES ($1, 1)
         ON CONFLICT (day) DO UPDATE SET count = check_in_days.count + 1
         RETURNING count",
        day,
    )
    .fetch_one(conn)
    .await
}

pub(crate) struct NewCheckIn {
    pub user_id: i64,
    pub day: NaiveDate,
    pub rank: i32,
    pub streak: i32,
    pub exp_granted: i32,
    pub denny_granted: i32,
    pub now: DateTime<Utc>,
}

pub(crate) async fn insert_check_in(conn: &mut PgConnection, c: &NewCheckIn) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO check_ins (user_id, day, rank, streak, exp_granted, denny_granted, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        c.user_id,
        c.day,
        c.rank,
        c.streak,
        c.exp_granted,
        c.denny_granted,
        c.now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn check_in_rank(
    conn: &mut PgConnection,
    user_id: i64,
    day: NaiveDate,
) -> sqlx::Result<Option<i32>> {
    sqlx::query_scalar!(
        "SELECT rank FROM check_ins WHERE user_id = $1 AND day = $2",
        user_id,
        day,
    )
    .fetch_optional(conn)
    .await
}

// ---------- 投币 ----------

/// 写投币记录。已投过（主键冲突）时返回 false。
pub(crate) async fn insert_gift(
    conn: &mut PgConnection,
    post_id: i64,
    giver_id: i64,
    message: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "INSERT INTO post_denny_gifts (post_id, giver_id, message, created_at)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (post_id, giver_id) DO NOTHING",
        post_id,
        giver_id,
        message,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn gifted_post_ids(
    conn: &mut PgConnection,
    giver_id: i64,
    post_ids: &[i64],
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT post_id FROM post_denny_gifts WHERE giver_id = $1 AND post_id = ANY($2)",
        giver_id,
        post_ids,
    )
    .fetch_all(conn)
    .await
}
