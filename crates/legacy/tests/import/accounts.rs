//! 账号导入：状态 / 邮箱 / 用户名的规范化、密码、资料、时区换算。

use crate::common::{import, setup};
use chrono::{TimeZone, Utc};
use sqlx::PgPool;

use crate::MIGRATOR;

const BCRYPT: &str = "$2a$10$abcdefghijklmnopqrstuuMXZf3w0pQ4c1H9Q8a9oQ1lH1o9e5W1e";

#[sqlx::test(migrator = "MIGRATOR")]
async fn accounts_are_normalized(db: PgPool) {
    setup(&db).await;
    // 测试里的固定数据（只拼进常量哈希）。
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        r#"
        INSERT INTO legacy.up_users (id, document_id, username, email, provider, account_status, password,
                                     blocked, is_admin, is_ai_agent, exam_passed, exam_passed_at, level,
                                     profile_hidden, created_at, updated_at) VALUES
          (1, 'u1', 'boss', ' Boss@Example.COM ', 'local', 'active', '{BCRYPT}', false, true, false, true,
           '2025-06-01 10:00:00', 5, false, '2025-01-01 08:00:00', '2025-01-02 08:00:00'),
          (2, 'u2', '绳网用户abc', 'mihoyo_42@mihoyo-login.inter-knot.invalid', 'mihoyo', 'active', '{BCRYPT}',
           false, false, false, true, NULL, 1, true, '2025-02-01 00:00:00', NULL),
          (3, 'u3', '已注销用户_3', 'deleted_3_1700000000000@anonymized.local', 'local', 'deleted',
           'ffffffffffffffffffffffffffffffff', true, false, false, true, NULL, 2, false,
           '2025-03-01 00:00:00', '2025-03-02 00:00:00'),
          (4, 'u4', 'a@b', 'four@example.com', 'local', 'active', '{BCRYPT}', true, false, false, false,
           NULL, 9, false, '2025-04-01 00:00:00', '2025-04-01 00:00:00'),
          (5, 'u5', 'a＠b', 'five@example.com', 'local', NULL, NULL, false, false, true, false, NULL, NULL,
           NULL, '2025-05-01 00:00:00', '2025-05-01 00:00:00');
        INSERT INTO legacy.authors (id, document_id, name, bio, profile_pinned_articles) VALUES
          (11, 'a1', 'boss', '[{{"type":"paragraph","children":[{{"type":"text","text":"你好"}},
             {{"type":"link","url":"https://x","children":[{{"type":"text","text":"链接"}}]}}]}},
             {{"type":"paragraph","children":[{{"type":"text","text":"第二段"}}]}}]', '[]'),
          (12, 'a2', 'mhy', NULL, NULL);
        INSERT INTO legacy.up_users_author_lnk (id, user_id, author_id) VALUES (1, 1, 11), (2, 2, 12);
        "#
    )))
    .execute(&db)
    .await
    .unwrap();

    import(&db, "Asia/Shanghai").await;

    let users = sqlx::query!(
        r#"SELECT id, username, email, status, role, is_ai_agent, exam_passed_at, legacy_id, level,
                  created_at, updated_at
           FROM users ORDER BY id"#
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(users.len(), 5);

    let boss = &users[0];
    assert_eq!(boss.username, "boss");
    assert_eq!(boss.email.as_deref(), Some("boss@example.com"));
    assert_eq!(
        (boss.status.as_str(), boss.role.as_str()),
        ("active", "admin")
    );
    // 北京时间 08:00 = UTC 00:00。
    assert_eq!(
        boss.created_at,
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()
    );
    assert_eq!(
        boss.exam_passed_at,
        Some(Utc.with_ymd_and_hms(2025, 6, 1, 2, 0, 0).unwrap())
    );
    assert_eq!(
        boss.legacy_id.as_deref(),
        Some("a1"),
        "优先用作者的 documentId"
    );
    assert_eq!(boss.level, 5);

    let mihoyo = &users[1];
    assert_eq!(mihoyo.email, None, "米游社占位邮箱置空");
    assert_eq!(
        mihoyo.exam_passed_at,
        Some(crate::common::NOW),
        "存量豁免用导入时刻"
    );
    assert_eq!(mihoyo.updated_at, mihoyo.created_at);

    let deleted = &users[2];
    assert_eq!(
        (deleted.status.as_str(), deleted.email.as_deref()),
        ("deleted", None)
    );
    assert_eq!(
        deleted.legacy_id.as_deref(),
        Some("u3"),
        "没有作者时用账号的 documentId"
    );

    let renamed = &users[3];
    assert_eq!(
        renamed.username, "用户_4",
        "改名后与 5 号的原名撞名，原名合法的一方保留"
    );
    assert_eq!(renamed.status, "blocked");
    assert_eq!(renamed.level, 7, "等级截到 0–7");
    assert_eq!(renamed.exam_passed_at, None);

    let legit = &users[4];
    assert_eq!(legit.username, "a＠b");
    assert_eq!((legit.status.as_str(), legit.is_ai_agent), ("active", true));

    // 只有本地注册、仍可登录、哈希是 bcrypt 的账号才迁密码。
    let with_password: Vec<i64> =
        sqlx::query_scalar!("SELECT user_id FROM user_passwords ORDER BY user_id")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(with_password, vec![1, 4]);

    let profiles = sqlx::query!("SELECT user_id, bio, hidden FROM user_profiles ORDER BY user_id")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(profiles.len(), 2);
    assert_eq!(profiles[0].bio, "你好链接\n第二段");
    assert!(!profiles[0].hidden);
    assert_eq!(
        (
            profiles[1].user_id,
            profiles[1].bio.as_str(),
            profiles[1].hidden
        ),
        (2, "", true)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn duplicate_emails_block_the_import(db: PgPool) {
    setup(&db).await;
    sqlx::raw_sql(
        r#"INSERT INTO legacy.up_users (id, username, email, account_status, created_at) VALUES
             (1, 'x', 'Same@Example.com', 'active', '2025-01-01'),
             (2, 'y', 'same@example.com ', 'active', '2025-01-01')"#,
    )
    .execute(&db)
    .await
    .unwrap();

    let findings = ik_legacy::check(&db).await.unwrap();
    let dup = findings
        .iter()
        .find(|f| f.key == "users.duplicate_email")
        .unwrap();
    assert_eq!(
        (dup.severity, dup.count),
        (ik_legacy::Severity::Blocking, 1)
    );

    let error = ik_legacy::import(&db, &crate::common::options("UTC"))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("users.duplicate_email"),
        "{error}"
    );
    let count: i64 = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM users"#)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn refuses_unknown_timezone_and_non_empty_target(db: PgPool) {
    setup(&db).await;
    let error = ik_legacy::import(&db, &crate::common::options("Mars/Base"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("未知的时区"), "{error}");

    sqlx::query!("INSERT INTO users (id, username) VALUES (99, 'already')")
        .execute(&db)
        .await
        .unwrap();
    let error = ik_legacy::import(&db, &crate::common::options("UTC"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("全新的库"), "{error}");
}
