//! 题库、装扮与装备、米游社绑定。

mod common;

use common::{import, setup};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[sqlx::test(migrator = "MIGRATOR")]
async fn catalog_is_imported(db: PgPool) {
    setup(&db).await;
    sqlx::raw_sql(
        r#"
        INSERT INTO legacy.up_users (id, username, account_status, blocked, created_at) VALUES
          (1, 'a', 'active', false, '2025-01-01'), (2, 'b', 'deleted', true, '2025-01-01'),
          (3, 'c', 'active', false, '2025-01-01');

        INSERT INTO legacy.exam_questions (id, document_id, question, type, options, correct_keys, weight, enabled) VALUES
          (1, 'q1', ' 1+1? ', 'single', '[{"key":" A ","text":"2"},{"key":"B","text":"3"},{"key":"A","text":"dup"},{"key":"C","text":" "}]', '["A","Z"]', 0, NULL),
          (2, 'q2', '坏题', 'single', '[{"key":"A","text":"只有一个"}]', '["A"]', 1, true),
          (3, 'q3', '没答案', 'weird', '[{"key":"A","text":"x"},{"key":"B","text":"y"}]', '["Q"]', 2, false);

        INSERT INTO legacy.files (id, name, url) VALUES (1, 'a.png', 'x'), (2, 'b.png', 'y');
        INSERT INTO legacy.ik_media_digests (file_id, object_key, sha256, mime, size_bytes) VALUES
          (1, 'k/a.png', repeat('a', 64), 'image/png', 1), (2, 'k/b.png', repeat('b', 64), 'image/png', 1);
        INSERT INTO legacy.avatars (id, name, type) VALUES (5, '妮可', 'character'), (6, '无图', 'city');
        INSERT INTO legacy.business_cards (id, name, description, story, type) VALUES
          (7, '新艾利都', '城市', '[{"type":"paragraph","children":[{"type":"text","text":"第一段"}]},
                                  {"type":"paragraph","children":[{"type":"text","text":"第二段"}]}]', 'mystery');
        INSERT INTO legacy.files_related_mph (id, file_id, related_id, related_type, field, "order") VALUES
          (1, 1, 5, 'api::avatar.avatar', 'image', 1),
          (2, 2, 7, 'api::business-card.business-card', 'image', 1);
        INSERT INTO legacy.up_users_equipped_avatar_lnk (id, user_id, avatar_id) VALUES (1, 1, 5), (2, 3, 6);
        INSERT INTO legacy.up_users_equipped_card_lnk (id, user_id, business_card_id) VALUES (1, 1, 7);

        INSERT INTO legacy.mihoyo_bindings (id, aid, mid, zzz_uid, created_at) VALUES
          (1, '1001', 'm1', 'z1', '2025-01-01'), (2, '1002', 'm2', NULL, '2025-01-02'),
          (3, '1001', 'm3', NULL, '2025-01-03');
        INSERT INTO legacy.mihoyo_bindings_user_lnk (id, mihoyo_binding_id, user_id) VALUES
          (1, 1, 1), (2, 2, 2), (3, 3, 3);
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
    assert_eq!(count("exam.invalid_questions"), 2);
    assert_eq!(count("cosmetics.no_image"), 1);

    import(&db, "UTC").await;

    let q = sqlx::query!(
        "SELECT id, question, kind, options, correct_keys, weight, enabled, legacy_id FROM exam_questions"
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(
        (q[0].id, q[0].question.as_str(), q[0].kind.as_str()),
        (1, "1+1?", "single")
    );
    assert_eq!(
        q[0].options,
        serde_json::json!([{ "key": "A", "text": "2" }, { "key": "B", "text": "3" }]),
        "trim、key 去重、去掉空选项"
    );
    assert_eq!(
        q[0].correct_keys,
        vec!["A".to_owned()],
        "只留选项里有的 key"
    );
    assert_eq!((q[0].weight, q[0].enabled), (1, true));

    let avatars: Vec<i64> = sqlx::query_scalar!("SELECT id FROM system_avatars")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(avatars, vec![5]);
    let card = sqlx::query!("SELECT kind, story, image_media_id FROM business_cards")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(
        (card.kind.as_str(), card.story.as_str(), card.image_media_id),
        ("character", "第一段\n\n第二段", 2)
    );

    let equipped = sqlx::query!(
        "SELECT id, equipped_avatar_id, equipped_avatar_media_id, equipped_card_id FROM users ORDER BY id"
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(
        (
            equipped[0].equipped_avatar_id,
            equipped[0].equipped_avatar_media_id,
            equipped[0].equipped_card_id
        ),
        (Some(5), Some(1), Some(7))
    );
    assert_eq!(
        equipped[2].equipped_avatar_id, None,
        "装备的头像没导入就卸下"
    );

    // 1001 被 1 号与 3 号同时绑定：只留最早的；2 号已注销不带绑定。
    let bindings = sqlx::query!("SELECT user_id, aid, mid FROM mihoyo_bindings ORDER BY user_id")
        .fetch_all(&db)
        .await
        .unwrap();
    let bindings: Vec<(i64, &str)> = bindings
        .iter()
        .map(|b| (b.user_id, b.aid.as_str()))
        .collect();
    assert_eq!(bindings, vec![(1, "1001")]);
}
