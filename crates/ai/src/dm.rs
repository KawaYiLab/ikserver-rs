//! AI 私聊回复的生成（GEN 第 6.1～6.3 节、LLM 第 10 节）。
//!
//! 一个任务（`ai_jobs` 一行）由后台任务 `ai.dm_reply` 执行，三段各自独立：
//! 1. 准备（短事务，锁会话行）：复核触发条件（TRIG-023、CTX-024），决定模型，预占额度，建占位消息；
//! 2. 生成（事务之外，[`crate::generate`]）：流式调用上游与工具循环，节流推送累计正文与工作流事件，
//!    轮询停止信号，首帧看门狗与总预算；
//! 3. 定稿（短事务，锁会话行）：写正文、状态与时间线，结算额度、记审计，提交后广播。
//!
//! 失败交给后台任务重试（同一条占位继续写，GEN-002/007）；首帧超时是终态，不重试（TRIG-034）。

use std::collections::HashMap;

use chrono::Utc;
use ik_kernel::bus::Effects;
use ik_kernel::jobs::{IntoTaskHandlerResult, JobSpec, Jobs, TaskHandler, WorkerContext};
use ik_kernel::{Problem, Snowflake};
use ik_messaging::interop::{HistoryItem, NewAiMessage};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::AiState;
use crate::catalog::{Catalog, Model, OutputMode, RoleCard};
use crate::generate::{End, Live, StreamCtx, Streamed, generate};
use crate::llm::{ChatMessage, LlmError, Providers};
use crate::quota::{self, Denial, Hold};
use crate::repo::{self, JobOutcome, JobRow, SessionRow};
use crate::sanitize;
use crate::toolbox::Toolbox;
use crate::tools::ToolContext;
use crate::workflow::Workflow;

const FAILED_TEXT: &str = "（生成失败，请重试）";
const STOPPED_TEXT: &str = "（已停止）";

/// 后台任务：生成一条 AI 私聊回复。重试次数取配置 `ai.max_attempts`（TRIG-033），
/// 退避用 graphile_worker 的指数退避。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmReply {
    pub job_id: i64,
}

/// 在调用方的事务里入队（TRIG-031：与触发消息同一事务）。
pub(crate) async fn enqueue(
    jobs: &Jobs,
    conn: &mut PgConnection,
    job_id: i64,
    max_attempts: i16,
) -> Result<(), Problem> {
    let spec = JobSpec {
        max_attempts: Some(max_attempts),
        job_key: Some(format!("ai:{job_id}")),
        ..JobSpec::default()
    };
    jobs.enqueue(conn, DmReply { job_id }, spec).await
}

impl TaskHandler for DmReply {
    const IDENTIFIER: &'static str = "ai.dm_reply";

    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(state) = ctx.get_ext::<AiState>() else {
            return Err("执行器没有注册 AiState".to_owned());
        };
        let attempt = *ctx.job().attempts();
        let last = attempt >= *ctx.job().max_attempts();
        match run_job(state, self.job_id, attempt, last).await {
            Ok(RunOutcome::Retry(reason)) => Err(reason),
            Ok(outcome) => {
                tracing::debug!(job = self.job_id, ?outcome, "AI 私聊任务结束");
                Ok(())
            }
            Err(problem) => Err(format!("{problem:?}")),
        }
    }
}

/// 一次执行的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// 已经有结果（TRIG-037），什么都没做。
    AlreadyDone,
    Completed,
    /// 用户停止、重置上下文或撤回触发消息打断。
    Stopped,
    /// 没有生成（原因见 TRIG-036）。
    Skipped(&'static str),
    /// 这次失败，交给后台任务重试。
    Retry(String),
    /// 最终失败（重试用完或终态错误）。
    Failed(String),
}

/// 被跳过时给用户的提示（D-082 方案 A：不计费的系统消息）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Notice {
    GlobalDisabled,
    UserBudget,
    SiteBudget,
}

impl Notice {
    fn text(self) -> &'static str {
        match self {
            Self::GlobalDisabled => "AI 助手暂时关闭，稍后再来吧。",
            Self::UserBudget => "今日 AI 额度已用完，将在北京时间 04:00 重置。",
            Self::SiteBudget => "今天找 AI 的人太多，全站额度已用完，将在北京时间 04:00 重置。",
        }
    }

    fn from_denial(denial: Denial) -> Self {
        match denial {
            Denial::GlobalDisabled => Self::GlobalDisabled,
            Denial::UserBudget => Self::UserBudget,
            Denial::SiteBudget => Self::SiteBudget,
        }
    }
}

/// 以 AI 的名义写一条系统提示（不进 AI 记忆：历史只取文本消息）。
pub(crate) async fn notice(
    conn: &mut PgConnection,
    ids: &Snowflake,
    session: &SessionRow,
    notice: Notice,
    now: chrono::DateTime<Utc>,
    effects: &mut Effects,
) -> Result<(), Problem> {
    ik_messaging::interop::insert_ai_message(
        conn,
        ids,
        NewAiMessage {
            conversation_id: session.conversation_id,
            sender_id: session.ai_user_id,
            kind: "system",
            content: notice.text(),
            ai_state: None,
            ai_model_key: None,
        },
        now,
        effects,
    )
    .await?;
    Ok(())
}

/// 单次回复的模型：按 CTX-030 决议；指向没有配置的提供方时退回默认模型（LLM-011）。
pub(crate) fn pick_model<'a>(
    catalog: &'a Catalog,
    card: &'a RoleCard,
    override_key: Option<&str>,
    session_key: Option<&str>,
    has_provider: impl Fn(&str) -> bool,
) -> Option<&'a Model> {
    let chosen = catalog.resolve_model(card, override_key, session_key)?;
    if has_provider(&chosen.provider) {
        return Some(chosen);
    }
    tracing::warn!(
        model = %chosen.key,
        provider = %chosen.provider,
        "模型指向的提供方没有配置，退回默认模型"
    );
    catalog
        .default_model()
        .filter(|m| has_provider(&m.provider))
}

/// system prompt（LLM-003、GEN-071）：人格 + 输出格式要求 + 示例对话（few-shot 块，不伪装成历史）。
pub(crate) fn system_prompt(card: &RoleCard, mode: OutputMode) -> String {
    let mut prompt = if card.system_prompt.trim().is_empty() {
        format!("你是绳网论坛的 AI 角色「{}」。", card.display_name)
    } else {
        card.system_prompt.trim().to_owned()
    };
    prompt.push_str(match mode {
        OutputMode::Plain => {
            "\n\n【输出格式】只输出纯文本：不要使用 Markdown 标题、列表符号、加粗、代码块或 HTML。"
        }
        OutputMode::Markdown => "\n\n【输出格式】可以使用 Markdown（标题、列表、代码块、链接）。",
    });
    if !card.examples.is_empty() {
        prompt.push_str(
            "\n\n【示例对话】以下示例只用于说明语气和格式，不是真实的对话历史，不要当作之前的轮次；\
             被问到之前聊过什么时，只依据真实的对话轮次回答。",
        );
        for example in &card.examples {
            prompt.push_str(&format!(
                "\n用户：{}\n你：{}",
                example.user.trim(),
                example.assistant.trim()
            ));
        }
    }
    prompt
}

/// 对话消息（LLM-001、CTX-011）：用户消息带发送者昵称（`<昵称> 说：<正文>`），AI 自己的回复按 assistant。
pub(crate) fn build_messages(
    card: &RoleCard,
    history: &[HistoryItem],
    ai_user_id: i64,
    names: &HashMap<i64, String>,
) -> Vec<ChatMessage> {
    let mut messages = Vec::with_capacity(history.len() + 1);
    messages.push(ChatMessage::system(system_prompt(card, card.output_mode)));
    for item in history {
        if item.sender_id == ai_user_id {
            messages.push(ChatMessage::assistant(item.content.clone()));
        } else {
            let name = names.get(&item.sender_id).map_or("用户", String::as_str);
            messages.push(ChatMessage::user(format!("{name} 说：{}", item.content)));
        }
    }
    messages
}

/// 准备阶段的结果。
struct Prepared {
    job: JobRow,
    session: SessionRow,
    card: RoleCard,
    model: Model,
    default_model: Option<Model>,
    /// 这次快照里可用的上游。
    providers: Providers,
    reply_id: i64,
    hold: Hold,
    messages: Vec<ChatMessage>,
    /// 本次可用的工具（按角色卡组装，ADR 0004）。
    toolbox: Toolbox,
}

enum Prepare {
    Ready(Box<Prepared>),
    Done(RunOutcome),
}

/// 记一次跳过：已有的占位定稿为「已停止」，审计记原因。
async fn skip(
    tx: &mut PgConnection,
    job: &JobRow,
    reason: &'static str,
    effects: &mut Effects,
) -> Result<Prepare, Problem> {
    if let Some(reply_id) = job.reply_message_id {
        ik_messaging::interop::update_ai_message(
            tx,
            reply_id,
            STOPPED_TEXT,
            "stopped",
            None,
            None,
            effects,
        )
        .await?;
    }
    repo::record_outcome(
        tx,
        &JobOutcome {
            id: job.id,
            status: "skipped",
            reason: Some(reason),
            model_key: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            now: Utc::now(),
        },
    )
    .await?;
    tracing::info!(job = job.id, reason, "AI 私聊任务跳过");
    Ok(Prepare::Done(RunOutcome::Skipped(reason)))
}

async fn prepare(
    state: &AiState,
    job_id: i64,
    attempt: i16,
    effects: &mut Effects,
) -> Result<Prepare, Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let Some(job) = repo::lock_job(&mut tx, job_id).await? else {
        return Ok(Prepare::Done(RunOutcome::Skipped("missing_job")));
    };
    if matches!(job.status.as_str(), "completed" | "failed" | "skipped") {
        return Ok(Prepare::Done(RunOutcome::AlreadyDone));
    }
    let (Some(conversation_id), Some(trigger_id)) = (job.conversation_id, job.trigger_message_id)
    else {
        let outcome = skip(&mut tx, &job, "no_agent_target", effects).await?;
        tx.commit().await?;
        return Ok(outcome);
    };
    let Some(session) = repo::lock_session(&mut tx, conversation_id).await? else {
        let outcome = skip(&mut tx, &job, "no_agent_target", effects).await?;
        tx.commit().await?;
        return Ok(outcome);
    };
    repo::start_attempt(&mut tx, job_id, attempt, now).await?;

    macro_rules! skip_with {
        ($reason:expr) => {{
            let outcome = skip(&mut tx, &job, $reason, effects).await?;
            tx.commit().await?;
            return Ok(outcome);
        }};
    }

    if job.stop_requested_at.is_some() {
        skip_with!("cancelled");
    }
    if session.context_epoch != job.generation_epoch {
        skip_with!("context_reset");
    }
    let trigger = ik_messaging::interop::message(&mut tx, trigger_id).await?;
    let Some(trigger) = trigger.filter(|m| m.withdrawn_at.is_none() && m.kind == "text") else {
        skip_with!("trigger_withdrawn");
    };
    let sender = job.quota_user_id;
    if ik_auth::interop::active_users(&mut tx, &[sender])
        .await?
        .is_empty()
        || ik_messaging::interop::active_peer(&mut tx, conversation_id, sender)
            .await?
            .is_none()
    {
        skip_with!("sender_unavailable");
    }
    let catalog = state.catalog.get().await?;
    if !catalog.limits.global_enabled {
        notice(
            &mut tx,
            &state.ids,
            &session,
            Notice::GlobalDisabled,
            now,
            effects,
        )
        .await?;
        skip_with!("global_disabled");
    }
    let Some(card) = catalog
        .card_for_user(job.ai_user_id)
        .filter(|c| c.dm_enabled)
        .cloned()
    else {
        skip_with!("no_agent_target");
    };
    let Some(model) = pick_model(
        &catalog,
        &card,
        job.model_override.as_deref(),
        session.model_key.as_deref(),
        |p| catalog.providers.contains(p),
    )
    .cloned() else {
        tracing::error!(
            job = job_id,
            "没有可用的 AI 模型（检查默认模型与提供方配置）"
        );
        skip_with!("no_model");
    };
    let default_model = catalog
        .default_model()
        .filter(|m| catalog.providers.contains(&m.provider))
        .cloned();

    // 历史（LLM-001）：当前代际、触发消息为止、最近 N 条。
    let limit = i64::try_from(state.config.max_history_messages).unwrap_or(40);
    let history = ik_messaging::interop::history(
        &mut tx,
        conversation_id,
        session.context_from,
        trigger.created_at,
        limit,
    )
    .await?;
    let mut senders: Vec<i64> = history.iter().map(|h| h.sender_id).collect();
    senders.sort_unstable();
    senders.dedup();
    let names: HashMap<i64, String> = ik_auth::service::authors(&state.db, &senders)
        .await?
        .into_iter()
        .map(|(id, a)| (id, a.username))
        .collect();
    let mut messages = build_messages(&card, &history, job.ai_user_id, &names);
    let toolbox = Toolbox::for_card(state, &catalog, &card, &state.mcp);
    toolbox.annotate_system(&mut messages);

    // 预占（QUOTA-005～010）：调用模型之前，按本次模型的倍率估高。
    let prompt_chars = messages
        .iter()
        .map(|m| i64::try_from(m.text().chars().count()).unwrap_or(i64::MAX))
        .sum::<i64>()
        .saturating_add(toolbox.extra_estimate_tokens(state.config.max_tool_rounds));
    let amount = quota::estimate(
        state.config.estimated_context_tokens,
        state.config.max_tool_rounds,
        prompt_chars,
        model.weight(),
    );
    let ttl = chrono::Duration::seconds(i64::try_from(state.config.hold_ttl_secs).unwrap_or(900));
    let hold = match quota::reserve(
        &mut tx,
        &state.ids,
        catalog.limits,
        sender,
        amount,
        now,
        ttl,
    )
    .await?
    {
        Ok(hold) => hold,
        Err(denial) => {
            notice(
                &mut tx,
                &state.ids,
                &session,
                Notice::from_denial(denial),
                now,
                effects,
            )
            .await?;
            skip_with!(denial.as_str());
        }
    };

    // 占位（GEN-001/002/003）：同一任务重试复用同一条。
    let reply_id = match job.reply_message_id {
        Some(reply_id) => {
            // 重试：清空上次的正文与时间线（WF-006：seq 继续递增）。
            let reused = ik_messaging::interop::update_ai_message(
                &mut tx,
                reply_id,
                "",
                "streaming",
                Some(&model.key),
                Some(&serde_json::json!([])),
                effects,
            )
            .await?;
            if reused.is_none() {
                // 占位已被撤回（重新生成替换掉了）。
                skip_with!("cancelled");
            }
            reply_id
        }
        None => {
            let view = ik_messaging::interop::insert_ai_message(
                &mut tx,
                &state.ids,
                NewAiMessage {
                    conversation_id,
                    sender_id: job.ai_user_id,
                    kind: "text",
                    content: "",
                    ai_state: Some("streaming"),
                    ai_model_key: Some(&model.key),
                },
                now,
                effects,
            )
            .await?;
            repo::set_job_reply(&mut tx, job_id, view.id.get()).await?;
            view.id.get()
        }
    };
    tx.commit().await?;
    Ok(Prepare::Ready(Box::new(Prepared {
        job,
        session,
        card,
        model,
        default_model,
        providers: catalog.providers.clone(),
        reply_id,
        hold,
        messages,
        toolbox,
    })))
}

/// 执行一个私聊任务。`last_attempt` 时失败记为最终失败（TRIG-034）。
pub async fn run_job(
    state: &AiState,
    job_id: i64,
    attempt: i16,
    last_attempt: bool,
) -> Result<RunOutcome, Problem> {
    let mut effects = Effects::new();
    let prepared = prepare(state, job_id, attempt, &mut effects).await?;
    effects.dispatch(state.bus.as_ref());
    let prepared = match prepared {
        Prepare::Done(outcome) => return Ok(outcome),
        Prepare::Ready(prepared) => *prepared,
    };

    let conversation_id = prepared.session.conversation_id;
    let audience: Vec<i64> = {
        let mut conn = state.db.acquire().await?;
        ik_messaging::interop::active_members(&mut conn, conversation_id)
            .await?
            .into_iter()
            .filter(|&u| u != prepared.job.ai_user_id)
            .collect()
    };
    let ctx = StreamCtx {
        state,
        job_id,
        live: Some(Live {
            conversation_id,
            reply_id: prepared.reply_id,
            ai_user_id: prepared.job.ai_user_id,
            audience,
        }),
        providers: prepared.providers.clone(),
        mode: prepared.card.output_mode,
        max_chars: sanitize::DM_MAX_CHARS,
        tools: Some(ToolContext::new(state, prepared.job.quota_user_id).await?),
        toolbox: prepared.toolbox.clone(),
    };
    let mut workflow = Workflow::new(attempt, prepared.card.output_mode);
    let generated = generate(
        &ctx,
        &mut workflow,
        &prepared.model,
        prepared.default_model.as_ref(),
        &prepared.messages,
    )
    .await;
    finalize(
        state,
        &ctx,
        &prepared,
        &generated.model,
        generated.streamed,
        workflow,
        attempt,
        last_attempt,
    )
    .await
}

/// 停止时的正文（GEN-011）：有正文则「正文 + 空行 + （已停止）」，否则只有「（已停止）」。
pub(crate) fn stopped_text(content: &str, mode: OutputMode) -> String {
    let budget = sanitize::DM_MAX_CHARS - STOPPED_TEXT.chars().count() - 2;
    let text = sanitize::sanitize(content, mode, budget);
    if text.is_empty() {
        STOPPED_TEXT.to_owned()
    } else {
        format!("{text}\n\n{STOPPED_TEXT}")
    }
}

/// 失败时时间线上的说明（不外泄上游错误文本）。
fn failure_label(end: &End) -> &'static str {
    match end {
        End::Failed(LlmError::Timeout) | End::FirstDeltaTimeout => "生成超时",
        End::Failed(LlmError::RateLimited(_)) => "模型服务繁忙",
        End::Failed(_) => "模型服务出错",
        End::TooLarge => "回复过长",
        End::Finished | End::Stopped => "没有生成有效回复",
    }
}

/// 回答里引用的公开帖子补进时间线（WF-009）。模型编出来的、不公开的 id 忽略。
pub(crate) async fn cite_answer(
    state: &AiState,
    workflow: &mut Workflow,
    answer: &str,
) -> Result<(), Problem> {
    let ids: Vec<i64> = crate::workflow::cited_post_ids(answer)
        .into_iter()
        .filter(|id| !workflow.is_cited(*id))
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let mut conn = state.db.acquire().await?;
    let titles = ik_content::interop::public_post_titles(&mut conn, &ids).await?;
    for id in ids {
        if let Some(title) = titles.get(&id) {
            workflow.cite(id, title);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn finalize(
    state: &AiState,
    ctx: &StreamCtx<'_>,
    prepared: &Prepared,
    model: &Model,
    streamed: Streamed,
    mut workflow: Workflow,
    attempt: i16,
    last_attempt: bool,
) -> Result<RunOutcome, Problem> {
    let job_id = prepared.job.id;
    let mode = prepared.card.output_mode;
    let usage = streamed.charged;
    let charge = quota::weighted(usage.total(), model.weight());
    let answer = match streamed.end {
        End::Finished => sanitize::sanitize(&streamed.content, mode, sanitize::DM_MAX_CHARS),
        _ => String::new(),
    };
    if !answer.is_empty() {
        cite_answer(state, &mut workflow, &answer).await?;
    }
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let session = repo::lock_session(&mut tx, prepared.session.conversation_id).await?;
    let job = repo::lock_job(&mut tx, job_id)
        .await?
        .ok_or_else(Problem::internal)?;
    // 定稿前复查（CTX-025、GEN-006）：停止、重置、撤回触发消息都会置停止信号。
    let interrupted = job.stop_requested_at.is_some()
        || session.is_none_or(|s| s.context_epoch != job.generation_epoch);
    let finished_text = if interrupted { String::new() } else { answer };
    let (content, ai_state, status, reason, outcome) = if !finished_text.is_empty() {
        workflow.finish(usage);
        (
            finished_text,
            "done",
            "completed",
            None,
            RunOutcome::Completed,
        )
    } else if interrupted || matches!(streamed.end, End::Stopped) {
        workflow.finish(usage);
        (
            stopped_text(&streamed.content, mode),
            "stopped",
            "completed",
            Some("stopped".to_owned()),
            RunOutcome::Stopped,
        )
    } else {
        // 空回复（GEN-024/044）、上游错误、超时、超长都按失败；首帧超时与超长不重试（TRIG-034）。
        workflow.error(failure_label(&streamed.end));
        let (message, retryable) = match &streamed.end {
            End::Failed(error) => (error.to_string(), true),
            End::FirstDeltaTimeout => ("first_delta_timeout".to_owned(), false),
            End::TooLarge => ("upstream output too large".to_owned(), false),
            End::Finished | End::Stopped => ("empty reply after sanitize".to_owned(), true),
        };
        if retryable && !last_attempt {
            (
                FAILED_TEXT.to_owned(),
                "failed",
                "processing",
                Some(message.clone()),
                RunOutcome::Retry(message),
            )
        } else {
            (
                FAILED_TEXT.to_owned(),
                "failed",
                "failed",
                Some(message.clone()),
                RunOutcome::Failed(message),
            )
        }
    };
    ik_messaging::interop::update_ai_message(
        &mut tx,
        prepared.reply_id,
        &content,
        ai_state,
        Some(&model.key),
        Some(&workflow.events()),
        &mut effects,
    )
    .await?;
    quota::settle(
        &mut tx,
        &prepared.hold,
        &format!("job:{job_id}:{attempt}"),
        charge,
        now,
    )
    .await?;
    repo::record_outcome(
        &mut tx,
        &JobOutcome {
            id: job_id,
            status,
            reason: reason.as_deref(),
            model_key: Some(&model.key),
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            now,
        },
    )
    .await?;
    tx.commit().await?;
    // 收尾事件先于定稿的 `dm.message.edited` 推出。
    ctx.publish_workflow(&mut workflow);
    effects.dispatch(state.bus.as_ref());
    match &outcome {
        RunOutcome::Retry(reason) => {
            tracing::warn!(job = job_id, attempt, %reason, "AI 私聊回复失败，稍后重试");
        }
        RunOutcome::Failed(reason) => {
            tracing::error!(job = job_id, attempt, %reason, "AI 私聊回复最终失败");
        }
        _ => {}
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::catalog::Example;

    fn card() -> RoleCard {
        RoleCard {
            id: 1,
            slug: "fairy".into(),
            display_name: "Fairy".into(),
            bio: String::new(),
            avatar_media_id: None,
            bound_user_id: 9,
            enabled: true,
            knock_visible: true,
            dm_enabled: true,
            comment_enabled: true,
            sort_order: 0,
            output_mode: OutputMode::Plain,
            system_prompt: "你是 Fairy。".into(),
            examples: vec![Example {
                user: "你好".into(),
                assistant: "你好，绳匠。".into(),
            }],
            default_model_id: None,
            allowed_model_ids: Vec::new(),
            suggested_questions: Vec::new(),
            story_tools: false,
            mcp_server_ids: Vec::new(),
        }
    }

    #[test]
    fn prompt_has_persona_format_and_fewshot_block() {
        let prompt = system_prompt(&card(), OutputMode::Plain);
        assert!(prompt.starts_with("你是 Fairy。"));
        assert!(prompt.contains("只输出纯文本"));
        assert!(prompt.contains("不是真实的对话历史"));
        assert!(prompt.contains("用户：你好\n你：你好，绳匠。"));
    }

    #[test]
    fn history_maps_to_roles_with_names() {
        let at = Utc::now();
        let history = vec![
            HistoryItem {
                id: 1,
                sender_id: 5,
                content: "在吗".into(),
                created_at: at,
            },
            HistoryItem {
                id: 2,
                sender_id: 9,
                content: "在。".into(),
                created_at: at,
            },
        ];
        let names = HashMap::from([(5, "绳匠".to_owned())]);
        let messages = build_messages(&card(), &history, 9, &names);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1], ChatMessage::user("绳匠 说：在吗"));
        assert_eq!(messages[2], ChatMessage::assistant("在。"));
    }

    #[test]
    fn stopped_text_keeps_partial_answer() {
        assert_eq!(stopped_text("", OutputMode::Plain), "（已停止）");
        assert_eq!(
            stopped_text("**半句**", OutputMode::Plain),
            "半句\n\n（已停止）"
        );
        let long = stopped_text(&"字".repeat(3000), OutputMode::Plain);
        assert!(long.chars().count() <= sanitize::DM_MAX_CHARS);
    }
}
