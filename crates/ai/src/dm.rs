//! AI 私聊回复的生成（GEN 第 6.1～6.3 节、LLM 第 10 节）。
//!
//! 一个任务（`ai_jobs` 一行）由后台任务 `ai.dm_reply` 执行，三段各自独立：
//! 1. 准备（短事务，锁会话行）：复核触发条件（TRIG-023、CTX-024），决定模型，预占额度，建占位消息；
//! 2. 生成（事务之外）：流式调用上游，节流推送累计正文，轮询停止信号，首帧看门狗与总预算；
//! 3. 定稿（短事务，锁会话行）：写正文与状态、结算额度、记审计，提交后广播。
//!
//! 失败交给后台任务重试（同一条占位继续写，GEN-002/007）；首帧超时是终态，不重试（TRIG-034）。

use std::collections::HashMap;
use std::time::Duration;

use chrono::Utc;
use futures_util::StreamExt;
use ik_kernel::bus::{Audience, Effects, Event};
use ik_kernel::jobs::{IntoTaskHandlerResult, JobSpec, Jobs, TaskHandler, WorkerContext};
use ik_kernel::{Problem, Snowflake};
use ik_messaging::interop::{HistoryItem, NewAiMessage};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use tokio::time::Instant;

use crate::AiState;
use crate::catalog::{Catalog, Model, OutputMode, RoleCard};
use crate::llm::{ChatMessage, ChatProvider, ChatRequest, LlmError, StreamEvent, Usage};
use crate::quota::{self, Denial, Hold};
use crate::repo::{self, JobOutcome, JobRow, SessionRow};
use crate::sanitize;

/// 停止信号的轮询间隔（GEN-011）。
const STOP_POLL: Duration = Duration::from_millis(600);
/// 生成期间「正在输入」的间隔（GEN-005）。
const TYPING_EVERY: Duration = Duration::from_secs(2);
/// 剩余预算少于这么多时，首帧超时后不再退回默认模型（GEN-022）。
const FALLBACK_MIN_REMAINING: Duration = Duration::from_secs(20);
/// 上游输出（正文 + 思考）累计上限（GEN-023、LLM-012）。
const MAX_STREAM_CHARS: usize = 131_072;
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
pub(crate) fn system_prompt(card: &RoleCard) -> String {
    let mut prompt = if card.system_prompt.trim().is_empty() {
        format!("你是绳网论坛的 AI 角色「{}」。", card.display_name)
    } else {
        card.system_prompt.trim().to_owned()
    };
    prompt.push_str(match card.output_mode {
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
    messages.push(ChatMessage::system(system_prompt(card)));
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

fn prompt_tokens_estimate(state: &AiState, messages: &[ChatMessage]) -> i64 {
    state.config.estimated_context_tokens
        + messages
            .iter()
            .map(|m| quota::estimate_text_tokens(m.text()))
            .sum::<i64>()
}

/// 准备阶段的结果。
struct Prepared {
    job: JobRow,
    session: SessionRow,
    card: RoleCard,
    model: Model,
    default_model: Option<Model>,
    reply_id: i64,
    hold: Hold,
    messages: Vec<ChatMessage>,
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
        |p| state.providers.contains(p),
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
        .filter(|m| state.providers.contains(&m.provider))
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
    let messages = build_messages(&card, &history, job.ai_user_id, &names);

    // 预占（QUOTA-005～010）：调用模型之前，按本次模型的倍率估高。
    let prompt_chars = messages
        .iter()
        .map(|m| i64::try_from(m.text().chars().count()).unwrap_or(i64::MAX))
        .sum();
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
            let reused = ik_messaging::interop::update_ai_message(
                &mut tx,
                reply_id,
                "",
                "streaming",
                Some(&model.key),
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
        reply_id,
        hold,
        messages,
    })))
}

/// 生成阶段怎么结束的。
#[derive(Debug)]
enum End {
    Finished,
    Stopped,
    FirstDeltaTimeout,
    Failed(LlmError),
    TooLarge,
}

/// 一次流式调用的产出。
#[derive(Debug)]
struct Streamed {
    content: String,
    received_chars: usize,
    usage: Option<Usage>,
    /// 上游已经接受请求（开始计费的可能）。
    started: bool,
    end: End,
}

struct StreamCtx<'a> {
    state: &'a AiState,
    job_id: i64,
    conversation_id: i64,
    reply_id: i64,
    ai_user_id: i64,
    /// 推送对象（会话里的人类成员）。
    audience: Vec<i64>,
    mode: OutputMode,
}

impl StreamCtx<'_> {
    fn publish(&self, kind: &'static str, data: &serde_json::Value) {
        if self.audience.is_empty() {
            return;
        }
        match Event::new(kind, Audience::Users(self.audience.clone().into()), data) {
            Ok(event) => self.state.bus.publish(event),
            Err(problem) => tracing::warn!(?problem, kind, "构造实时事件失败"),
        }
    }

    async fn stop_requested(&self) -> bool {
        match self.state.db.acquire().await {
            Ok(mut conn) => repo::stop_requested(&mut conn, self.job_id)
                .await
                .map(|s| s.is_some())
                .unwrap_or(false),
            Err(_) => false,
        }
    }
}

/// 一次流式调用（LLM-004）：节流推送累计正文（GEN-004），每 600ms 查停止信号（GEN-011），
/// 每 2 秒推「正在输入」（GEN-005），首帧看门狗（GEN-021）与总预算（GEN-020）。
async fn stream_once(
    ctx: &StreamCtx<'_>,
    provider: &dyn ChatProvider,
    request: &ChatRequest,
    deadline: Instant,
    first_delta: Option<Duration>,
) -> Streamed {
    let mut out = Streamed {
        content: String::new(),
        received_chars: 0,
        usage: None,
        started: false,
        end: End::Finished,
    };
    let first_deadline = first_delta.map(|d| (Instant::now() + d).min(deadline));
    let opened = tokio::time::timeout_at(deadline, provider.stream(request)).await;
    let mut stream = match opened {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            out.end = End::Failed(error);
            return out;
        }
        Err(_) => {
            out.end = End::Failed(LlmError::Timeout);
            return out;
        }
    };
    out.started = true;
    let mut stop_tick = tokio::time::interval(STOP_POLL);
    let mut typing_tick = tokio::time::interval(TYPING_EVERY);
    let mut got_content = false;
    let mut pushed_chars = 0usize;
    let min_chars = ctx.state.config.stream_min_chars;
    let typing = serde_json::json!({
        "conversationId": ctx.conversation_id.to_string(),
        "userId": ctx.ai_user_id.to_string(),
    });
    loop {
        let watchdog = async {
            match first_deadline {
                Some(at) if !got_content => tokio::time::sleep_until(at).await,
                _ => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            item = stream.next() => {
                let Some(item) = item else { break };
                let event = match item {
                    Ok(event) => event,
                    Err(error) => {
                        out.end = End::Failed(error);
                        break;
                    }
                };
                match event {
                    StreamEvent::Reasoning(text) => {
                        got_content = true;
                        out.received_chars += text.chars().count();
                    }
                    StreamEvent::Content(text) => {
                        got_content = true;
                        out.received_chars += text.chars().count();
                        out.content.push_str(&text);
                        let total = out.content.chars().count();
                        if total >= pushed_chars + min_chars {
                            pushed_chars = total;
                            let shown = sanitize::sanitize(&out.content, ctx.mode, sanitize::DM_MAX_CHARS);
                            ctx.publish("dm.message.delta", &serde_json::json!({
                                "conversationId": ctx.conversation_id.to_string(),
                                "messageId": ctx.reply_id.to_string(),
                                "content": shown,
                            }));
                        }
                    }
                    StreamEvent::ToolCallDelta { .. } => got_content = true,
                    StreamEvent::Usage(usage) => out.usage = Some(usage),
                    StreamEvent::Finish(_) => {}
                }
                if out.received_chars > MAX_STREAM_CHARS {
                    out.end = End::TooLarge;
                    break;
                }
            }
            _ = stop_tick.tick() => {
                if ctx.stop_requested().await {
                    out.end = End::Stopped;
                    break;
                }
            }
            _ = typing_tick.tick() => ctx.publish("dm.typing", &typing),
            () = tokio::time::sleep_until(deadline) => {
                out.end = End::Failed(LlmError::Timeout);
                break;
            }
            () = watchdog => {
                out.end = End::FirstDeltaTimeout;
                break;
            }
        }
    }
    // 丢弃流即断开上游连接：停止也终止服务端那次生成（GEN-012）。
    drop(stream);
    out
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
        conversation_id,
        reply_id: prepared.reply_id,
        ai_user_id: prepared.job.ai_user_id,
        audience,
        mode: prepared.card.output_mode,
    };
    let budget = Duration::from_secs(state.config.reply_timeout_secs);
    let deadline = Instant::now() + budget;
    let first_delta =
        Some(Duration::from_secs(state.config.first_delta_timeout_secs)).filter(|d| !d.is_zero());

    let mut model = prepared.model.clone();
    let mut streamed = match state.providers.get(&model.provider) {
        Ok(provider) => {
            let request = ChatRequest {
                model: model.provider_model.clone(),
                messages: prepared.messages.clone(),
                tools: Vec::new(),
            };
            stream_once(&ctx, provider.as_ref(), &request, deadline, first_delta).await
        }
        Err(error) => Streamed {
            content: String::new(),
            received_chars: 0,
            usage: None,
            started: false,
            end: End::Failed(error),
        },
    };
    let mut prompt_charged = Usage::default();
    // 首帧超时：剩余预算够、且当前不是默认模型时，退回默认模型重试一次且不挂看门狗（GEN-022）。
    if matches!(streamed.end, End::FirstDeltaTimeout)
        && let Some(fallback) = prepared
            .default_model
            .as_ref()
            .filter(|d| d.key != model.key)
        && deadline.saturating_duration_since(Instant::now()) >= FALLBACK_MIN_REMAINING
        && let Ok(provider) = state.providers.get(&fallback.provider)
    {
        tracing::warn!(job = job_id, from = %model.key, to = %fallback.key, "首帧超时，退回默认模型");
        // 第一次调用已经开始，按估算计入（D-083 方案 C）。
        prompt_charged = charge_usage(state, &prepared.messages, &streamed);
        model = fallback.clone();
        let request = ChatRequest {
            model: model.provider_model.clone(),
            messages: prepared.messages.clone(),
            tools: Vec::new(),
        };
        streamed = stream_once(&ctx, provider.as_ref(), &request, deadline, None).await;
    }
    finalize(
        state,
        &prepared,
        &model,
        streamed,
        prompt_charged,
        attempt,
        last_attempt,
    )
    .await
}

/// 计费用量（LLM-006、GEN-080/081、D-083 方案 C）：上游报了就用上游的；没报但调用已开始，按估算；
/// 没开始记 0。
fn charge_usage(state: &AiState, messages: &[ChatMessage], streamed: &Streamed) -> Usage {
    if let Some(usage) = streamed.usage.filter(|u| !u.is_zero()) {
        return usage;
    }
    if !streamed.started {
        return Usage::default();
    }
    tracing::info!("provider reported no usage, charging estimate");
    Usage {
        prompt_tokens: prompt_tokens_estimate(state, messages),
        completion_tokens: quota::estimate_text_tokens(&streamed.content)
            .max(i64::try_from(streamed.received_chars / 4).unwrap_or(0)),
    }
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

#[allow(clippy::too_many_arguments)]
async fn finalize(
    state: &AiState,
    prepared: &Prepared,
    model: &Model,
    streamed: Streamed,
    earlier: Usage,
    attempt: i16,
    last_attempt: bool,
) -> Result<RunOutcome, Problem> {
    let job_id = prepared.job.id;
    let mode = prepared.card.output_mode;
    let usage = {
        let current = charge_usage(state, &prepared.messages, &streamed);
        Usage {
            prompt_tokens: current.prompt_tokens + earlier.prompt_tokens,
            completion_tokens: current.completion_tokens + earlier.completion_tokens,
        }
    };
    let charge = quota::weighted(usage.total(), model.weight());
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
    let finished_text = match streamed.end {
        End::Finished if !interrupted => {
            sanitize::sanitize(&streamed.content, mode, sanitize::DM_MAX_CHARS)
        }
        _ => String::new(),
    };
    let (content, ai_state, status, reason, outcome) = if !finished_text.is_empty() {
        (
            finished_text,
            "done",
            "completed",
            None,
            RunOutcome::Completed,
        )
    } else if interrupted || matches!(streamed.end, End::Stopped) {
        (
            stopped_text(&streamed.content, mode),
            "stopped",
            "completed",
            Some("stopped".to_owned()),
            RunOutcome::Stopped,
        )
    } else {
        // 空回复（GEN-024/044）、上游错误、超时、超长都按失败；首帧超时与超长不重试（TRIG-034）。
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
        }
    }

    #[test]
    fn prompt_has_persona_format_and_fewshot_block() {
        let prompt = system_prompt(&card());
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
