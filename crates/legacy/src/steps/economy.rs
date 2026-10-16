//! 经济：余额与累计（user_economy）、经验流水、丁尼流水、签到与每日签到人数。
//!
//! 业务日按新口径（北京时间 04:00 切日，`ik_kernel::time::biz_day`）从记录时间算出。

use sqlx::{PgConnection, Postgres, Transaction};

use crate::import::{ImportOptions, ImportReport};

pub async fn run(
    tx: &mut Transaction<'_, Postgres>,
    options: &ImportOptions,
    report: &mut ImportReport,
) -> anyhow::Result<()> {
    report.add("user_economy", balances(tx, options).await?);
    report.add("exp_logs", exp_logs(tx, options).await?);
    report.add("denny_logs", denny_logs(tx, options).await?);
    report.add("check_ins", check_ins(tx, options).await?);
    report.add("check_in_days", check_in_days(tx).await?);
    Ok(())
}

/// 每个账号一行。余额截到 0–9999（旧系统入账同样封顶 9999，DENNY-006；预检里报数量）。
async fn balances(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO user_economy (user_id, exp, denny_balance, denny_given, last_check_in_day,
                                     check_in_streak, check_in_total, created_at, updated_at)
           SELECT u.id, greatest(coalesce(o.exp, 0), 0),
                  least(greatest(coalesce(o.denny, 0), 0), 9999),
                  greatest(coalesce(o.denny_given, 0), 0),
                  o.last_check_in_date,
                  greatest(coalesce(o.consecutive_check_in_days, 0), 0),
                  (SELECT count(*) FROM legacy.ik_check_in_users c WHERE c.user_id = u.id)::int,
                  u.created_at, $1
           FROM users u
           JOIN legacy.up_users o ON o.id = u.id"#,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 只导入实际发放量 > 0、动作认识的行。申请量从描述 `Experience change: 发放/申请 …` 里取（EXP-011）。
/// 去重键：按日去重的动作给当天第一条 `day:<业务日>`、采纳给 `ref:comment:<id>`（与新代码一致，
/// 切换当天不会重复发放），其余 `legacy:<id>`。
async fn exp_logs(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO exp_logs (id, user_id, action, amount, requested, bucket, biz_day, dedup_key,
                                 created_at)
           SELECT k.id, k.user_id, k.action, k.amount, k.requested, k.bucket, k.biz_day,
                  CASE WHEN k.rn = 1 AND k.candidate IS NOT NULL THEN k.candidate
                       ELSE 'legacy:' || k.id END,
                  k.at
           FROM (
               SELECT d.*,
                      row_number() OVER (PARTITION BY d.user_id, d.action, d.candidate
                                         ORDER BY d.at, d.id) AS rn
               FROM (
                   SELECT t.*,
                          CASE WHEN t.daily THEN 'day:' || to_char(t.biz_day, 'YYYY-MM-DD')
                               WHEN t.action = 'comment_accepted' AND t.related_id ~ '^comment:\d+$'
                               THEN 'ref:' || t.related_id
                          END AS candidate
                   FROM (
                       SELECT e.id, lu.user_id, m.action, m.bucket, m.daily, e.related_id,
                              e.exp AS amount,
                              greatest(coalesce(substring(e.description FROM 'Experience change: -?\d+/(\d+)')::int,
                                                e.exp), e.exp) AS requested,
                              coalesce(e.created_at AT TIME ZONE $1, $2) AS at,
                              ((coalesce(e.created_at AT TIME ZONE $1, $2) AT TIME ZONE 'Asia/Shanghai')
                               - interval '4 hours')::date AS biz_day
                       FROM legacy.experience_logs e
                       JOIN legacy.ik_exp_log_users lu ON lu.log_id = e.id
                       JOIN users u ON u.id = lu.user_id
                       JOIN legacy.ik_exp_actions m ON m.legacy_action = e.action
                       WHERE e.exp > 0
                   ) t
               ) d
           ) k"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 金额为 0 或动作不认识的行不导入；余额截到 0–9999；描述截到 255 字。
async fn denny_logs(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO denny_logs (id, user_id, action, amount, balance, description, created_at)
           SELECT d.id, lu.user_id, m.action, d.amount,
                  least(greatest(coalesce(d.balance, 0), 0), 9999),
                  left(coalesce(d.description, ''), 255),
                  coalesce(d.created_at AT TIME ZONE $1, $2)
           FROM legacy.denny_logs d
           JOIN legacy.ik_denny_log_users lu ON lu.log_id = d.id
           JOIN users u ON u.id = lu.user_id
           JOIN legacy.ik_denny_actions m ON m.legacy_action = d.action
           WHERE d.amount IS NOT NULL AND d.amount <> 0"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 每人每天一行。连签天数由连续的签到日重新算出；当天发的经验与丁尼取同一业务日的签到流水。
async fn check_ins(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO check_ins (user_id, day, rank, streak, exp_granted, denny_granted, created_at)
           SELECT s.user_id, s.day, s.rank,
                  row_number() OVER (PARTITION BY s.user_id, s.grp ORDER BY s.day)::int,
                  coalesce((SELECT sum(x.amount) FROM exp_logs x
                            WHERE x.user_id = s.user_id AND x.action = 'check_in'
                              AND x.biz_day = s.day), 0)::int,
                  coalesce((SELECT sum(d.amount) FROM denny_logs d
                            WHERE d.user_id = s.user_id AND d.action = 'check_in' AND d.amount > 0
                              AND ((d.created_at AT TIME ZONE 'Asia/Shanghai') - interval '4 hours')::date
                                  = s.day), 0)::int,
                  s.at
           FROM (
               SELECT c.*,
                      c.day - (row_number() OVER (PARTITION BY c.user_id ORDER BY c.day))::int AS grp
               FROM (
                   SELECT DISTINCT ON (cu.user_id, l.check_in_day)
                          cu.user_id, l.check_in_day AS day, greatest(coalesce(l.rank, 1), 1) AS rank,
                          coalesce(l.created_at AT TIME ZONE $1, $2) AS at
                   FROM legacy.check_in_logs l
                   JOIN legacy.ik_check_in_users cu ON cu.log_id = l.id
                   JOIN users u ON u.id = cu.user_id
                   WHERE l.check_in_day IS NOT NULL
                   ORDER BY cu.user_id, l.check_in_day, l.id
               ) c
           ) s"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 每日签到人数：新签到的排名接在后面，所以取「人数」与「最大排名」中的较大者。
async fn check_in_days(conn: &mut PgConnection) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO check_in_days (day, count)
           SELECT day, greatest(count(*), max(rank))::int FROM check_ins GROUP BY day"#
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}
