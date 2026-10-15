//! 私信业务逻辑。业务规则：`docs/business-rules/messaging-notifications.md` 第 1～3 节。
//!
//! 已读与会话列表在统一收件箱（`ik_notify` 的 inbox）里，本模块提供会话、消息与输入中提示。

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use governor::clock::{Clock, DefaultClock};
use http::StatusCode;
use ik_auth::Principal;
use ik_auth::service::Author;
use ik_kernel::bus::{Audience, Effects, Event};
use ik_kernel::cursor::{self, Page};
use ik_kernel::{Id, Problem};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::MessagingState;
use crate::dto::{
    AiReplyMeta, ConversationView, EditMessageRequest, LastMessageView, MessageView, MessagesQuery,
    OpenDirectRequest, PeerView, ReplyView, SendMessageRequest, UpdateConversationRequest,
    WorkflowEvent,
};
use crate::repo::{self, MessageRow, NewMessage, SummaryRow};

/// 正文上限（MSG-001）。
pub const CONTENT_MAX_CHARS: usize = 4000;
/// 发出后可编辑、撤回的时间（MSG-015、MSG-016）。
pub const EDIT_WINDOW: Duration = Duration::minutes(5);
/// 每人最多置顶的会话数（CONV-023）。
pub const PIN_CAP: i64 = 50;
/// 预览与被回复摘要的字数（CONV-032、MSG-017）。
const PREVIEW_CHARS: usize = 80;
const CLIENT_ID_MAX_CHARS: usize = 64;
const DEFAULT_PAGE: u32 = 50;
const MAX_PAGE: u32 = 100;
const GONE_USER_NAME: &str = "已注销用户";
const WITHDRAWN_PREVIEW: &str = "[消息已撤回]";

fn message_too_old() -> Problem {
    Problem::conflict("MESSAGE_TOO_OLD", "消息发出超过 5 分钟，不能再修改或撤回")
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// 会话列表里最后一条消息的预览文案（CONV-032）。
pub fn preview(kind: &str, content: &str, withdrawn: bool) -> String {
    if withdrawn {
        return WITHDRAWN_PREVIEW.to_owned();
    }
    let mut text = truncate(content, PREVIEW_CHARS);
    if kind != "system" && content.chars().nth(PREVIEW_CHARS).is_some() {
        text.push('…');
    }
    text
}

pub(crate) fn message_view(row: MessageRow) -> MessageView {
    let withdrawn = row.withdrawn_at.is_some();
    let reply_to = row.reply_to_id.map(|id| {
        let reply_withdrawn = row.reply_withdrawn_at.is_some();
        ReplyView {
            id: Id::new(id),
            sender_id: row.reply_sender_id.map(Id::new),
            content: row
                .reply_content
                .filter(|_| !reply_withdrawn)
                .map(|c| truncate(&c, PREVIEW_CHARS)),
            withdrawn: reply_withdrawn,
        }
    });
    MessageView {
        id: Id::new(row.id),
        conversation_id: Id::new(row.conversation_id),
        sender_id: Id::new(row.sender_id),
        kind: row.kind,
        content: (!withdrawn).then_some(row.content),
        reply_to,
        client_id: row.client_id,
        edited_at: row.edited_at,
        withdrawn_at: row.withdrawn_at,
        created_at: row.created_at,
        ai: row.ai_state.map(|state| AiReplyMeta {
            state,
            model_key: row.ai_model_key,
            // 撤回后时间线随正文一起隐藏。
            workflow: row
                .ai_workflow
                .filter(|_| !withdrawn)
                .and_then(|w| serde_json::from_value(w).ok())
                .filter(|w: &Vec<WorkflowEvent>| !w.is_empty()),
        }),
    }
}

pub(crate) fn peer_view(
    peer_id: i64,
    authors: &HashMap<i64, Author>,
    avatars: &HashMap<i64, String>,
) -> PeerView {
    match authors.get(&peer_id) {
        Some(a) if !a.gone => PeerView {
            id: Some(a.id),
            name: a.username.clone(),
            level: Some(a.level),
            avatar_url: avatars.get(&peer_id).cloned(),
        },
        _ => PeerView {
            id: None,
            name: GONE_USER_NAME.to_owned(),
            level: None,
            avatar_url: None,
        },
    }
}

fn conversation_view(
    row: SummaryRow,
    authors: &HashMap<i64, Author>,
    avatars: &HashMap<i64, String>,
) -> ConversationView {
    let last_message = match (
        row.last_id,
        row.last_sender_id,
        row.last_kind,
        row.last_content,
        row.last_created_at,
    ) {
        (Some(id), Some(sender_id), Some(kind), Some(content), Some(created_at)) => {
            Some(LastMessageView {
                id: Id::new(id),
                sender_id: Id::new(sender_id),
                preview: preview(&kind, &content, row.last_withdrawn_at.is_some()),
                created_at,
            })
        }
        _ => None,
    };
    ConversationView {
        id: Id::new(row.conversation_id),
        kind: row.conv_kind,
        title: row.title,
        peer: peer_view(row.peer_id, authors, avatars),
        peer_user_id: row.peer_id,
        muted: row.muted,
        pinned: row.pinned_at.is_some(),
        activity_at: row.activity_at,
        last_read_at: row.last_read_at,
        last_message,
        unread_count: row.unread,
    }
}

/// 把摘要行转成视图（批量取对方资料）。
pub(crate) async fn conversation_views(
    db: &PgPool,
    media: &ik_media::MediaState,
    rows: Vec<SummaryRow>,
) -> Result<Vec<ConversationView>, Problem> {
    let mut peers: Vec<i64> = rows.iter().map(|r| r.peer_id).collect();
    peers.sort_unstable();
    peers.dedup();
    let authors = ik_auth::service::authors(db, &peers).await?;
    let avatars = media.avatar_urls(authors.values()).await?;
    Ok(rows
        .into_iter()
        .map(|row| conversation_view(row, &authors, &avatars))
        .collect())
}

/// 正文：去掉首尾空白后 1～4000 字。
fn validate_content(content: &str) -> Result<String, Problem> {
    let content = content.trim();
    if content.is_empty() {
        return Err(Problem::field("content", "REQUIRED", "消息内容不能为空"));
    }
    if content.chars().nth(CONTENT_MAX_CHARS).is_some() {
        return Err(Problem::field(
            "content",
            "TOO_LONG",
            format!("消息最多 {CONTENT_MAX_CHARS} 字"),
        ));
    }
    Ok(content.to_owned())
}

fn validate_client_id(client_id: Option<&str>) -> Result<Option<&str>, Problem> {
    match client_id {
        None => Ok(None),
        Some(id)
            if !id.is_empty()
                && id.len() <= CLIENT_ID_MAX_CHARS
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') =>
        {
            Ok(Some(id))
        }
        Some(_) => Err(Problem::field(
            "clientId",
            "INVALID",
            "clientId 须为 1～64 位字母、数字、- 或 _",
        )),
    }
}

/// 每秒 5 条、每分钟 60 条（MSG-020）。
fn check_send_rate(state: &MessagingState, user_id: i64) -> Result<(), Problem> {
    let limits = &state.send_limits;
    limits
        .per_second
        .check_key(&user_id)
        .and_then(|()| limits.per_minute.check_key(&user_id))
        .map_err(|denied| {
            let wait = denied.wait_time_from(DefaultClock::default().now());
            Problem::rate_limited().with_retry_after(wait.as_secs().saturating_add(1))
        })
}

fn event<T: Serialize>(kind: &'static str, audience: Audience, data: &T) -> Result<Event, Problem> {
    Event::new(kind, audience, data)
}

// ---------- 会话 ----------

/// 发起（或回到）与某人的私聊。幂等：同一对用户只有一个会话（CONV-020）。
///
/// 需要通过入站考试；对方必须存在且状态正常，双方不能存在拉黑关系（CONV-011）。
/// 新会话在对方的列表里先隐藏，收到第一条消息时才出现。
pub async fn open_direct(
    state: &MessagingState,
    principal: &Principal,
    request: OpenDirectRequest,
) -> Result<ConversationView, Problem> {
    principal.require_write()?;
    let me = principal.user_id.get();
    let target = request.user_id.get();
    if target == me {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "DM_SELF",
            "不能给自己发私信",
        ));
    }
    let mut tx = state.db.begin().await?;
    let Some(target_user) = ik_auth::interop::active_users(&mut tx, &[target])
        .await?
        .into_iter()
        .next()
    else {
        return Err(Problem::not_found());
    };
    if target_user.is_ai_agent {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "DM_AI_SESSION_REQUIRED",
            "与 AI 对话请新建 AI 会话",
        ));
    }
    ik_social::interop::ensure_no_block(&mut tx, me, target).await?;
    let now = Utc::now();
    let id = repo::upsert_direct(
        &mut tx,
        state.ids.next_raw(),
        me.min(target),
        me.max(target),
        now,
    )
    .await?;
    repo::insert_member(&mut tx, id, me, target, now, false).await?;
    repo::insert_member(&mut tx, id, target, me, now, true).await?;
    repo::rejoin(&mut tx, id, me, now).await?;
    tx.commit().await?;
    conversation(state, principal, id).await
}

/// 会话详情。只有在场成员能看（CONV-014）。
pub async fn conversation(
    state: &MessagingState,
    principal: &Principal,
    id: i64,
) -> Result<ConversationView, Problem> {
    let mut conn = state.db.acquire().await?;
    let row = repo::summary(&mut conn, principal.user_id.get(), id)
        .await?
        .ok_or_else(Problem::not_found)?;
    drop(conn);
    conversation_views(&state.db, &state.media, vec![row])
        .await?
        .pop()
        .ok_or_else(Problem::internal)
}

/// 改免打扰、置顶。只影响自己。
pub async fn update(
    state: &MessagingState,
    principal: &Principal,
    id: i64,
    request: UpdateConversationRequest,
) -> Result<ConversationView, Problem> {
    let me = principal.user_id.get();
    let mut tx = state.db.begin().await?;
    if repo::active_peer(&mut tx, id, me).await?.is_none() {
        return Err(Problem::not_found());
    }
    if let Some(muted) = request.muted {
        repo::set_muted(&mut tx, id, me, muted).await?;
    }
    if let Some(pinned) = request.pinned {
        if pinned {
            // 锁住用户行，串行化同一用户的并发置顶，计数才可靠。
            ik_auth::interop::lock_active_user(&mut tx, me).await?;
            if repo::pinned_count(&mut tx, me, id).await? >= PIN_CAP {
                return Err(Problem::conflict(
                    "DM_PIN_LIMIT",
                    format!("最多置顶 {PIN_CAP} 个会话"),
                ));
            }
        }
        repo::set_pinned(&mut tx, id, me, pinned, Utc::now()).await?;
    }
    tx.commit().await?;
    let view = conversation(state, principal, id).await?;
    let mut effects = Effects::new();
    effects.publish(event(
        "dm.conversation.updated",
        Audience::User(me),
        &serde_json::json!({
            "conversationId": view.id,
            "muted": view.muted,
            "pinned": view.pinned,
        }),
    )?);
    effects.dispatch(state.bus.as_ref());
    Ok(view)
}

/// 从自己的列表移除会话（历史保留）。对方不受影响；对方再发消息、或自己重新发起时恢复。
pub async fn leave(state: &MessagingState, principal: &Principal, id: i64) -> Result<(), Problem> {
    let me = principal.user_id.get();
    let mut conn = state.db.acquire().await?;
    if !repo::leave(&mut conn, id, me, Utc::now()).await? {
        return Err(Problem::not_found());
    }
    let mut effects = Effects::new();
    effects.publish(event(
        "dm.conversation.left",
        Audience::User(me),
        &serde_json::json!({ "conversationId": Id::<()>::new(id) }),
    )?);
    effects.dispatch(state.bus.as_ref());
    Ok(())
}

// ---------- 消息 ----------

#[derive(Serialize, Deserialize)]
struct MessageCursor {
    c: DateTime<Utc>,
    i: i64,
}

/// 会话里的消息（新的在前）。只有在场成员能看。
pub async fn messages(
    state: &MessagingState,
    principal: &Principal,
    id: i64,
    query: MessagesQuery,
) -> Result<Page<MessageView>, Problem> {
    let limit = i64::from(query.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE));
    let bound = query
        .cursor
        .as_deref()
        .map(cursor::decode::<MessageCursor>)
        .transpose()?
        .map(|c| (c.c, Some(c.i), false));
    let mut conn = state.db.acquire().await?;
    if repo::active_peer(&mut conn, id, principal.user_id.get())
        .await?
        .is_none()
    {
        return Err(Problem::not_found());
    }
    let rows = repo::messages(&mut conn, id, bound, limit + 1).await?;
    Page::from_rows(
        rows.into_iter().map(message_view).collect(),
        usize::try_from(limit).unwrap_or(50),
        |m: &MessageView| MessageCursor {
            c: m.created_at,
            i: m.id.get(),
        },
    )
}

/// 发消息。返回（是否新建，消息）：带了 `clientId` 且之前已发过时返回第一次的消息。
///
/// 需要通过入站考试、是在场成员、双方没有拉黑关系（MSG-013）。新消息推进双方的会话排序，
/// 对方移除了会话的话重新出现；发送者的已读水位推到这条消息。
pub async fn send(
    state: &MessagingState,
    principal: &Principal,
    conversation_id: i64,
    request: SendMessageRequest,
) -> Result<(bool, MessageView), Problem> {
    principal.require_write()?;
    let me = principal.user_id.get();
    check_send_rate(state, me)?;
    let content = validate_content(&request.content)?;
    let client_id = validate_client_id(request.client_id.as_deref())?;

    let mut tx = state.db.begin().await?;
    if let Some(client_id) = client_id
        && let Some(existing) = repo::message_by_client_id(&mut tx, me, client_id).await?
    {
        return replay(existing, conversation_id);
    }
    let peer = repo::active_peer(&mut tx, conversation_id, me)
        .await?
        .ok_or_else(Problem::not_found)?;
    ik_social::interop::ensure_no_block(&mut tx, me, peer).await?;
    if let Some(reply_to) = request.reply_to_id {
        let valid = repo::message(&mut tx, reply_to.get())
            .await?
            .is_some_and(|r| r.conversation_id == conversation_id && r.withdrawn_at.is_none());
        if !valid {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "REPLY_TARGET_INVALID",
                "被回复的消息不存在或已撤回",
            ));
        }
    }
    let id = state.ids.next_raw();
    let now = Utc::now();
    let inserted = repo::insert_message(
        &mut tx,
        &NewMessage {
            id,
            conversation_id,
            sender_id: me,
            content: &content,
            reply_to_id: request.reply_to_id.map(Id::get),
            client_id,
            now,
        },
    )
    .await?;
    if !inserted {
        // 同一个 clientId 的并发重试：另一个请求先提交了。
        drop(tx);
        let mut conn = state.db.acquire().await?;
        let existing = match client_id {
            Some(client_id) => repo::message_by_client_id(&mut conn, me, client_id).await?,
            None => None,
        };
        return existing.map_or_else(|| Err(Problem::internal()), |m| replay(m, conversation_id));
    }
    let members = repo::bump_members(&mut tx, conversation_id, now).await?;
    repo::mark_read(&mut tx, conversation_id, me, now).await?;
    let row = repo::message(&mut tx, id)
        .await?
        .ok_or_else(Problem::internal)?;
    let view = message_view(row);
    let mut effects = Effects::new();
    effects.publish(event(
        "dm.message.created",
        Audience::Users(members.into()),
        &serde_json::json!({ "conversationId": view.conversation_id, "message": view }),
    )?);
    if let Some(hooks) = state.hooks() {
        let conversation_kind = repo::conversation_kind(&mut tx, conversation_id)
            .await?
            .ok_or_else(Problem::internal)?;
        let sent = crate::SentMessage {
            id,
            conversation_id,
            conversation_kind,
            sender_id: me,
            peer_id: peer,
            content: content.clone(),
            created_at: now,
        };
        hooks.sent(&mut tx, &sent, &mut effects).await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok((true, view))
}

/// 重放：同一个 clientId 已经发过。用在别的会话上视为冲突。
fn replay(existing: MessageRow, conversation_id: i64) -> Result<(bool, MessageView), Problem> {
    if existing.conversation_id == conversation_id {
        Ok((false, message_view(existing)))
    } else {
        Err(Problem::conflict(
            "ALREADY_EXISTS",
            "该 clientId 已用于另一个会话的消息",
        ))
    }
}

/// 锁住自己发的消息。不是自己的：会话成员看得到这条消息，返回 403；否则 404。
async fn lock_own_message(
    conn: &mut sqlx::PgConnection,
    id: i64,
    me: i64,
) -> Result<MessageRow, Problem> {
    let row = repo::lock_message(conn, id)
        .await?
        .ok_or_else(Problem::not_found)?;
    if row.sender_id == me {
        return Ok(row);
    }
    if repo::active_peer(conn, row.conversation_id, me)
        .await?
        .is_some()
    {
        Err(Problem::forbidden())
    } else {
        Err(Problem::not_found())
    }
}

fn ensure_editable(row: &MessageRow, now: DateTime<Utc>) -> Result<(), Problem> {
    if row.kind != "text" {
        return Err(Problem::forbidden());
    }
    if now - row.created_at > EDIT_WINDOW {
        return Err(message_too_old());
    }
    Ok(())
}

pub(crate) async fn publish_change(
    conn: &mut sqlx::PgConnection,
    kind: &'static str,
    view: &MessageView,
) -> Result<Effects, Problem> {
    let members = repo::active_member_ids(conn, view.conversation_id.get()).await?;
    let mut effects = Effects::new();
    if !members.is_empty() {
        effects.publish(event(
            kind,
            Audience::Users(members.into()),
            &serde_json::json!({ "conversationId": view.conversation_id, "message": view }),
        )?);
    }
    Ok(effects)
}

/// 编辑自己的文本消息：发出 5 分钟内、未撤回（MSG-015）。内容没变时不记编辑时间。
pub async fn edit(
    state: &MessagingState,
    principal: &Principal,
    id: i64,
    request: EditMessageRequest,
) -> Result<MessageView, Problem> {
    principal.require_write()?;
    let me = principal.user_id.get();
    let content = validate_content(&request.content)?;
    let mut tx = state.db.begin().await?;
    let row = lock_own_message(&mut tx, id, me).await?;
    if row.withdrawn_at.is_some() {
        return Err(Problem::conflict("MESSAGE_WITHDRAWN", "消息已撤回"));
    }
    let now = Utc::now();
    ensure_editable(&row, now)?;
    if row.content == content {
        return Ok(message_view(row));
    }
    repo::edit_message(&mut tx, id, &content, now).await?;
    let view = message_view(
        repo::message(&mut tx, id)
            .await?
            .ok_or_else(Problem::internal)?,
    );
    let effects = publish_change(&mut tx, "dm.message.edited", &view).await?;
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(view)
}

/// 撤回自己的消息：发出 5 分钟内；幂等，已撤回的直接返回（MSG-016）。不要求通过考试。
pub async fn withdraw(
    state: &MessagingState,
    principal: &Principal,
    id: i64,
) -> Result<MessageView, Problem> {
    let me = principal.user_id.get();
    let mut tx = state.db.begin().await?;
    let row = lock_own_message(&mut tx, id, me).await?;
    if row.withdrawn_at.is_some() {
        return Ok(message_view(row));
    }
    let now = Utc::now();
    ensure_editable(&row, now)?;
    repo::withdraw_message(&mut tx, id, now).await?;
    let view = message_view(
        repo::message(&mut tx, id)
            .await?
            .ok_or_else(Problem::internal)?,
    );
    let mut effects = publish_change(&mut tx, "dm.message.withdrawn", &view).await?;
    if let Some(hooks) = state.hooks() {
        hooks
            .withdrawn(&mut tx, id, row.conversation_id, &mut effects)
            .await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(view)
}

/// 输入中提示（WS `typing` 帧，WS-010）：发给会话里在场的对方。
/// 不是在场成员 → 404；双方存在拉黑关系 → 403 `USER_BLOCKED`。节流由网关按连接做。
pub async fn typing(
    state: &MessagingState,
    user_id: i64,
    conversation_id: i64,
) -> Result<(), Problem> {
    let mut conn = state.db.acquire().await?;
    let peer = repo::active_peer(&mut conn, conversation_id, user_id)
        .await?
        .ok_or_else(Problem::not_found)?;
    ik_social::interop::ensure_no_block(&mut conn, user_id, peer).await?;
    let present = repo::active_member_ids(&mut conn, conversation_id).await?;
    if present.contains(&peer) {
        let mut effects = Effects::new();
        effects.publish(event(
            "dm.typing",
            Audience::User(peer),
            &serde_json::json!({
                "conversationId": Id::<()>::new(conversation_id),
                "userId": Id::<()>::new(user_id),
            }),
        )?);
        effects.dispatch(state.bus.as_ref());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_truncate_and_mask() {
        assert_eq!(preview("text", "你好", false), "你好");
        assert_eq!(preview("text", "你好", true), "[消息已撤回]");
        let long = "字".repeat(81);
        let p = preview("text", &long, false);
        assert_eq!(p.chars().count(), 81);
        assert!(p.ends_with('…'));
        assert_eq!(preview("text", &"字".repeat(80), false).chars().count(), 80);
        assert_eq!(preview("system", &long, false).chars().count(), 80);
    }

    #[test]
    fn content_is_trimmed_and_bounded() {
        assert_eq!(validate_content("  hi \n").unwrap(), "hi");
        assert!(validate_content(" \n\t").is_err());
        assert!(validate_content(&"字".repeat(4000)).is_ok());
        assert!(validate_content(&"字".repeat(4001)).is_err());
    }

    #[test]
    fn client_ids_are_restricted() {
        assert_eq!(validate_client_id(None).unwrap(), None);
        assert_eq!(
            validate_client_id(Some("0b5f-AB_9")).unwrap(),
            Some("0b5f-AB_9")
        );
        for bad in ["", "has space", "中文", &"a".repeat(65)] {
            assert!(validate_client_id(Some(bad)).is_err(), "{bad}");
        }
    }
}
