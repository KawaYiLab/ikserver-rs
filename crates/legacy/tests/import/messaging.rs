//! 私信导入：重复私聊合并、群聊与残缺会话跳过、图片消息（转成消息图片或图片地址）、撤回、引用、
//! AI 会话与重置后的上下文。

use crate::common::{at, import, setup};
use sqlx::PgPool;

use crate::MIGRATOR;

#[sqlx::test(migrator = "MIGRATOR")]
async fn conversations_and_messages(db: PgPool) {
    setup(&db).await;
    sqlx::raw_sql(
        r#"
        INSERT INTO legacy.up_users (id, username, account_status, is_ai_agent, created_at) VALUES
          (1, 'alice', 'active', false, '2025-01-01'), (2, 'bob', 'active', false, '2025-01-01'),
          (3, 'fairy', 'active', true, '2025-01-01'), (4, 'carol', 'active', false, '2025-01-01');
        INSERT INTO legacy.ai_models (id, key) VALUES (1, 'deepseek');

        -- 10、11：alice 与 bob 的重复私聊；12：alice 与 AI（重置过一次）；13：群聊；14：只有一个成员。
        INSERT INTO legacy.conversations (id, kind, context_epoch, last_message_at, created_at) VALUES
          (10, 'direct', 0, '2025-03-01 10:00:00', '2025-03-01 00:00:00'),
          (11, 'direct', 0, '2025-03-02 10:00:00', '2025-03-02 00:00:00'),
          (12, 'direct', 1, '2025-03-03 10:00:00', '2025-03-03 00:00:00'),
          (13, 'group', 0, NULL, '2025-03-01 00:00:00'),
          (14, 'direct', 0, NULL, '2025-03-01 00:00:00');
        INSERT INTO legacy.conversations_ai_model_lnk (id, conversation_id, ai_model_id) VALUES (1, 12, 1);
        INSERT INTO legacy.conversation_members (id, last_read_at, muted, pinned, left_at, created_at, updated_at) VALUES
          (1, '2025-03-01 09:00:00', false, true, NULL, '2025-03-01 00:00:00', '2025-03-01 05:00:00'),
          (2, NULL, true, false, NULL, '2025-03-01 00:00:00', '2025-03-01 00:00:00'),
          (3, '2025-03-02 09:00:00', false, false, NULL, '2025-03-02 00:00:00', '2025-03-02 00:00:00'),
          (4, NULL, false, false, '2025-03-02 11:00:00', '2025-03-02 00:00:00', '2025-03-02 00:00:00'),
          (5, NULL, false, false, NULL, '2025-03-03 00:00:00', '2025-03-03 00:00:00'),
          (6, NULL, false, false, NULL, '2025-03-03 00:00:00', '2025-03-03 00:00:00'),
          (7, NULL, false, false, NULL, '2025-03-01 00:00:00', '2025-03-01 00:00:00'),
          (8, NULL, false, false, NULL, '2025-03-01 00:00:00', '2025-03-01 00:00:00'),
          (9, NULL, false, false, NULL, '2025-03-01 00:00:00', '2025-03-01 00:00:00'),
          (10, NULL, false, false, NULL, '2025-03-01 00:00:00', '2025-03-01 00:00:00');
        INSERT INTO legacy.conversation_members_conversation_lnk (id, conversation_member_id, conversation_id) VALUES
          (1, 1, 10), (2, 2, 10), (3, 3, 11), (4, 4, 11), (5, 5, 12), (6, 6, 12),
          (7, 7, 13), (8, 8, 13), (9, 9, 13), (10, 10, 14);
        INSERT INTO legacy.conversation_members_user_lnk (id, conversation_member_id, user_id) VALUES
          (1, 1, 1), (2, 2, 2), (3, 3, 1), (4, 4, 2), (5, 5, 1), (6, 6, 3),
          (7, 7, 1), (8, 8, 2), (9, 9, 4), (10, 10, 1);

        -- 50 读不到（只能写地址）；51～56 摘要读取成功（能转成消息图片）。
        INSERT INTO legacy.files (id, url) VALUES
          (50, 'https://image.tiwat.cn/dm/pic.png'),
          (51, 'https://image.tiwat.cn/dm/a.png'), (52, 'https://image.tiwat.cn/dm/b.png'),
          (53, 'https://image.tiwat.cn/dm/c.png'), (54, 'https://image.tiwat.cn/dm/d.png'),
          (55, 'https://image.tiwat.cn/dm/e.png'), (56, 'https://image.tiwat.cn/dm/f.png');
        INSERT INTO legacy.ik_media_digests (file_id, error) VALUES (50, '不是支持的图片类型');
        INSERT INTO legacy.ik_media_digests (file_id, object_key, sha256, mime, size_bytes) VALUES
          (51, 'dm/a.png', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'image/png', 100), (52, 'dm/b.png', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'image/png', 100),
          (53, 'dm/c.png', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'image/png', 100), (54, 'dm/d.png', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'image/png', 100),
          (55, 'dm/e.png', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'image/png', 100), (56, 'dm/f.png', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'image/png', 100);
        -- 102：只有读不到的附件；110：有字、两张图（一张读不到）；111：只有图，6 张（超出 4 张的写地址）。
        INSERT INTO legacy.files_related_mph (id, file_id, related_id, related_type, field, "order") VALUES
          (1, 50, 102, 'api::message.message', 'attachments', 1),
          (2, 50, 110, 'api::message.message', 'attachments', 1),
          (3, 51, 110, 'api::message.message', 'attachments', 2),
          (4, 56, 111, 'api::message.message', 'attachments', 6),
          (5, 52, 111, 'api::message.message', 'attachments', 1),
          (6, 53, 111, 'api::message.message', 'attachments', 2),
          (7, 54, 111, 'api::message.message', 'attachments', 3),
          (8, 55, 111, 'api::message.message', 'attachments', 4),
          (9, 51, 111, 'api::message.message', 'attachments', 5);
        INSERT INTO legacy.messages (id, content, kind, generation_epoch, ai_model_key, deleted_at, created_at) VALUES
          (100, '你好', 'text', 0, NULL, NULL, '2025-03-01 10:00:00'),
          (101, '收到', 'text', 0, NULL, NULL, '2025-03-02 10:00:00'),
          (102, '', 'image', 0, NULL, NULL, '2025-03-01 10:05:00'),
          (103, '我不是成员', 'text', 0, NULL, NULL, '2025-03-01 10:06:00'),
          (104, '重置前的秘密', 'text', 0, NULL, NULL, '2025-03-03 09:00:00'),
          (105, '重置后', 'text', 1, NULL, NULL, '2025-03-03 10:00:00'),
          (106, '回答', 'text', 1, 'deepseek', NULL, '2025-03-03 10:00:05'),
          (107, '撤回了', 'text', 0, NULL, '2025-03-01 10:10:00', '2025-03-01 10:09:00'),
          (108, '   ', 'text', 0, NULL, NULL, '2025-03-01 10:11:00'),
          (109, '群里说话', 'text', 0, NULL, NULL, '2025-03-01 10:12:00'),
          (110, '看图', 'image', 0, NULL, NULL, '2025-03-01 10:13:00'),
          (111, NULL, 'image', 0, NULL, NULL, '2025-03-01 10:14:00');
        INSERT INTO legacy.messages_conversation_lnk (id, message_id, conversation_id) VALUES
          (1, 100, 10), (2, 101, 11), (3, 102, 10), (4, 103, 10), (5, 104, 12), (6, 105, 12),
          (7, 106, 12), (8, 107, 10), (9, 108, 10), (10, 109, 13), (11, 110, 10), (12, 111, 10);
        INSERT INTO legacy.messages_sender_lnk (id, message_id, user_id) VALUES
          (1, 100, 1), (2, 101, 2), (3, 102, 1), (4, 103, 4), (5, 104, 1), (6, 105, 1),
          (7, 106, 3), (8, 107, 2), (9, 108, 1), (10, 109, 4), (11, 110, 1), (12, 111, 1);
        INSERT INTO legacy.messages_reply_to_lnk (id, message_id, inv_message_id) VALUES (1, 101, 100);
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
    assert_eq!(count("messaging.conversations_skipped"), 2);
    assert_eq!(count("messaging.conversations_merged"), 1);
    assert_eq!(count("messaging.image_messages"), 3);
    assert_eq!(count("messaging.empty_messages"), 1);

    import(&db, "UTC").await;

    let conversations =
        sqlx::query!("SELECT id, kind, user_low, user_high FROM conversations ORDER BY id")
            .fetch_all(&db)
            .await
            .unwrap();
    let conversations: Vec<(i64, &str, i64, i64)> = conversations
        .iter()
        .map(|c| (c.id, c.kind.as_str(), c.user_low, c.user_high))
        .collect();
    assert_eq!(conversations, vec![(10, "direct", 1, 2), (12, "ai", 1, 3)]);

    // 合并后的成员视角：已读取最晚、置顶 / 免打扰任一为真、只在一条里移除的不算移除。
    let members = sqlx::query!(
        "SELECT user_id, peer_id, activity_at, last_read_at, muted, pinned_at, left_at
         FROM conversation_members WHERE conversation_id = 10 ORDER BY user_id"
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!((members[0].user_id, members[0].peer_id), (1, 2));
    assert_eq!(members[0].activity_at, at(2025, 3, 2, 10, 0));
    assert_eq!(members[0].last_read_at, Some(at(2025, 3, 2, 9, 0)));
    assert_eq!(members[0].pinned_at, Some(at(2025, 3, 1, 5, 0)));
    assert!(members[1].muted);
    assert_eq!(members[1].left_at, None);

    let messages = sqlx::query!(
        "SELECT id, conversation_id, sender_id, kind, content, reply_to_id, withdrawn_at, ai_state, ai_model_key,
                image_ids
         FROM messages ORDER BY id"
    )
    .fetch_all(&db)
    .await
    .unwrap();
    let ids: Vec<i64> = messages.iter().map(|m| m.id).collect();
    assert_eq!(
        ids,
        vec![100, 101, 102, 104, 105, 106, 107, 110, 111],
        "非成员、空正文、群聊的消息不导入"
    );
    assert_eq!(
        (messages[1].conversation_id, messages[1].reply_to_id),
        (10, Some(100)),
        "并进存活的会话"
    );
    assert_eq!(
        (
            messages[2].content.as_str(),
            messages[2].image_ids.as_slice()
        ),
        ("https://im.tiwat.cn/dm/pic.png", &[][..]),
        "读不到的图片改为图片地址"
    );
    assert_eq!(
        (
            messages[7].content.as_str(),
            messages[7].image_ids.as_slice()
        ),
        ("看图\nhttps://im.tiwat.cn/dm/pic.png", &[51][..]),
        "读得到的转成消息图片，读不到的地址接在正文后"
    );
    assert_eq!(
        (
            messages[8].content.as_str(),
            messages[8].image_ids.as_slice()
        ),
        (
            "https://im.tiwat.cn/dm/a.png\nhttps://im.tiwat.cn/dm/f.png",
            &[52, 53, 54, 55][..]
        ),
        "按顺序前 4 张转成图片，超出的写地址"
    );
    assert_eq!(messages[6].withdrawn_at, Some(at(2025, 3, 1, 10, 10)));
    assert_eq!(messages[5].ai_state.as_deref(), Some("done"));
    assert_eq!(messages[5].ai_model_key.as_deref(), Some("deepseek"));
    assert_eq!(messages[4].ai_state, None, "人发的消息不带 AI 状态");

    let session = sqlx::query!(
        "SELECT conversation_id, user_id, ai_user_id, context_epoch, context_from, model_key FROM ai_sessions"
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(
        (session.conversation_id, session.user_id, session.ai_user_id),
        (12, 1, 3)
    );
    assert_eq!(session.context_epoch, 1);
    assert_eq!(
        session.context_from,
        Some(at(2025, 3, 3, 10, 0)),
        "上下文从重置后的第一条开始，读不到重置前的消息"
    );
    assert_eq!(session.model_key.as_deref(), Some("deepseek"));
}
