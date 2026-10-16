//! 经济导入：余额截断、经验与丁尼流水的动作映射与去重键、签到连签重算。

mod common;

use chrono::NaiveDate;
use common::{day, import, setup};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[sqlx::test(migrator = "MIGRATOR")]
async fn economy_is_imported(db: PgPool) {
    setup(&db).await;
    sqlx::raw_sql(
        r#"
        INSERT INTO legacy.up_users (id, username, account_status, exp, denny, denny_given,
                                     last_check_in_date, consecutive_check_in_days, created_at) VALUES
          (1, 'rich', 'active', 120, 12000, 3, '2025-03-03', 2, '2025-01-01 00:00:00'),
          (2, 'poor', 'active', NULL, NULL, NULL, NULL, NULL, '2025-01-01 00:00:00');

        -- 北京时间：03-01 10:00 签到（业务日 03-01）；03-02 03:00 发帖（04:00 前仍算 03-01）；
        -- 03-01 同一天两次发帖（第二条去重键退回 legacy:）；一条不认识的动作；一条 0 经验。
        INSERT INTO legacy.experience_logs (id, action, exp, related_id, description, created_at) VALUES
          (1, 'check_in', 5, NULL, 'Experience change: 5/5 (Total: 5)', '2025-03-01 10:00:00'),
          (2, 'create_article', 4, 'p1', 'Experience change: 4/4', '2025-03-01 12:00:00'),
          (3, 'create_article', 4, 'p2', 'Experience change: 2/4', '2025-03-02 03:00:00'),
          (4, 'comment_accepted', 10, 'comment:77', NULL, '2025-03-02 12:00:00'),
          (5, 'mystery', 1, NULL, NULL, '2025-03-02 12:00:00'),
          (6, 'like_give', 0, NULL, NULL, '2025-03-02 12:00:00'),
          (7, 'check_in', 6, NULL, NULL, '2025-03-02 10:00:00');
        INSERT INTO legacy.experience_logs_user_lnk (id, experience_log_id, user_id) VALUES
          (1, 1, 1), (2, 2, 1), (3, 3, 1), (4, 4, 1), (5, 5, 1), (6, 6, 1), (7, 7, 1);

        INSERT INTO legacy.denny_logs (id, action, amount, balance, description, created_at) VALUES
          (1, 'daily_login', 3, 10003, '签到', '2025-03-01 10:00:00'),
          (2, 'give_post', -1, 5, NULL, '2025-03-02 10:00:00'),
          (3, 'weird', 1, 6, NULL, '2025-03-02 10:00:00');
        INSERT INTO legacy.denny_logs_user_lnk (id, denny_log_id, user_id) VALUES (1, 1, 1), (2, 2, 1), (3, 3, 1);

        -- 03-01、03-02 连签，03-05 断签后重新开始；03-02 重复记录。
        INSERT INTO legacy.check_in_logs (id, check_in_day, rank, created_at) VALUES
          (1, '2025-03-01', 1, '2025-03-01 10:00:00'), (2, '2025-03-02', 3, '2025-03-02 10:00:00'),
          (3, '2025-03-02', 4, '2025-03-02 10:01:00'), (4, '2025-03-05', NULL, '2025-03-05 10:00:00'),
          (5, '2025-03-02', 1, '2025-03-02 09:00:00');
        INSERT INTO legacy.check_in_logs_user_lnk (id, check_in_log_id, user_id) VALUES
          (1, 1, 1), (2, 2, 1), (3, 3, 1), (4, 4, 1), (5, 5, 2);
        "#,
    )
    .execute(&db)
    .await
    .unwrap();

    let findings = ik_legacy::check(&db).await.unwrap();
    let count = |key: &str| {
        findings
            .iter()
            .find(|f| f.key == key)
            .map_or(0, |f| f.count)
    };
    assert_eq!(count("economy.balance_clamped"), 1);
    assert_eq!(count("economy.exp_unknown_action"), 1);
    assert_eq!(count("economy.denny_unknown_action"), 1);

    import(&db, "Asia/Shanghai").await;

    let balances = sqlx::query!(
        "SELECT user_id, exp, denny_balance, denny_given, last_check_in_day, check_in_streak, check_in_total
         FROM user_economy ORDER BY user_id"
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(
        (
            balances[0].exp,
            balances[0].denny_balance,
            balances[0].denny_given
        ),
        (120, 9999, 3)
    );
    assert_eq!(balances[0].last_check_in_day, Some(day(2025, 3, 3)));
    assert_eq!(
        (balances[0].check_in_streak, balances[0].check_in_total),
        (2, 4)
    );
    assert_eq!(
        (
            balances[1].exp,
            balances[1].denny_balance,
            balances[1].check_in_total
        ),
        (0, 0, 1)
    );

    let exp = sqlx::query!(
        "SELECT id, action, amount, requested, bucket, biz_day, dedup_key FROM exp_logs ORDER BY id"
    )
    .fetch_all(&db)
    .await
    .unwrap();
    let rows: Vec<(i64, &str, i32, i32, &str, NaiveDate, &str)> = exp
        .iter()
        .map(|e| {
            (
                e.id,
                e.action.as_str(),
                e.amount,
                e.requested,
                e.bucket.as_str(),
                e.biz_day,
                e.dedup_key.as_str(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                1,
                "check_in",
                5,
                5,
                "active",
                day(2025, 3, 1),
                "day:2025-03-01"
            ),
            (
                2,
                "create_post",
                4,
                4,
                "active",
                day(2025, 3, 1),
                "day:2025-03-01"
            ),
            (
                3,
                "create_post",
                4,
                4,
                "active",
                day(2025, 3, 1),
                "legacy:3"
            ),
            (
                4,
                "comment_accepted",
                10,
                10,
                "active",
                day(2025, 3, 2),
                "ref:comment:77"
            ),
            (
                7,
                "check_in",
                6,
                6,
                "active",
                day(2025, 3, 2),
                "day:2025-03-02"
            ),
        ]
    );

    let denny = sqlx::query!("SELECT id, action, amount, balance FROM denny_logs ORDER BY id")
        .fetch_all(&db)
        .await
        .unwrap();
    let denny: Vec<(i64, &str, i32, i32)> = denny
        .iter()
        .map(|d| (d.id, d.action.as_str(), d.amount, d.balance))
        .collect();
    assert_eq!(
        denny,
        vec![(1, "check_in", 3, 9999), (2, "gift_sent", -1, 5)]
    );

    let check_ins = sqlx::query!(
        "SELECT user_id, day, rank, streak, exp_granted, denny_granted FROM check_ins ORDER BY user_id, day"
    )
    .fetch_all(&db)
    .await
    .unwrap();
    let check_ins: Vec<(i64, NaiveDate, i32, i32, i32, i32)> = check_ins
        .iter()
        .map(|c| {
            (
                c.user_id,
                c.day,
                c.rank,
                c.streak,
                c.exp_granted,
                c.denny_granted,
            )
        })
        .collect();
    assert_eq!(
        check_ins,
        vec![
            (1, day(2025, 3, 1), 1, 1, 5, 3),
            (1, day(2025, 3, 2), 3, 2, 6, 0),
            (1, day(2025, 3, 5), 1, 1, 0, 0),
            (2, day(2025, 3, 2), 1, 1, 0, 0),
        ]
    );
    let days = sqlx::query!("SELECT day, count FROM check_in_days ORDER BY day")
        .fetch_all(&db)
        .await
        .unwrap();
    let days: Vec<(NaiveDate, i32)> = days.iter().map(|d| (d.day, d.count)).collect();
    assert_eq!(
        days,
        vec![
            (day(2025, 3, 1), 1),
            (day(2025, 3, 2), 3),
            (day(2025, 3, 5), 1)
        ],
        "人数与最大排名取大"
    );
}
