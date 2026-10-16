//! 通知与举报：匿名通知改为系统通知、自己给自己的不导入、举报的状态与处理人。

mod common;

use common::{import, setup};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[sqlx::test(migrator = "MIGRATOR")]
async fn notifications_and_reports(db: PgPool) {
    setup(&db).await;
    sqlx::raw_sql(
        r#"
        INSERT INTO legacy.up_users (id, username, email, account_status, created_at) VALUES
          (1, 'alice', 'alice@example.com', 'active', '2025-01-01'),
          (2, 'bob', 'bob@example.com', 'active', '2025-01-01'),
          (3, 'mod', 'Mod@Example.com', 'active', '2025-01-01');
        INSERT INTO legacy.authors (id, document_id, name) VALUES (11, 'a1', 'alice'), (12, 'a2', 'bob');
        INSERT INTO legacy.up_users_author_lnk (id, user_id, author_id) VALUES (1, 1, 11), (2, 2, 12);
        INSERT INTO legacy.articles (id, document_id, title, text, published_at, created_at) VALUES
          (10, 'p1', '帖子', '正文', '2025-02-01', '2025-02-01');
        INSERT INTO legacy.articles_author_lnk (id, article_id, author_id) VALUES (1, 10, 11);
        INSERT INTO legacy.comments (id, document_id, content, is_anonymous, anonymous_seed, created_at) VALUES
          (100, 'c1', '实名评论', false, NULL, '2025-02-02'),
          (101, 'c2', '匿名评论', true, 'seed', '2025-02-02');
        INSERT INTO legacy.comments_article_lnk (id, comment_id, article_id) VALUES (1, 100, 10), (2, 101, 10);
        INSERT INTO legacy.comments_author_lnk (id, comment_id, author_id) VALUES (1, 100, 12), (2, 101, 12);

        INSERT INTO legacy.notifications (id, type, message, is_read, bucket_key, created_at) VALUES
          (1, 'comment', NULL, false, 'contacts:2', '2025-02-02'),
          (2, 'comment', NULL, true, 'anonymous:seed', '2025-02-02'),
          (3, 'system', '你的举报已处理', false, NULL, '2025-02-03'),
          (4, 'like', NULL, false, 'contacts:1', '2025-02-03'),
          (5, 'weird', NULL, false, NULL, '2025-02-03');
        INSERT INTO legacy.notifications_recipient_lnk (id, notification_id, user_id) VALUES
          (1, 1, 1), (2, 2, 1), (3, 3, 2), (4, 4, 1), (5, 5, 1);
        INSERT INTO legacy.notifications_sender_lnk (id, notification_id, user_id) VALUES
          (1, 1, 2), (2, 2, 2), (3, 4, 1), (4, 5, 2);
        INSERT INTO legacy.notifications_article_lnk (id, notification_id, article_id) VALUES (1, 1, 10), (2, 2, 10);
        INSERT INTO legacy.notifications_comment_lnk (id, notification_id, comment_id) VALUES (1, 1, 100), (2, 2, 101);

        INSERT INTO legacy.admin_users (id, username, email) VALUES (1, 'moderator', 'mod@example.com'),
                                                                     (2, 'ghost', 'ghost@example.com');
        INSERT INTO legacy.reports (id, target_type, reason, detail, report_status, resolution, handled_by,
                                    handled_at, assigned_at, snapshot, created_at, updated_at) VALUES
          (1, 'article', 'spam', NULL, 'accepted', NULL, 'mod@example.com', NULL, NULL,
           '{"title":"帖子"}', '2025-02-04', '2025-02-05'),
          (2, 'comment', 'other', NULL, 'reviewing', NULL, NULL, NULL, '2025-02-04', NULL, '2025-02-04', '2025-02-04'),
          (3, 'user', 'abuse', 'x', 'reviewing', NULL, NULL, NULL, NULL, NULL, '2025-02-04', '2025-02-04');
        INSERT INTO legacy.reports_reporter_lnk (id, report_id, user_id) VALUES (1, 1, 2), (2, 2, 1), (3, 3, 1);
        INSERT INTO legacy.reports_article_lnk (id, report_id, article_id) VALUES (1, 1, 10);
        INSERT INTO legacy.reports_comment_lnk (id, report_id, comment_id) VALUES (1, 2, 101);
        INSERT INTO legacy.reports_target_user_lnk (id, report_id, user_id) VALUES (1, 3, 2);
        INSERT INTO legacy.reports_assigned_to_lnk (id, report_id, user_id) VALUES (1, 2, 1);
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
    assert_eq!(count("notifications.anonymous_as_system"), 1);

    import(&db, "UTC").await;

    let notes = sqlx::query!(
        "SELECT id, recipient_id, sender_id, kind, bucket_key, post_id, comment_id, post_title, excerpt, is_read
         FROM notifications ORDER BY id"
    )
    .fetch_all(&db)
    .await
    .unwrap();
    let ids: Vec<i64> = notes.iter().map(|n| n.id).collect();
    assert_eq!(ids, vec![1, 2, 3], "自己给自己、不认识的类型不导入");
    assert_eq!(
        (
            notes[0].sender_id,
            notes[0].kind.as_str(),
            notes[0].bucket_key.as_str()
        ),
        (Some(2), "comment", "user-2")
    );
    assert_eq!(
        (notes[0].post_id, notes[0].comment_id),
        (Some(10), Some(100))
    );
    assert_eq!(notes[0].post_title.as_deref(), Some("帖子"));
    assert_eq!(notes[0].excerpt, "实名评论");
    // 匿名评论的通知：不能暴露发送者。
    assert_eq!(
        (
            notes[1].sender_id,
            notes[1].kind.as_str(),
            notes[1].bucket_key.as_str()
        ),
        (None, "system", "system")
    );
    assert_eq!(notes[1].excerpt, "有人匿名评论了你的帖子：匿名评论");
    assert!(notes[1].is_read);
    assert_eq!(
        (notes[2].kind.as_str(), notes[2].excerpt.as_str()),
        ("system", "你的举报已处理")
    );

    let reports = sqlx::query!(
        "SELECT id, reporter_id, target_type, target_id, owner_user_id, detail, status, resolution,
                assigned_to, handled_by, handled_at, snapshot
         FROM reports ORDER BY id"
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(reports.len(), 3);
    let r1 = &reports[0];
    assert_eq!(
        (r1.target_type.as_str(), r1.target_id, r1.owner_user_id),
        ("post", 10, Some(1))
    );
    assert_eq!(
        (r1.status.as_str(), r1.resolution.as_deref()),
        ("accepted", Some("none"))
    );
    assert_eq!(r1.handled_by, Some(3), "按邮箱对应到论坛账号");
    assert!(r1.handled_at.is_some());
    let r2 = &reports[1];
    assert_eq!(
        (r2.target_type.as_str(), r2.target_id, r2.owner_user_id),
        ("comment", 101, Some(2))
    );
    assert_eq!(r2.detail.as_deref(), Some("（未填写）"));
    assert_eq!((r2.status.as_str(), r2.assigned_to), ("reviewing", Some(3)));
    assert_eq!(r2.snapshot, serde_json::json!({}));
    assert_eq!(reports[2].status, "pending", "复核中却没人领取的退回待处理");
}
