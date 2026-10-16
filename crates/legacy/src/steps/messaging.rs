//! 私信：会话（一对一私聊、AI 会话）、成员、消息、AI 会话状态。

use sqlx::{PgConnection, Postgres, Transaction};

use crate::import::{ImportOptions, ImportReport};

pub async fn run(
    tx: &mut Transaction<'_, Postgres>,
    options: &ImportOptions,
    report: &mut ImportReport,
) -> anyhow::Result<()> {
    report.add("conversations", conversations(tx, options).await?);
    report.add("conversation_members", members(tx, options).await?);
    report.add("messages", messages(tx, options).await?);
    report.add("messages.reply_to", replies(tx).await?);
    report.add("ai_sessions", ai_sessions(tx, options).await?);
    Ok(())
}

/// 普通私聊只建存活的那条（同一对用户只留一条）；双方账号都要存在。
async fn conversations(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO conversations (id, kind, user_low, user_high, created_at)
           SELECT c.id, CASE WHEN c.is_ai THEN 'ai' ELSE 'direct' END, c.user_low, c.user_high,
                  coalesce(c.created_at AT TIME ZONE $1, $2)
           FROM legacy.ik_conversations c
           JOIN users lo ON lo.id = c.user_low
           JOIN users hi ON hi.id = c.user_high
           WHERE c.id = c.survivor_id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 每个会话两行（各自的视角）。合并过的私聊：已读水位取最晚、免打扰 / 置顶任一为真即为真、
/// 只有每条都「已移除」才算移除；排序时间取各会话最后一条消息的最晚时间。
async fn members(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO conversation_members (conversation_id, user_id, peer_id, activity_at,
                                             last_read_at, muted, pinned_at, left_at, created_at)
           SELECT nc.id, m.user_id,
                  CASE WHEN m.user_id = nc.user_low THEN nc.user_high ELSE nc.user_low END,
                  max(coalesce(c.last_message_at, c.created_at) AT TIME ZONE $1),
                  max(m.last_read_at AT TIME ZONE $1),
                  bool_or(coalesce(m.muted, false)),
                  max(coalesce(m.updated_at, m.created_at) AT TIME ZONE $1) FILTER (WHERE m.pinned),
                  CASE WHEN bool_and(m.left_at IS NOT NULL) THEN max(m.left_at AT TIME ZONE $1) END,
                  min(coalesce(m.created_at AT TIME ZONE $1, $2))
           FROM legacy.ik_conversations c
           JOIN conversations nc ON nc.id = c.survivor_id
           JOIN legacy.ik_conversation_members m ON m.conversation_id = c.id
           WHERE m.user_id IN (nc.user_low, nc.user_high)
           GROUP BY nc.id, nc.user_low, nc.user_high, m.user_id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 发送者必须是会话双方之一；正文为空的不导入。撤回（deletedAt）保留行、正文不可见（MSG-003）。
/// AI 发的消息记为已定稿（`done`）；旧的工作流时间线格式不同，不导入。
async fn messages(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO messages (id, conversation_id, sender_id, kind, content, edited_at,
                                 withdrawn_at, created_at, ai_state, ai_model_key)
           SELECT m.id, nc.id, m.sender_id,
                  CASE WHEN m.kind = 'system' THEN 'system' ELSE 'text' END,
                  m.new_content,
                  m.edited_at AT TIME ZONE $1,
                  m.deleted_at AT TIME ZONE $1,
                  coalesce(m.created_at AT TIME ZONE $1, $2),
                  CASE WHEN nc.kind = 'ai' AND m.sender_id = ai.id THEN 'done' END,
                  CASE WHEN nc.kind = 'ai' AND m.sender_id = ai.id
                       THEN nullif(left(btrim(m.ai_model_key), 64), '') END
           FROM legacy.ik_messages m
           JOIN conversations nc ON nc.id = m.conversation_id
           LEFT JOIN users ai ON nc.kind = 'ai' AND ai.id IN (nc.user_low, nc.user_high) AND ai.is_ai_agent
           WHERE m.sender_id IN (nc.user_low, nc.user_high)
             AND char_length(coalesce(m.new_content, '')) >= 1"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 引用回复只认同一会话里导入了的消息。
async fn replies(conn: &mut PgConnection) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"UPDATE messages nm SET reply_to_id = target.id
           FROM legacy.ik_messages m
           JOIN messages target ON target.id = m.reply_to_legacy_id
           WHERE nm.id = m.id AND target.conversation_id = nm.conversation_id"#
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// AI 会话状态：代际沿用旧值。重置过（代际 > 0）的会话，上下文从重置后的第一条消息开始，
/// 找不到时从导入时刻开始——绝不能让 AI 重新读到重置之前的私密历史（CTX-023）。
async fn ai_sessions(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO ai_sessions (conversation_id, user_id, ai_user_id, context_epoch, context_from,
                                    model_key, created_at)
           SELECT nc.id, c.human_id, c.ai_user_id, greatest(coalesce(c.context_epoch, 0), 0),
                  CASE WHEN coalesce(c.context_epoch, 0) > 0 THEN coalesce(
                      (SELECT min(m.created_at) AT TIME ZONE $1 FROM legacy.ik_messages m
                       WHERE m.conversation_id = c.id
                         AND coalesce(m.generation_epoch, 0) >= c.context_epoch),
                      $2) END,
                  (SELECT nullif(btrim(am.key), '') FROM legacy.conversations_ai_model_lnk l
                   JOIN legacy.ai_models am ON am.id = l.ai_model_id
                   WHERE l.conversation_id = c.id ORDER BY l.id LIMIT 1),
                  nc.created_at
           FROM legacy.ik_conversations c
           JOIN conversations nc ON nc.id = c.id AND nc.kind = 'ai'"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}
