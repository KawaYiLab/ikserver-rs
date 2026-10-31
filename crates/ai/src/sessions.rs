//! AI 私聊会话：新建、设置（标题、模型）、重置上下文、停止生成、重新生成，以及私信的触发钩子。
//!
//! 业务规则：ai-assistant.md 第 3.3、4.3、4.4、6.2 节。会话与消息本身在 ik-messaging（`kind = 'ai'`），
//! 代际与模型选择在 `ai_sessions`，任务在 `ai_jobs`。

use chrono::Utc;
use http::StatusCode;
use ik_auth::Principal;
use ik_kernel::bus::{Audience, Effects, Event};
use ik_kernel::jobs::Jobs;
use ik_kernel::{Id, Problem, Snowflake};
use ik_messaging::dto::{ConversationView, MessageView};
use ik_messaging::interop::NewAiMessage;
use ik_messaging::{MessageHooks, SentMessage};
use sqlx::PgConnection;

use crate::AiState;
use crate::catalog::{Catalog, CatalogCache, RoleCard};
use crate::dm;
use crate::dto::{
    CreateSessionRequest, RegenerateAccepted, RegenerateRequest, SessionView, UpdateSessionRequest,
};
use crate::repo::{self, NewJob, SessionRow};

/// 标题最多 64 字（TRIG-024）。
const TITLE_MAX_CHARS: usize = 64;
/// 重置分界消息（CTX-021）。
pub const RESET_MARKER: &str = "[对话已重置]";
/// 判定「最后一条 AI 回复」看最近多少条文本消息（GEN-013）。
const REGENERATE_WINDOW: i64 = 30;

fn character_not_found() -> Problem {
    Problem::new(
        StatusCode::NOT_FOUND,
        "AI_CHARACTER_NOT_FOUND",
        "AI 角色不存在或未开放私信",
    )
}

fn not_ai_session() -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "NOT_AI_SESSION",
        "只有 AI 会话支持此操作",
    )
}

fn not_ai_reply() -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "NOT_AI_REPLY",
        "这条消息不是 AI 回复",
    )
}

fn model_unavailable() -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "AI_MODEL_UNAVAILABLE",
        "模型不存在或已停用",
    )
}

fn model_not_allowed() -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "AI_MODEL_NOT_ALLOWED",
        "这个角色不能使用该模型",
    )
}

fn clean_title(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().take(TITLE_MAX_CHARS).collect())
}

/// 校验模型 key：启用、在角色卡白名单内（CTX-032）。空串视为没给。
fn checked_model<'a>(
    catalog: &'a Catalog,
    card: Option<&RoleCard>,
    key: Option<&str>,
) -> Result<Option<&'a str>, Problem> {
    let Some(key) = key.map(str::trim).filter(|k| !k.is_empty()) else {
        return Ok(None);
    };
    let model = catalog.model(key).ok_or_else(model_unavailable)?;
    let card = card.ok_or_else(character_not_found)?;
    if !card.allows(model) {
        return Err(model_not_allowed());
    }
    Ok(Some(model.key.as_str()))
}

/// 自己在场的 AI 会话。
async fn my_session(
    conn: &mut PgConnection,
    conversation_id: i64,
    me: i64,
    lock: bool,
) -> Result<SessionRow, Problem> {
    if ik_messaging::interop::active_peer(conn, conversation_id, me)
        .await?
        .is_none()
    {
        return Err(Problem::not_found());
    }
    let session = if lock {
        repo::lock_session(conn, conversation_id).await?
    } else {
        repo::session(conn, conversation_id).await?
    };
    session
        .filter(|s| s.user_id == me)
        .ok_or_else(not_ai_session)
}

/// 新建 AI 会话（TRIG-024）：每次新建一条；目标角色启用且开放私信，绑定的账号可用，双方没有拉黑。
pub async fn create(
    state: &AiState,
    principal: &Principal,
    request: CreateSessionRequest,
) -> Result<ConversationView, Problem> {
    principal.require_write()?;
    let me = principal.user_id.get();
    let catalog = state.catalog.get().await?;
    let card = catalog
        .card_by_slug(request.character.trim())
        .filter(|c| c.enabled && c.dm_enabled)
        .ok_or_else(character_not_found)?;
    if card.bound_user_id == me {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "DM_SELF",
            "不能给自己发私信",
        ));
    }
    let title = clean_title(request.title.as_deref());
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let target = ik_auth::interop::active_users(&mut tx, &[card.bound_user_id])
        .await?
        .into_iter()
        .next()
        .filter(|u| u.is_ai_agent)
        .ok_or_else(character_not_found)?;
    ik_social::interop::ensure_no_block(&mut tx, me, target.id).await?;
    let id = ik_messaging::interop::create_ai_session(
        &mut tx,
        &state.ids,
        me,
        target.id,
        title.as_deref(),
        now,
    )
    .await?;
    repo::insert_session(&mut tx, id, me, target.id, now).await?;
    tx.commit().await?;
    ik_messaging::service::conversation(&state.messaging, principal, id).await
}

fn session_view(catalog: &Catalog, session: &SessionRow, title: Option<String>) -> SessionView {
    SessionView {
        conversation_id: Id::new(session.conversation_id),
        character: catalog
            .card_for_user(session.ai_user_id)
            .map(|c| c.slug.clone()),
        title,
        model_key: session.model_key.clone(),
    }
}

/// AI 会话的设置。
pub async fn get(
    state: &AiState,
    principal: &Principal,
    conversation_id: i64,
) -> Result<SessionView, Problem> {
    let catalog = state.catalog.get().await?;
    let mut conn = state.db.acquire().await?;
    let session = my_session(&mut conn, conversation_id, principal.user_id.get(), false).await?;
    drop(conn);
    let view =
        ik_messaging::service::conversation(&state.messaging, principal, conversation_id).await?;
    Ok(session_view(&catalog, &session, view.title))
}

/// 改标题、会话级模型（CTX-032、TRIG-025）。变更推 `dm.conversation.updated` 给自己。
pub async fn update(
    state: &AiState,
    principal: &Principal,
    conversation_id: i64,
    request: UpdateSessionRequest,
) -> Result<SessionView, Problem> {
    let me = principal.user_id.get();
    let catalog = state.catalog.get().await?;
    let mut tx = state.db.begin().await?;
    let session = my_session(&mut tx, conversation_id, me, true).await?;
    if let Some(title) = &request.title {
        let title = clean_title(title.as_deref());
        ik_messaging::interop::set_title(&mut tx, conversation_id, title.as_deref()).await?;
    }
    if let Some(key) = &request.model_key {
        let card = catalog.card_for_user(session.ai_user_id);
        let key = checked_model(&catalog, card, key.as_deref())?;
        repo::set_session_model(&mut tx, conversation_id, key).await?;
    }
    tx.commit().await?;
    let view = get(state, principal, conversation_id).await?;
    let mut effects = Effects::new();
    effects.publish(Event::new(
        "dm.conversation.updated",
        Audience::User(me),
        &serde_json::json!({
            "conversationId": view.conversation_id,
            "title": view.title,
            "aiModelKey": view.model_key,
        }),
    )?);
    effects.dispatch(state.bus.as_ref());
    Ok(view)
}

/// 重置上下文（CTX-020～022）：代际加一，写一条 `[对话已重置]` 分界消息（不删历史），
/// 停止旧代际未结束的生成。返回分界消息。
pub async fn reset(
    state: &AiState,
    principal: &Principal,
    conversation_id: i64,
) -> Result<MessageView, Problem> {
    let me = principal.user_id.get();
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    my_session(&mut tx, conversation_id, me, true).await?;
    let epoch = repo::bump_epoch(&mut tx, conversation_id, now).await?;
    repo::stop_older_epochs(&mut tx, conversation_id, epoch, now).await?;
    let marker = ik_messaging::interop::insert_ai_message(
        &mut tx,
        &state.ids,
        NewAiMessage {
            conversation_id,
            sender_id: me,
            kind: "system",
            content: RESET_MARKER,
            ai_state: None,
            ai_model_key: None,
        },
        now,
        &mut effects,
    )
    .await?;
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(marker)
}

/// AI 回复对应的任务；调用者须是会话的在场成员。
async fn reply_job(
    conn: &mut PgConnection,
    message_id: i64,
    me: i64,
) -> Result<(MessageView, repo::JobRow), Problem> {
    let message = ik_messaging::interop::message(conn, message_id)
        .await?
        .ok_or_else(Problem::not_found)?;
    if ik_messaging::interop::active_peer(conn, message.conversation_id.get(), me)
        .await?
        .is_none()
    {
        return Err(Problem::not_found());
    }
    if message.withdrawn_at.is_some() || message.ai.is_none() {
        return Err(not_ai_reply());
    }
    let job = repo::job_by_reply(conn, message_id)
        .await?
        .ok_or_else(not_ai_reply)?;
    Ok((message, job))
}

/// 停止生成（GEN-010）。已经结束的回复直接返回。
pub async fn stop(state: &AiState, principal: &Principal, message_id: i64) -> Result<(), Problem> {
    let mut tx = state.db.begin().await?;
    let (_, job) = reply_job(&mut tx, message_id, principal.user_id.get()).await?;
    repo::request_stop(&mut tx, job.id, Utc::now()).await?;
    tx.commit().await?;
    Ok(())
}

/// 重新生成（GEN-013～015）：只能是会话里最后一条 AI 回复；先建新任务、再撤回旧回复（同一事务）。
/// 重复点击只产生一个任务。`modelKey` 只对这一次生效。
pub async fn regenerate(
    state: &AiState,
    principal: &Principal,
    message_id: i64,
    request: RegenerateRequest,
) -> Result<RegenerateAccepted, Problem> {
    principal.require_write()?;
    let me = principal.user_id.get();
    let catalog = state.catalog.get().await?;
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let (message, job) = reply_job(&mut tx, message_id, me).await?;
    let conversation_id = message.conversation_id.get();
    let session = my_session(&mut tx, conversation_id, me, true).await?;
    let recent =
        ik_messaging::interop::recent_texts(&mut tx, conversation_id, REGENERATE_WINDOW).await?;
    let position = recent.iter().position(|m| m.id == message_id);
    let latest = position.is_some_and(|index| {
        recent[..index]
            .iter()
            .all(|m| m.sender_id != session.ai_user_id)
    });
    if !latest {
        return Err(Problem::conflict(
            "AI_NOT_LATEST_REPLY",
            "只能重新生成最后一条 AI 回复",
        ));
    }
    let trigger_id = job.trigger_message_id.ok_or_else(not_ai_reply)?;
    let trigger_ok = ik_messaging::interop::message(&mut tx, trigger_id)
        .await?
        .is_some_and(|m| m.withdrawn_at.is_none());
    if !trigger_ok {
        return Err(Problem::conflict(
            "AI_TRIGGER_WITHDRAWN",
            "触发这条回复的消息已撤回",
        ));
    }
    let model_override = checked_model(
        &catalog,
        catalog.card_for_user(session.ai_user_id),
        request.model_key.as_deref(),
    )?;
    let created = repo::insert_job(
        &mut tx,
        &NewJob {
            id: state.ids.next_raw(),
            kind: "dm",
            execution_key: &format!("regenerate:{message_id}"),
            quota_user_id: me,
            ai_user_id: session.ai_user_id,
            conversation_id: Some(conversation_id),
            trigger_message_id: Some(trigger_id),
            post_id: None,
            trigger_comment_id: None,
            generation_epoch: session.context_epoch,
            model_override,
            now,
        },
    )
    .await?;
    if let Some(new_job) = created {
        dm::enqueue(
            &state.jobs,
            &mut tx,
            new_job,
            conversation_id,
            state.config.max_attempts,
        )
        .await?;
        repo::request_stop(&mut tx, job.id, now).await?;
        ik_messaging::interop::withdraw_by_system(&mut tx, message_id, now, &mut effects).await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(RegenerateAccepted {
        replaced_message_id: Id::new(message_id),
    })
}

/// 私信的钩子：AI 会话里用户发出文本消息时建任务（TRIG-020/021），撤回时停止对应生成（CTX-026）。
pub(crate) struct Hooks {
    catalog: CatalogCache,
    jobs: Jobs,
    ids: std::sync::Arc<Snowflake>,
    max_attempts: i16,
}

impl Hooks {
    pub(crate) fn new(state: AiState) -> Self {
        Self {
            catalog: state.catalog,
            jobs: state.jobs,
            ids: state.ids,
            max_attempts: state.config.max_attempts,
        }
    }
}

#[async_trait::async_trait]
impl MessageHooks for Hooks {
    async fn sent(
        &self,
        conn: &mut PgConnection,
        message: &SentMessage,
        effects: &mut Effects,
    ) -> Result<(), Problem> {
        if message.conversation_kind != "ai" {
            return Ok(());
        }
        let Some(session) = repo::session(conn, message.conversation_id).await? else {
            return Ok(());
        };
        // AI 自己发的不触发；编辑不经过这里（D-088 方案 A）。
        if message.sender_id == session.ai_user_id || message.sender_id != session.user_id {
            return Ok(());
        }
        let catalog = match self.catalog.get().await {
            Ok(catalog) => catalog,
            Err(problem) => {
                // 触发判定失败不影响消息写入（TRIG-002）。
                tracing::warn!(?problem, "读取 AI 配置失败，本条消息不触发 AI");
                return Ok(());
            }
        };
        if !catalog.limits.global_enabled {
            // 全局关闭：不入队，回一条不计费的提示（TRIG-001、D-082 方案 A）。
            dm::notice(
                conn,
                &self.ids,
                &session,
                dm::Notice::GlobalDisabled,
                Utc::now(),
                effects,
            )
            .await?;
            return Ok(());
        }
        if !catalog
            .card_for_user(session.ai_user_id)
            .is_some_and(|c| c.dm_enabled)
        {
            return Ok(());
        }
        crate::popular::record(conn, session.ai_user_id, session.context_from, message).await?;
        let created = repo::insert_job(
            conn,
            &NewJob {
                id: self.ids.next_raw(),
                kind: "dm",
                execution_key: &format!("dm:{}", message.id),
                quota_user_id: message.sender_id,
                ai_user_id: session.ai_user_id,
                conversation_id: Some(message.conversation_id),
                trigger_message_id: Some(message.id),
                post_id: None,
                trigger_comment_id: None,
                generation_epoch: session.context_epoch,
                model_override: None,
                now: message.created_at,
            },
        )
        .await?;
        if let Some(job_id) = created {
            dm::enqueue(
                &self.jobs,
                conn,
                job_id,
                message.conversation_id,
                self.max_attempts,
            )
            .await?;
        }
        Ok(())
    }

    async fn withdrawn(
        &self,
        conn: &mut PgConnection,
        message_id: i64,
        _conversation_id: i64,
        _effects: &mut Effects,
    ) -> Result<(), Problem> {
        repo::stop_by_trigger(conn, message_id, Utc::now()).await?;
        crate::popular::forget(conn, message_id).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dm::DmReply;

    #[test]
    fn titles_are_trimmed_and_capped() {
        assert_eq!(clean_title(None), None);
        assert_eq!(clean_title(Some("   ")), None);
        assert_eq!(clean_title(Some(" 你好 ")).as_deref(), Some("你好"));
        assert_eq!(
            clean_title(Some(&"字".repeat(70))).map(|t| t.chars().count()),
            Some(64)
        );
    }

    #[test]
    fn job_payload_is_stable() {
        let payload = serde_json::to_value(DmReply { job_id: 7 }).unwrap_or_default();
        assert_eq!(payload, serde_json::json!({ "jobId": 7 }));
    }
}
