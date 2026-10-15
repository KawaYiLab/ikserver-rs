//! SQL。只被本 crate 的 service / interop 调用。
//!
//! 游标边界统一用三个参数表达「严格早于游标」：`at`、`key`、`inclusive`。
//! `inclusive = true` 时与 `at` 同一时刻的行全部算在游标之后（游标落在另一个数据源的项上，
//! 同一时刻那个数据源排在前面，见 `crate::interop::Before`）。

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

// ---------- 会话与成员 ----------

/// 建私聊会话（已存在则什么都不做），返回会话 id。
pub(crate) async fn upsert_direct(
    conn: &mut PgConnection,
    id: i64,
    low: i64,
    high: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<i64> {
    sqlx::query!(
        "INSERT INTO conversations (id, kind, user_low, user_high, created_at)
         VALUES ($1, 'direct', $2, $3, $4)
         ON CONFLICT (user_low, user_high) WHERE kind = 'direct' DO NOTHING",
        id,
        low,
        high,
        now,
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query_scalar!(
        "SELECT id FROM conversations WHERE user_low = $1 AND user_high = $2 AND kind = 'direct'",
        low,
        high,
    )
    .fetch_one(conn)
    .await
}

/// 建一条 AI 会话（每次新建，CONV-012）。
pub(crate) async fn insert_ai_conversation(
    conn: &mut PgConnection,
    id: i64,
    low: i64,
    high: i64,
    title: Option<&str>,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO conversations (id, kind, user_low, user_high, title, created_at)
         VALUES ($1, 'ai', $2, $3, $4, $5)",
        id,
        low,
        high,
        title,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn set_title(
    conn: &mut PgConnection,
    conversation_id: i64,
    title: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE conversations SET title = $2 WHERE id = $1 AND kind = 'ai'",
        conversation_id,
        title,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 会话类型（`direct` / `ai`）。
pub(crate) async fn conversation_kind(
    conn: &mut PgConnection,
    conversation_id: i64,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar!(
        "SELECT kind FROM conversations WHERE id = $1",
        conversation_id
    )
    .fetch_optional(conn)
    .await
}

/// 加入成员（已存在则不动）。新成员的排序键为 `now`；`hidden` 时先不出现在列表里，
/// 等第一条消息到达（[`bump_members`]）。
pub(crate) async fn insert_member(
    conn: &mut PgConnection,
    conversation_id: i64,
    user_id: i64,
    peer_id: i64,
    now: DateTime<Utc>,
    hidden: bool,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO conversation_members (conversation_id, user_id, peer_id, activity_at,
                                          left_at, created_at)
         VALUES ($1, $2, $3, $4::timestamptz, CASE WHEN $5 THEN $4::timestamptz END, $4::timestamptz)
         ON CONFLICT DO NOTHING",
        conversation_id,
        user_id,
        peer_id,
        now,
        hidden,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 自己重新发起已移除的会话：恢复到列表，排到最前。
pub(crate) async fn rejoin(
    conn: &mut PgConnection,
    conversation_id: i64,
    user_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE conversation_members
         SET left_at = NULL, activity_at = GREATEST(activity_at, $3)
         WHERE conversation_id = $1 AND user_id = $2 AND left_at IS NOT NULL",
        conversation_id,
        user_id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 在场成员身份（`left_at IS NULL`），返回对方 id。
pub(crate) async fn active_peer(
    conn: &mut PgConnection,
    conversation_id: i64,
    user_id: i64,
) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar!(
        "SELECT peer_id FROM conversation_members
         WHERE conversation_id = $1 AND user_id = $2 AND left_at IS NULL",
        conversation_id,
        user_id,
    )
    .fetch_optional(conn)
    .await
}

/// 新消息：推进全部成员的排序键，并让移除了会话的成员重新看到它。返回在场成员。
pub(crate) async fn bump_members(
    conn: &mut PgConnection,
    conversation_id: i64,
    at: DateTime<Utc>,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "UPDATE conversation_members
         SET activity_at = GREATEST(activity_at, $2), left_at = NULL
         WHERE conversation_id = $1
         RETURNING user_id",
        conversation_id,
        at,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn active_member_ids(
    conn: &mut PgConnection,
    conversation_id: i64,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT user_id FROM conversation_members WHERE conversation_id = $1 AND left_at IS NULL",
        conversation_id,
    )
    .fetch_all(conn)
    .await
}

/// 对方发来的通知：只推进收件人自己的排序键，不恢复已移除的会话。返回会话 id。
pub(crate) async fn touch_by_peer(
    conn: &mut PgConnection,
    user_id: i64,
    peer_id: i64,
    at: DateTime<Utc>,
) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar!(
        "UPDATE conversation_members m SET activity_at = GREATEST(m.activity_at, $3)
         FROM conversations c
         WHERE m.user_id = $1 AND m.peer_id = $2 AND m.left_at IS NULL
           AND c.id = m.conversation_id AND c.kind = 'direct'
         RETURNING m.conversation_id",
        user_id,
        peer_id,
        at,
    )
    .fetch_optional(conn)
    .await
}

/// 所有在场私聊的对方（含置顶）。
pub(crate) async fn active_peers(conn: &mut PgConnection, user_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT m.peer_id FROM conversation_members m
         JOIN conversations c ON c.id = m.conversation_id AND c.kind = 'direct'
         WHERE m.user_id = $1 AND m.left_at IS NULL",
        user_id,
    )
    .fetch_all(conn)
    .await
}

/// 推进已读水位（只前进），返回对方 id；不是在场成员时返回 None。
pub(crate) async fn mark_read(
    conn: &mut PgConnection,
    conversation_id: i64,
    user_id: i64,
    at: DateTime<Utc>,
) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar!(
        "UPDATE conversation_members
         SET last_read_at = GREATEST(COALESCE(last_read_at, $3), $3)
         WHERE conversation_id = $1 AND user_id = $2 AND left_at IS NULL
         RETURNING peer_id",
        conversation_id,
        user_id,
        at,
    )
    .fetch_optional(conn)
    .await
}

/// 全部在场会话标为已读，返回水位真正前进的会话数。
pub(crate) async fn mark_all_read(
    conn: &mut PgConnection,
    user_id: i64,
    at: DateTime<Utc>,
) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        "UPDATE conversation_members SET last_read_at = $2
         WHERE user_id = $1 AND left_at IS NULL AND (last_read_at IS NULL OR last_read_at < $2)",
        user_id,
        at,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

pub(crate) async fn set_muted(
    conn: &mut PgConnection,
    conversation_id: i64,
    user_id: i64,
    muted: bool,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE conversation_members SET muted = $3
         WHERE conversation_id = $1 AND user_id = $2 AND left_at IS NULL",
        conversation_id,
        user_id,
        muted,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 置顶 / 取消置顶。已置顶的再置顶不改时间（保持原有顺序）。
pub(crate) async fn set_pinned(
    conn: &mut PgConnection,
    conversation_id: i64,
    user_id: i64,
    pinned: bool,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE conversation_members
         SET pinned_at = CASE WHEN $3 THEN COALESCE(pinned_at, $4) END
         WHERE conversation_id = $1 AND user_id = $2 AND left_at IS NULL",
        conversation_id,
        user_id,
        pinned,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 置顶的会话数（不含 `except` 这一个）。
pub(crate) async fn pinned_count(
    conn: &mut PgConnection,
    user_id: i64,
    except: i64,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM conversation_members
           WHERE user_id = $1 AND left_at IS NULL AND pinned_at IS NOT NULL
             AND conversation_id <> $2"#,
        user_id,
        except,
    )
    .fetch_one(conn)
    .await
}

/// 从自己的列表移除。顺带取消置顶、把水位推到现在（移除后不再计未读）。
pub(crate) async fn leave(
    conn: &mut PgConnection,
    conversation_id: i64,
    user_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE conversation_members
         SET left_at = $3, pinned_at = NULL, last_read_at = GREATEST(COALESCE(last_read_at, $3), $3)
         WHERE conversation_id = $1 AND user_id = $2 AND left_at IS NULL",
        conversation_id,
        user_id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

// ---------- 会话摘要 ----------

pub(crate) struct SummaryRow {
    pub conversation_id: i64,
    pub conv_kind: String,
    pub title: Option<String>,
    pub peer_id: i64,
    pub activity_at: DateTime<Utc>,
    pub last_read_at: Option<DateTime<Utc>>,
    pub muted: bool,
    pub pinned_at: Option<DateTime<Utc>>,
    pub unread: i64,
    pub last_id: Option<i64>,
    pub last_sender_id: Option<i64>,
    pub last_kind: Option<String>,
    pub last_content: Option<String>,
    pub last_withdrawn_at: Option<DateTime<Utc>>,
    pub last_created_at: Option<DateTime<Utc>>,
}

/// 一个在场会话的摘要。
pub(crate) async fn summary(
    conn: &mut PgConnection,
    user_id: i64,
    conversation_id: i64,
) -> sqlx::Result<Option<SummaryRow>> {
    sqlx::query_as!(
        SummaryRow,
        r#"SELECT m.conversation_id, cv.kind AS conv_kind, cv.title, m.peer_id, m.activity_at, m.last_read_at, m.muted, m.pinned_at,
                  (SELECT count(*) FROM messages x
                   WHERE x.conversation_id = m.conversation_id AND x.sender_id <> m.user_id
                     AND x.withdrawn_at IS NULL
                     AND (m.last_read_at IS NULL OR x.created_at > m.last_read_at)) AS "unread!",
                  l.id AS "last_id?", l.sender_id AS "last_sender_id?", l.kind AS "last_kind?",
                  l.content AS "last_content?", l.withdrawn_at AS "last_withdrawn_at?",
                  l.created_at AS "last_created_at?"
           FROM conversation_members m
           LEFT JOIN LATERAL (
               SELECT x.id, x.sender_id, x.kind, x.content, x.withdrawn_at, x.created_at
               FROM messages x WHERE x.conversation_id = m.conversation_id
               ORDER BY x.created_at DESC, x.id DESC LIMIT 1
           ) l ON TRUE
           JOIN conversations cv ON cv.id = m.conversation_id
           WHERE m.user_id = $1 AND m.conversation_id = $2 AND m.left_at IS NULL"#,
        user_id,
        conversation_id,
    )
    .fetch_optional(conn)
    .await
}

/// 全部置顶会话（不分页、不设上限：历史超限的置顶也要全部可见，CONV-023）。
pub(crate) async fn pinned_summaries(
    conn: &mut PgConnection,
    user_id: i64,
) -> sqlx::Result<Vec<SummaryRow>> {
    sqlx::query_as!(
        SummaryRow,
        r#"SELECT m.conversation_id, cv.kind AS conv_kind, cv.title, m.peer_id, m.activity_at, m.last_read_at, m.muted, m.pinned_at,
                  (SELECT count(*) FROM messages x
                   WHERE x.conversation_id = m.conversation_id AND x.sender_id <> m.user_id
                     AND x.withdrawn_at IS NULL
                     AND (m.last_read_at IS NULL OR x.created_at > m.last_read_at)) AS "unread!",
                  l.id AS "last_id?", l.sender_id AS "last_sender_id?", l.kind AS "last_kind?",
                  l.content AS "last_content?", l.withdrawn_at AS "last_withdrawn_at?",
                  l.created_at AS "last_created_at?"
           FROM conversation_members m
           LEFT JOIN LATERAL (
               SELECT x.id, x.sender_id, x.kind, x.content, x.withdrawn_at, x.created_at
               FROM messages x WHERE x.conversation_id = m.conversation_id
               ORDER BY x.created_at DESC, x.id DESC LIMIT 1
           ) l ON TRUE
           JOIN conversations cv ON cv.id = m.conversation_id
           WHERE m.user_id = $1 AND m.left_at IS NULL AND m.pinned_at IS NOT NULL
           ORDER BY m.pinned_at DESC, m.conversation_id DESC"#,
        user_id,
    )
    .fetch_all(conn)
    .await
}

/// 未置顶的在场会话，按 `(activity_at, conversation_id)` 倒序，严格早于游标。
pub(crate) async fn stream_summaries(
    conn: &mut PgConnection,
    user_id: i64,
    bound: Option<(DateTime<Utc>, Option<i64>, bool)>,
    limit: i64,
) -> sqlx::Result<Vec<SummaryRow>> {
    let (at, key, inclusive) = match bound {
        Some((at, key, inclusive)) => (Some(at), key, inclusive),
        None => (None, None, false),
    };
    sqlx::query_as!(
        SummaryRow,
        r#"SELECT m.conversation_id, cv.kind AS conv_kind, cv.title, m.peer_id, m.activity_at, m.last_read_at, m.muted, m.pinned_at,
                  (SELECT count(*) FROM messages x
                   WHERE x.conversation_id = m.conversation_id AND x.sender_id <> m.user_id
                     AND x.withdrawn_at IS NULL
                     AND (m.last_read_at IS NULL OR x.created_at > m.last_read_at)) AS "unread!",
                  l.id AS "last_id?", l.sender_id AS "last_sender_id?", l.kind AS "last_kind?",
                  l.content AS "last_content?", l.withdrawn_at AS "last_withdrawn_at?",
                  l.created_at AS "last_created_at?"
           FROM (
               SELECT * FROM conversation_members c
               WHERE c.user_id = $1 AND c.left_at IS NULL AND c.pinned_at IS NULL
                 AND ($2::timestamptz IS NULL OR c.activity_at < $2
                      OR (c.activity_at = $2 AND ($4 OR c.conversation_id < $3::bigint)))
               ORDER BY c.activity_at DESC, c.conversation_id DESC
               LIMIT $5
           ) m
           LEFT JOIN LATERAL (
               SELECT x.id, x.sender_id, x.kind, x.content, x.withdrawn_at, x.created_at
               FROM messages x WHERE x.conversation_id = m.conversation_id
               ORDER BY x.created_at DESC, x.id DESC LIMIT 1
           ) l ON TRUE
           JOIN conversations cv ON cv.id = m.conversation_id
           ORDER BY m.activity_at DESC, m.conversation_id DESC"#,
        user_id,
        at,
        key,
        inclusive,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 全部在场会话的未读消息合计。
pub(crate) async fn total_unread(conn: &mut PgConnection, user_id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!"
           FROM conversation_members m
           JOIN messages x ON x.conversation_id = m.conversation_id
           WHERE m.user_id = $1 AND m.left_at IS NULL
             AND x.sender_id <> m.user_id AND x.withdrawn_at IS NULL
             AND (m.last_read_at IS NULL OR x.created_at > m.last_read_at)"#,
        user_id,
    )
    .fetch_one(conn)
    .await
}

// ---------- 消息 ----------

pub(crate) struct NewMessage<'a> {
    pub id: i64,
    pub conversation_id: i64,
    pub sender_id: i64,
    pub content: &'a str,
    pub reply_to_id: Option<i64>,
    pub client_id: Option<&'a str>,
    pub now: DateTime<Utc>,
}

/// 写入一条文本消息。`client_id` 重复时不写，返回 false。
pub(crate) async fn insert_message(
    conn: &mut PgConnection,
    m: &NewMessage<'_>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "INSERT INTO messages (id, conversation_id, sender_id, kind, content, reply_to_id,
                               client_id, created_at)
         VALUES ($1, $2, $3, 'text', $4, $5, $6, $7)
         ON CONFLICT (sender_id, client_id) WHERE client_id IS NOT NULL DO NOTHING",
        m.id,
        m.conversation_id,
        m.sender_id,
        m.content,
        m.reply_to_id,
        m.client_id,
        m.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) struct MessageRow {
    pub id: i64,
    pub conversation_id: i64,
    pub sender_id: i64,
    pub kind: String,
    pub content: String,
    pub client_id: Option<String>,
    pub edited_at: Option<DateTime<Utc>>,
    pub withdrawn_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub reply_to_id: Option<i64>,
    pub reply_sender_id: Option<i64>,
    pub reply_content: Option<String>,
    pub reply_withdrawn_at: Option<DateTime<Utc>>,
    pub ai_state: Option<String>,
    pub ai_model_key: Option<String>,
    pub ai_workflow: Option<serde_json::Value>,
}

pub(crate) async fn message(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<MessageRow>> {
    sqlx::query_as!(
        MessageRow,
        r#"SELECT x.id, x.conversation_id, x.sender_id, x.kind, x.content, x.client_id,
                  x.edited_at, x.withdrawn_at, x.created_at, x.reply_to_id,
                  r.sender_id AS "reply_sender_id?", r.content AS "reply_content?",
                  r.withdrawn_at AS "reply_withdrawn_at?", x.ai_state, x.ai_model_key, x.ai_workflow
           FROM messages x LEFT JOIN messages r ON r.id = x.reply_to_id
           WHERE x.id = $1"#,
        id,
    )
    .fetch_optional(conn)
    .await
}

/// 锁住一条消息（编辑、撤回前）。
pub(crate) async fn lock_message(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<MessageRow>> {
    sqlx::query_as!(
        MessageRow,
        r#"SELECT x.id, x.conversation_id, x.sender_id, x.kind, x.content, x.client_id,
                  x.edited_at, x.withdrawn_at, x.created_at, x.reply_to_id,
                  r.sender_id AS "reply_sender_id?", r.content AS "reply_content?",
                  r.withdrawn_at AS "reply_withdrawn_at?", x.ai_state, x.ai_model_key, x.ai_workflow
           FROM messages x LEFT JOIN messages r ON r.id = x.reply_to_id
           WHERE x.id = $1
           FOR NO KEY UPDATE OF x"#,
        id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn message_by_client_id(
    conn: &mut PgConnection,
    sender_id: i64,
    client_id: &str,
) -> sqlx::Result<Option<MessageRow>> {
    sqlx::query_as!(
        MessageRow,
        r#"SELECT x.id, x.conversation_id, x.sender_id, x.kind, x.content, x.client_id,
                  x.edited_at, x.withdrawn_at, x.created_at, x.reply_to_id,
                  r.sender_id AS "reply_sender_id?", r.content AS "reply_content?",
                  r.withdrawn_at AS "reply_withdrawn_at?", x.ai_state, x.ai_model_key, x.ai_workflow
           FROM messages x LEFT JOIN messages r ON r.id = x.reply_to_id
           WHERE x.sender_id = $1 AND x.client_id = $2"#,
        sender_id,
        client_id,
    )
    .fetch_optional(conn)
    .await
}

/// 会话里的消息，新的在前，严格早于游标。
pub(crate) async fn messages(
    conn: &mut PgConnection,
    conversation_id: i64,
    bound: Option<(DateTime<Utc>, Option<i64>, bool)>,
    limit: i64,
) -> sqlx::Result<Vec<MessageRow>> {
    let (at, key, inclusive) = match bound {
        Some((at, key, inclusive)) => (Some(at), key, inclusive),
        None => (None, None, false),
    };
    sqlx::query_as!(
        MessageRow,
        r#"SELECT x.id, x.conversation_id, x.sender_id, x.kind, x.content, x.client_id,
                  x.edited_at, x.withdrawn_at, x.created_at, x.reply_to_id,
                  r.sender_id AS "reply_sender_id?", r.content AS "reply_content?",
                  r.withdrawn_at AS "reply_withdrawn_at?", x.ai_state, x.ai_model_key, x.ai_workflow
           FROM messages x LEFT JOIN messages r ON r.id = x.reply_to_id
           WHERE x.conversation_id = $1
             AND ($2::timestamptz IS NULL OR x.created_at < $2
                  OR (x.created_at = $2 AND ($4 OR x.id < $3::bigint)))
           ORDER BY x.created_at DESC, x.id DESC
           LIMIT $5"#,
        conversation_id,
        at,
        key,
        inclusive,
        limit,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn edit_message(
    conn: &mut PgConnection,
    id: i64,
    content: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE messages SET content = $2, edited_at = $3 WHERE id = $1 AND withdrawn_at IS NULL",
        id,
        content,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn withdraw_message(
    conn: &mut PgConnection,
    id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE messages SET withdrawn_at = $2 WHERE id = $1 AND withdrawn_at IS NULL",
        id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

// ---------- AI 消息（经 interop 由 ik-ai 写入） ----------

pub(crate) struct NewAiMessage<'a> {
    pub id: i64,
    pub conversation_id: i64,
    pub sender_id: i64,
    /// `text` | `system`
    pub kind: &'a str,
    pub content: &'a str,
    pub ai_state: Option<&'a str>,
    pub ai_model_key: Option<&'a str>,
    pub now: DateTime<Utc>,
}

pub(crate) async fn insert_ai_message(
    conn: &mut PgConnection,
    m: &NewAiMessage<'_>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO messages (id, conversation_id, sender_id, kind, content, ai_state,
                               ai_model_key, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        m.id,
        m.conversation_id,
        m.sender_id,
        m.kind,
        m.content,
        m.ai_state,
        m.ai_model_key,
        m.now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 改写 AI 回复的正文与状态（只改 AI 回复，已撤回的不动）。返回是否改了。
pub(crate) async fn update_ai_message(
    conn: &mut PgConnection,
    id: i64,
    content: &str,
    ai_state: &str,
    ai_model_key: Option<&str>,
    ai_workflow: Option<&serde_json::Value>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE messages SET content = $2, ai_state = $3, ai_model_key = $4,
             ai_workflow = COALESCE($5, ai_workflow)
         WHERE id = $1 AND ai_state IS NOT NULL AND withdrawn_at IS NULL",
        id,
        content,
        ai_state,
        ai_model_key,
        ai_workflow,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) struct HistoryRow {
    pub id: i64,
    pub sender_id: i64,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

/// AI 上下文用的历史（LLM-001），新的在前：`(from, until]` 之间未撤回的文本消息；
/// AI 回复只取已定稿或被停止的（停止的按用户看到的截断文本）。
pub(crate) async fn history(
    conn: &mut PgConnection,
    conversation_id: i64,
    from: Option<DateTime<Utc>>,
    until: DateTime<Utc>,
    limit: i64,
) -> sqlx::Result<Vec<HistoryRow>> {
    sqlx::query_as!(
        HistoryRow,
        "SELECT id, sender_id, content, created_at FROM messages
         WHERE conversation_id = $1 AND kind = 'text' AND withdrawn_at IS NULL
           AND (ai_state IS NULL OR ai_state IN ('done', 'stopped'))
           AND ($2::timestamptz IS NULL OR created_at > $2) AND created_at <= $3
         ORDER BY created_at DESC, id DESC
         LIMIT $4",
        conversation_id,
        from,
        until,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 最近的若干条未撤回文本消息，新的在前（重新生成判定「最后一条 AI 回复」，GEN-013）。
pub(crate) async fn recent_texts(
    conn: &mut PgConnection,
    conversation_id: i64,
    limit: i64,
) -> sqlx::Result<Vec<HistoryRow>> {
    sqlx::query_as!(
        HistoryRow,
        "SELECT id, sender_id, content, created_at FROM messages
         WHERE conversation_id = $1 AND kind = 'text' AND withdrawn_at IS NULL
         ORDER BY created_at DESC, id DESC
         LIMIT $2",
        conversation_id,
        limit,
    )
    .fetch_all(conn)
    .await
}
