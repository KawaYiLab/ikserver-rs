//! 给 notify（统一收件箱）与 ai（AI 私聊）用的接口。
//!
//! 写入类函数接收调用方的连接/事务：通知写入时推进私聊排序、收件箱已读，
//! 与调用方的写入同一事务提交（AGENTS.md 规则 6），同时不需要直接读写本模块的表（规则 5）。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use ik_kernel::bus::{Audience, Effects, Event};
use ik_kernel::{Problem, Snowflake};
use sqlx::{PgConnection, PgPool};

use crate::dto::{ConversationView, MessageView};
use crate::repo;
use crate::service::{conversation_views, message_view, message_views, publish_change};

/// 双源归并时「严格早于游标」的边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Before {
    /// 游标落在本数据源的一项上：`(时间, id)` 严格更小。
    Item { at: DateTime<Utc>, id: i64 },
    /// 游标落在另一个数据源的项上，且同一时刻那个数据源排在前面：同一时刻的本源项都还没出过。
    UpTo(DateTime<Utc>),
}

impl Before {
    fn bound(self) -> (DateTime<Utc>, Option<i64>, bool) {
        match self {
            Self::Item { at, id } => (at, Some(id), false),
            Self::UpTo(at) => (at, None, true),
        }
    }
}

/// 对方发来通知时推进收件人这一侧私聊会话的排序（CONV-005）。不恢复已移除的会话。
/// 返回会话 id（收件人与发送者之间没有在场的私聊时为 None）。
pub async fn touch_direct(
    conn: &mut PgConnection,
    recipient_id: i64,
    sender_id: i64,
    at: DateTime<Utc>,
) -> Result<Option<i64>, Problem> {
    Ok(repo::touch_by_peer(conn, recipient_id, sender_id, at).await?)
}

/// 所有在场私聊的对方 id（含置顶）。收件箱据此把这些人发来的通知合并进私聊会话。
pub async fn active_peers(conn: &mut PgConnection, user_id: i64) -> Result<Vec<i64>, Problem> {
    Ok(repo::active_peers(conn, user_id).await?)
}

/// 在场会话的对方；不是在场成员时为 None。
pub async fn active_peer(
    conn: &mut PgConnection,
    conversation_id: i64,
    user_id: i64,
) -> Result<Option<i64>, Problem> {
    Ok(repo::active_peer(conn, conversation_id, user_id).await?)
}

/// 在场会话的成员身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Membership {
    /// 对方 id。
    pub peer_id: i64,
    /// 一对一私聊。只有这种会话混排对方发来的通知；AI 会话（同一个 AI 可以有多条）不混，
    /// AI 在评论区回复、提及你的通知留在它自己的通知分组里。
    pub direct: bool,
}

/// 在场会话的成员身份；不是在场成员时为 None。
pub async fn membership(
    conn: &mut PgConnection,
    conversation_id: i64,
    user_id: i64,
) -> Result<Option<Membership>, Problem> {
    let row = repo::active_membership(conn, conversation_id, user_id).await?;
    Ok(row.map(|(peer_id, kind)| Membership {
        peer_id,
        direct: kind == "direct",
    }))
}

/// 全部置顶会话（置顶时间倒序）。
pub async fn pinned(
    db: &PgPool,
    media: &ik_media::MediaState,
    user_id: i64,
) -> Result<Vec<ConversationView>, Problem> {
    let mut conn = db.acquire().await?;
    let rows = repo::pinned_summaries(&mut conn, user_id).await?;
    conversation_views(&mut conn, media, rows).await
}

/// 未置顶的在场会话，按 `(activityAt, id)` 倒序，严格早于 `before`。
pub async fn page(
    db: &PgPool,
    media: &ik_media::MediaState,
    user_id: i64,
    before: Option<Before>,
    limit: i64,
) -> Result<Vec<ConversationView>, Problem> {
    let mut conn = db.acquire().await?;
    let rows = repo::stream_summaries(&mut conn, user_id, before.map(Before::bound), limit).await?;
    conversation_views(&mut conn, media, rows).await
}

/// 全部在场会话的未读消息合计。
pub async fn total_unread(conn: &mut PgConnection, user_id: i64) -> Result<i64, Problem> {
    Ok(repo::total_unread(conn, user_id).await?)
}

/// 推进已读水位（只前进）。返回对方 id；不是在场成员时为 None。
pub async fn mark_read(
    conn: &mut PgConnection,
    user_id: i64,
    conversation_id: i64,
    at: DateTime<Utc>,
) -> Result<Option<i64>, Problem> {
    Ok(repo::mark_read(conn, conversation_id, user_id, at).await?)
}

/// 全部在场会话标为已读，返回水位前进了的会话数。
pub async fn mark_all_read(
    conn: &mut PgConnection,
    user_id: i64,
    at: DateTime<Utc>,
) -> Result<u64, Problem> {
    Ok(repo::mark_all_read(conn, user_id, at).await?)
}

/// 会话里的消息，新的在前，严格早于 `before`。调用方负责成员校验（[`active_peer`]）。
pub async fn messages(
    conn: &mut PgConnection,
    media: &ik_media::MediaState,
    conversation_id: i64,
    before: Option<Before>,
    limit: i64,
) -> Result<Vec<MessageView>, Problem> {
    let rows = repo::messages(conn, conversation_id, before.map(Before::bound), limit).await?;
    message_views(conn, media, rows).await
}

// ---------- AI 私聊（ik-ai） ----------

/// 新建一条 AI 会话（CONV-012：每次新建，不复用）。AI 一侧先不出现在列表里。返回会话 id。
/// 调用方负责校验目标是可用的 AI 账号、双方没有拉黑。
pub async fn create_ai_session(
    conn: &mut PgConnection,
    ids: &Snowflake,
    user_id: i64,
    ai_user_id: i64,
    title: Option<&str>,
    now: DateTime<Utc>,
) -> Result<i64, Problem> {
    let id = ids.next_raw();
    repo::insert_ai_conversation(
        conn,
        id,
        user_id.min(ai_user_id),
        user_id.max(ai_user_id),
        title,
        now,
    )
    .await?;
    repo::insert_member(conn, id, user_id, ai_user_id, now, false).await?;
    repo::insert_member(conn, id, ai_user_id, user_id, now, true).await?;
    Ok(id)
}

/// 会话类型（`direct` / `ai`）；会话不存在时为 None。
pub async fn conversation_kind(
    conn: &mut PgConnection,
    conversation_id: i64,
) -> Result<Option<String>, Problem> {
    Ok(repo::conversation_kind(conn, conversation_id).await?)
}

/// 改 AI 会话的标题（None 清除）。
pub async fn set_title(
    conn: &mut PgConnection,
    conversation_id: i64,
    title: Option<&str>,
) -> Result<(), Problem> {
    Ok(repo::set_title(conn, conversation_id, title).await?)
}

/// 还没有标题的 AI 会话补上标题（首条消息自动起名）。返回是否写入。
pub async fn fill_title(
    conn: &mut PgConnection,
    conversation_id: i64,
    title: &str,
) -> Result<bool, Problem> {
    Ok(repo::fill_title(conn, conversation_id, title).await?)
}

/// 自己和某个 AI 账号之间在场的 AI 会话，最近活动的在前，严格早于 `before = (activityAt, id)`。
/// 多取一条判断下一页由调用方决定（`limit` 原样传给 SQL）。
pub async fn ai_sessions(
    conn: &mut PgConnection,
    media: &ik_media::MediaState,
    user_id: i64,
    ai_user_id: i64,
    before: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> Result<Vec<ConversationView>, Problem> {
    let rows = repo::ai_session_summaries(conn, user_id, ai_user_id, before, limit).await?;
    conversation_views(conn, media, rows).await
}

/// 在场成员。
pub async fn active_members(
    conn: &mut PgConnection,
    conversation_id: i64,
) -> Result<Vec<i64>, Problem> {
    Ok(repo::active_member_ids(conn, conversation_id).await?)
}

/// 一条由 ik-ai 写入的消息。
#[derive(Debug, Clone, Copy)]
pub struct NewAiMessage<'a> {
    pub conversation_id: i64,
    pub sender_id: i64,
    /// `text`（AI 回复、额度提示）| `system`（上下文重置分界）
    pub kind: &'a str,
    pub content: &'a str,
    /// AI 回复的状态；分界消息为 None。
    pub ai_state: Option<&'a str>,
    pub ai_model_key: Option<&'a str>,
}

/// 写入一条消息：推进双方的会话排序，发送者的已读水位推到这条消息；`dm.message.created`
/// 放进 `effects`（`streaming` 的回复正文为空，随后由 `dm.message.delta` 增长）。
pub async fn insert_ai_message(
    conn: &mut PgConnection,
    ids: &Snowflake,
    message: NewAiMessage<'_>,
    now: DateTime<Utc>,
    effects: &mut Effects,
) -> Result<MessageView, Problem> {
    let id = ids.next_raw();
    repo::insert_ai_message(
        conn,
        &repo::NewAiMessage {
            id,
            conversation_id: message.conversation_id,
            sender_id: message.sender_id,
            kind: message.kind,
            content: message.content,
            ai_state: message.ai_state,
            ai_model_key: message.ai_model_key,
            now,
        },
    )
    .await?;
    let members = repo::bump_members(conn, message.conversation_id, now).await?;
    repo::mark_read(conn, message.conversation_id, message.sender_id, now).await?;
    // AI 写的消息没有图片。
    let view = message_view(
        repo::message(conn, id)
            .await?
            .ok_or_else(Problem::internal)?,
        &HashMap::new(),
    );
    effects.publish(Event::new(
        "dm.message.created",
        Audience::Users(members.into()),
        &serde_json::json!({ "conversationId": view.conversation_id, "message": view }),
    )?);
    Ok(view)
}

/// 改写 AI 回复（重试时回到 `streaming`、定稿、失败、停止）。不是 AI 回复或已撤回时返回 None。
/// 定稿等状态变化以 `dm.message.edited` 广播给在场成员（与普通编辑同形，含完整消息）。
pub async fn update_ai_message(
    conn: &mut PgConnection,
    message_id: i64,
    content: &str,
    ai_state: &str,
    ai_model_key: Option<&str>,
    workflow: Option<&serde_json::Value>,
    effects: &mut Effects,
) -> Result<Option<MessageView>, Problem> {
    if !repo::update_ai_message(conn, message_id, content, ai_state, ai_model_key, workflow).await?
    {
        return Ok(None);
    }
    // AI 写的消息没有图片。
    let view = message_view(
        repo::message(conn, message_id)
            .await?
            .ok_or_else(Problem::internal)?,
        &HashMap::new(),
    );
    effects.extend(publish_change(conn, "dm.message.edited", &view).await?);
    Ok(Some(view))
}

/// 撤回一条消息（重新生成时移除旧回复，GEN-015），广播 `dm.message.withdrawn`。已撤回时不动。
pub async fn withdraw_by_system(
    conn: &mut PgConnection,
    message_id: i64,
    now: DateTime<Utc>,
    effects: &mut Effects,
) -> Result<(), Problem> {
    let Some(row) = repo::lock_message(conn, message_id).await? else {
        return Ok(());
    };
    if row.withdrawn_at.is_some() {
        return Ok(());
    }
    repo::withdraw_message(conn, message_id, now).await?;
    // 撤回后不带图片。
    let view = message_view(
        repo::message(conn, message_id)
            .await?
            .ok_or_else(Problem::internal)?,
        &HashMap::new(),
    );
    effects.extend(publish_change(conn, "dm.message.withdrawn", &view).await?);
    Ok(())
}

/// 一条消息（含已撤回的）。不取图片（`images` 为空）：调用方只看状态与归属。
pub async fn message(
    conn: &mut PgConnection,
    message_id: i64,
) -> Result<Option<MessageView>, Problem> {
    Ok(repo::message(conn, message_id)
        .await?
        .map(|row| message_view(row, &HashMap::new())))
}

/// 历史里的一条消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryItem {
    pub id: i64,
    pub sender_id: i64,
    /// 带图的消息可以为空串。
    pub content: String,
    /// 图片（媒体 id，有序，MSG-030）。
    pub image_ids: Vec<i64>,
    pub created_at: DateTime<Utc>,
    /// AI 回复落库的工作流时间线（已脱敏，WF-003）；用来把上一轮查过什么带进上下文（CTX-012）。
    pub workflow: Option<serde_json::Value>,
}

fn history_items(rows: Vec<repo::HistoryRow>) -> Vec<HistoryItem> {
    rows.into_iter()
        .map(|r| HistoryItem {
            id: r.id,
            sender_id: r.sender_id,
            content: r.content,
            image_ids: r.image_ids,
            created_at: r.created_at,
            workflow: r.ai_workflow,
        })
        .collect()
}

/// AI 上下文用的历史（LLM-001），按时间正序：`(from, until]` 之间未撤回的文本消息，最多 `limit` 条
/// （取最近的）；AI 回复只取已定稿或被停止的。
pub async fn history(
    conn: &mut PgConnection,
    conversation_id: i64,
    from: Option<DateTime<Utc>>,
    until: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<HistoryItem>, Problem> {
    let mut items = history_items(repo::history(conn, conversation_id, from, until, limit).await?);
    items.reverse();
    Ok(items)
}

/// AI 跨会话衔接（ai-assistant.md MEM-005）：`sessions` 是（会话 id, 上下文起点）；取 `since` 之后、
/// `user_id` 仍在场的那些会话里最近的 `limit` 条文本消息，按时间正序。
pub async fn recent_across(
    conn: &mut PgConnection,
    user_id: i64,
    sessions: &[(i64, Option<DateTime<Utc>>)],
    since: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<HistoryItem>, Problem> {
    if sessions.is_empty() {
        return Ok(Vec::new());
    }
    let (ids, from): (Vec<i64>, Vec<Option<DateTime<Utc>>>) = sessions.iter().copied().unzip();
    let mut items =
        history_items(repo::recent_across(conn, user_id, &ids, &from, since, limit).await?);
    items.reverse();
    Ok(items)
}

/// 翻聊天记录的一条命中（AI 记忆 MEM-095）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub conversation_id: i64,
    pub message_id: i64,
    pub sender_id: i64,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

/// 按关键词在几个会话里翻聊天记录（AI 记忆 MEM-095）：`sessions` 是（会话 id, 上下文起点），只看起点之后的、
/// `user_id` 仍在场的；`[from, to)` 限定时间。命中关键词多的在前，同样多的按 `oldest_first` 排。
#[allow(clippy::too_many_arguments)]
pub async fn search_across(
    conn: &mut PgConnection,
    user_id: i64,
    sessions: &[(i64, Option<DateTime<Utc>>)],
    terms: &[String],
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    oldest_first: bool,
    limit: i64,
) -> Result<Vec<SearchHit>, Problem> {
    if sessions.is_empty() {
        return Ok(Vec::new());
    }
    let (ids, starts): (Vec<i64>, Vec<Option<DateTime<Utc>>>) = sessions.iter().copied().unzip();
    Ok(repo::search_across(
        conn,
        user_id,
        &ids,
        &starts,
        terms,
        from,
        to,
        oldest_first,
        limit,
    )
    .await?
    .into_iter()
    .map(|r| SearchHit {
        conversation_id: r.conversation_id,
        message_id: r.id,
        sender_id: r.sender_id,
        content: r.content,
        created_at: r.created_at,
    })
    .collect())
}

/// 几个会话里 `before` 之前最后一条文本消息的时刻（AI 久别重逢，MEM-100）。
pub async fn last_text_before(
    conn: &mut PgConnection,
    conversation_ids: &[i64],
    before: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, Problem> {
    if conversation_ids.is_empty() {
        return Ok(None);
    }
    Ok(repo::last_text_before(conn, conversation_ids, before).await?)
}

/// 最近的未撤回文本消息（新的在前）。
pub async fn recent_texts(
    conn: &mut PgConnection,
    conversation_id: i64,
    limit: i64,
) -> Result<Vec<HistoryItem>, Problem> {
    Ok(history_items(
        repo::recent_texts(conn, conversation_id, limit).await?,
    ))
}
