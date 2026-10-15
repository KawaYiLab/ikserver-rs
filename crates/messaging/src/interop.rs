//! 给 notify（统一收件箱）用的接口。
//!
//! 写入类函数接收调用方的连接/事务：通知写入时推进私聊排序、收件箱已读，
//! 与调用方的写入同一事务提交（AGENTS.md 规则 6），同时不需要直接读写本模块的表（规则 5）。

use chrono::{DateTime, Utc};
use ik_kernel::Problem;
use sqlx::{PgConnection, PgPool};

use crate::dto::{ConversationView, MessageView};
use crate::repo;
use crate::service::{conversation_views, message_view};

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

/// 全部置顶会话（置顶时间倒序）。
pub async fn pinned(
    db: &PgPool,
    media: &ik_media::MediaState,
    user_id: i64,
) -> Result<Vec<ConversationView>, Problem> {
    let mut conn = db.acquire().await?;
    let rows = repo::pinned_summaries(&mut conn, user_id).await?;
    drop(conn);
    conversation_views(db, media, rows).await
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
    drop(conn);
    conversation_views(db, media, rows).await
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
    conversation_id: i64,
    before: Option<Before>,
    limit: i64,
) -> Result<Vec<MessageView>, Problem> {
    Ok(
        repo::messages(conn, conversation_id, before.map(Before::bound), limit)
            .await?
            .into_iter()
            .map(message_view)
            .collect(),
    )
}
