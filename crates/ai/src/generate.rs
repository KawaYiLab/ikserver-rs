//! 一次回复的生成（私信与评论共用，LLM 第 10 节）：流式调用上游、工具调用循环、首帧看门狗与总预算、
//! 退回默认模型、计费用量。全部在事务之外进行（LLM-009）。
//!
//! - 工具循环（LLM-005）：模型返回 tool_calls 时在进程内执行（[`crate::tools`]），结果回灌后继续请求；
//!   最多 `ai.max_tool_rounds` 轮，用完后不再提供工具并要求模型基于已有结果作答；
//! - 轮内重试（GEN-025）：上游限流、5xx、断流时在总预算内重发这一轮（最多 [`MAX_ROUND_RETRIES`] 次），
//!   之前轮次的工具结果保留；已经推给用户的正文不清空，让模型接着写；
//! - 超长续写（GEN-026）：正文因输出上限（`finish_reason = length`）被截断时接着写一次；工具参数被截断时
//!   不执行，把原因告诉模型；
//! - 上下文预算（LLM-018，[`crate::context`]）：超出模型窗口时裁剪最早的历史、省略较早的工具结果；
//! - 用量（LLM-006）：每一轮请求单独计费再累加；没报用量的轮次按估算（D-083 方案 C）；
//! - 私信有推送目标（[`Live`]）：推累计正文（GEN-004）、「正在输入」（GEN-005）、工作流事件（WF-003），
//!   并等停止信号（GEN-011，[`crate::stops`]）；评论没有。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::time::Duration;

use futures_util::future::{BoxFuture, Shared};
use futures_util::stream::FuturesUnordered;
use futures_util::{FutureExt, StreamExt};
use ik_kernel::bus::{Audience, Event};
use tokio::time::Instant;

use crate::AiState;
use crate::catalog::{CardParams, Model, OutputMode};
use crate::context;
use crate::llm::{
    ChatMessage, ChatProvider, ChatRequest, LlmError, Providers, StreamEvent, ToolCall,
    ToolCallAssembler, Usage,
};
use crate::quota;
use crate::repo;
use crate::sanitize;
use crate::stops::StopWatch;
use crate::toolbox::Toolbox;
use crate::tools::{self, ToolContext};
use crate::workflow::Workflow;

/// 生成期间「正在输入」的间隔（GEN-005）。
const TYPING_EVERY: Duration = Duration::from_secs(2);
/// `dm.message.append` 每隔这么多条带一次全文快照，供中途加入或丢了事件的客户端对齐（GEN-009）。
const APPEND_SNAPSHOT_EVERY: u64 = 64;
/// 正文增长不足 `stream_min_chars` 时，距上次推送超过这么久也推一次，免得尾巴卡住（GEN-004）。
const STREAM_PUSH_EVERY: Duration = Duration::from_millis(150);
/// 剩余预算少于这么多时，首帧超时后不再退回默认模型（GEN-022）。
const FALLBACK_MIN_REMAINING: Duration = Duration::from_secs(20);
/// 上游输出（正文 + 思考 + 工具参数）累计上限（GEN-023、LLM-012）。
const MAX_STREAM_CHARS: usize = 131_072;
/// 一次回复里工具结果最多附多少张图给识图模型（LLM-015）。
const MAX_TOOL_IMAGES: usize = 8;
/// 工具结果附图时那条消息的说明（LLM-015）。
const TOOL_IMAGES_NOTE: &str = "（系统附上刚才工具读到的帖子图片，不是用户发的，按需参考）";
/// 工具轮数用完后追加给模型的要求（LLM-005）。
const NO_MORE_TOOLS: &str =
    "（系统提示）工具调用次数已达上限。请不要再调用工具，基于已有结果直接回答。";
/// 一轮请求失败后最多重发几次（GEN-025）。
const MAX_ROUND_RETRIES: u32 = 2;
/// 剩余预算少于这么多时不再重发。
const RETRY_MIN_REMAINING: Duration = Duration::from_secs(15);
/// 重发前的等待：普通错误 1s、2s，限流 3s、6s。
const RETRY_BASE: Duration = Duration::from_secs(1);
const RATE_LIMITED_RETRY_BASE: Duration = Duration::from_secs(3);
/// 正文被输出上限截断后最多接着写几次（GEN-026）。
const MAX_LENGTH_CONTINUES: u32 = 1;
/// 断流后接着写的要求（GEN-025）。
const RESUME_NOTE: &str = "（系统提示）你上一条回复传到一半断开了，上面是已经发出去的部分。请从断开处直接接着写，不要重复已写的内容，也不要提到中断。";
/// 被输出上限截断后接着写的要求（GEN-026）。
const LENGTH_NOTE: &str = "（系统提示）你上一条回复超出了单次输出长度，上面是已经发出去的部分。请从截断处直接接着写完，不要重复，尽快收尾。";
/// 工具参数被输出上限截断时给模型的结果（GEN-026）。
const TRUNCATED_ARGS: &str =
    "这次调用的参数因输出长度上限被截断，没有执行。请把参数写短一些再调用。";

/// 生成怎么结束的。
#[derive(Debug)]
pub(crate) enum End {
    Finished,
    Stopped,
    FirstDeltaTimeout,
    Failed(LlmError),
    TooLarge,
}

/// 一次回复（可能多轮）的产出。
#[derive(Debug)]
pub(crate) struct Streamed {
    /// 各轮正文按顺序拼起来（私信展示的全文）。
    pub content: String,
    /// 最后一轮的正文（评论只用它：前几轮多是「我查一下」之类的过程话）。
    pub final_text: String,
    pub received_chars: usize,
    /// 各轮的计费用量之和（LLM-006）。
    pub charged: Usage,
    /// 上游至少接受过一次请求（开始计费的可能）。
    pub started: bool,
    pub tool_calls: usize,
    /// 跑了几轮工具。
    pub tool_rounds: u32,
    /// 轮内重发了几次（GEN-025）。
    pub retries: u32,
    /// 第一帧（正文、思考或工具调用）到达的时刻（OBS-001）。
    pub first_delta_at: Option<Instant>,
    /// 最后一份请求（调用记录开着时，OBS-002）。
    pub request: Option<ChatRequest>,
    /// 从开始生成到第一帧、到结束各用了多久（OBS-001）。
    pub first_delta: Option<Duration>,
    pub elapsed: Duration,
    pub end: End,
}

impl Streamed {
    fn failed(error: LlmError) -> Self {
        Self {
            content: String::new(),
            final_text: String::new(),
            received_chars: 0,
            charged: Usage::default(),
            started: false,
            tool_calls: 0,
            tool_rounds: 0,
            retries: 0,
            first_delta_at: None,
            request: None,
            first_delta: None,
            elapsed: Duration::ZERO,
            end: End::Failed(error),
        }
    }

    pub(crate) fn metrics(&self) -> repo::JobMetrics {
        let ms = |d: Duration| i32::try_from(d.as_millis()).unwrap_or(i32::MAX);
        repo::JobMetrics {
            cached_tokens: self.charged.cached_tokens,
            first_delta_ms: self.first_delta.map(ms),
            elapsed_ms: ms(self.elapsed),
            tool_rounds: i16::try_from(self.tool_rounds).unwrap_or(i16::MAX),
            tool_calls: i16::try_from(self.tool_calls).unwrap_or(i16::MAX),
            retries: i16::try_from(self.retries).unwrap_or(i16::MAX),
        }
    }
}

/// 定稿事务里记下这次尝试的指标与调用记录（OBS-001/002）。
pub(crate) async fn record_run(
    conn: &mut sqlx::PgConnection,
    job_id: i64,
    attempt: i16,
    model_key: &str,
    streamed: &Streamed,
    outcome: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> sqlx::Result<()> {
    repo::record_metrics(conn, job_id, &streamed.metrics()).await?;
    if let Some(request) = &streamed.request {
        repo::insert_trace(
            conn,
            &repo::NewTrace {
                job_id,
                attempt,
                model_key,
                request: &request.body(),
                reply: &streamed.content,
                outcome,
                now,
            },
        )
        .await?;
    }
    Ok(())
}

/// 私信回复的实时推送目标。评论回复没有（GEN-030：一次性发出）。
pub(crate) struct Live {
    pub conversation_id: i64,
    pub reply_id: i64,
    pub ai_user_id: i64,
    /// 推送对象（会话里的人类成员）。
    pub audience: Vec<i64>,
    /// 已经用 `dm.message.append` 推出去的正文（跨工具调用的各轮累计）。
    pub appended: parking_lot::Mutex<Appended>,
}

#[derive(Default)]
pub(crate) struct Appended {
    seq: u64,
    text: String,
}

pub(crate) struct StreamCtx<'a> {
    pub state: &'a AiState,
    pub job_id: i64,
    /// 有推送目标时推送累计正文、「正在输入」与工作流事件，并等停止信号。
    pub live: Option<Live>,
    /// 可用的上游（准备阶段那份配置快照里的）。
    pub providers: Providers,
    pub mode: OutputMode,
    /// 正文上限（私信 2500，评论 1500）。
    pub max_chars: usize,
    /// 工具执行上下文（查看者 = 触发者）。None 时不提供工具。
    pub tools: Option<ToolContext>,
    /// 本次可用的工具（论坛 + 角色卡启用的剧情库与站外服务，ADR 0004）。
    pub toolbox: Toolbox,
    /// 角色卡覆盖的采样参数（LLM-016）。
    pub card_params: Option<CardParams>,
    /// 记下最后一份请求，写调用记录（OBS-002）。
    pub trace: bool,
}

impl StreamCtx<'_> {
    fn publish(&self, kind: &'static str, data: &serde_json::Value) {
        let Some(live) = self.live.as_ref().filter(|l| !l.audience.is_empty()) else {
            return;
        };
        match Event::new(kind, Audience::Users(live.audience.clone().into()), data) {
            Ok(event) => self.state.bus.publish(event),
            Err(problem) => tracing::warn!(?problem, kind, "构造实时事件失败"),
        }
    }

    /// 推送新产生的工作流事件（`dm.message.workflow`，WF-003）。
    pub(crate) fn publish_workflow(&self, workflow: &mut Workflow) {
        let events = workflow.take_pending();
        let Some(live) = self.live.as_ref() else {
            return;
        };
        for event in events {
            self.publish(
                "dm.message.workflow",
                &serde_json::json!({
                    "conversationId": live.conversation_id.to_string(),
                    "messageId": live.reply_id.to_string(),
                    "event": event,
                }),
            );
        }
    }

    /// 兜底查库（GEN-011）：渠道授权已撤销，或私信任务被要求停止。
    async fn stop_requested(&self) -> bool {
        if !self.toolbox.authorized(self.state).await {
            return true;
        }
        if self.live.is_none() {
            return false;
        }
        match self.state.db.acquire().await {
            Ok(mut conn) => repo::stop_requested(&mut conn, self.job_id)
                .await
                .map(|s| s.is_some())
                .unwrap_or(false),
            Err(_) => false,
        }
    }
}

/// 收到停止通知时返回；没有登记时永远不返回。
async fn stop_signal(stop: Option<&StopWatch>) {
    match stop {
        Some(stop) => stop.notified().await,
        None => std::future::pending().await,
    }
}

/// 一轮请求的产出。
struct Round {
    content: String,
    /// 这一轮的思考原文：只在带工具调用时回传给上游（LLM-013），不下发、不落库。
    reasoning: String,
    received_chars: usize,
    usage: Option<Usage>,
    started: bool,
    tool_calls: Vec<ToolCall>,
    /// 上游给的结束原因（`stop`、`tool_calls`、`length`……）。
    finish: Option<String>,
    first_delta_at: Option<Instant>,
    end: End,
}

/// 正文接在之前的后面：接着写（`glue`）时直接相连，新的一轮之间空一行。
fn combine(prefix: &str, current: &str, glue: bool) -> String {
    if glue {
        format!("{prefix}{current}")
    } else {
        join_rounds(prefix, current)
    }
}

/// 一轮流式请求（LLM-004）：节流推送累计正文（GEN-004），等停止信号并兜底查库（GEN-011），
/// 每 2 秒推「正在输入」（GEN-005），首帧看门狗（GEN-021）与总预算（GEN-020）。
/// `prefix` 是之前各轮的正文（推送的是累计全文），`glue` 表示本轮接着上一轮没写完的往下写，
/// `already` 是之前各轮收到的字符数。
#[allow(clippy::too_many_arguments)]
async fn stream_round(
    ctx: &StreamCtx<'_>,
    stop: Option<&StopWatch>,
    workflow: &mut Workflow,
    provider: &dyn ChatProvider,
    request: &ChatRequest,
    deadline: Instant,
    first_delta: Option<Duration>,
    prefix: &str,
    glue: bool,
    already: usize,
) -> Round {
    let mut out = Round {
        content: String::new(),
        reasoning: String::new(),
        received_chars: 0,
        usage: None,
        started: false,
        tool_calls: Vec::new(),
        finish: None,
        first_delta_at: None,
        end: End::Finished,
    };
    let first_deadline = first_delta.map(|d| (Instant::now() + d).min(deadline));
    if !ctx.toolbox.authorized(ctx.state).await {
        out.end = End::Stopped;
        return out;
    }
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
    let mut stop_tick = tokio::time::interval(ctx.state.stops.poll_interval());
    let mut typing_tick = tokio::time::interval(TYPING_EVERY);
    let mut got_content = false;
    let mut pushed_chars = 0usize;
    let mut pushed_at = Instant::now();
    let mut assembler = ToolCallAssembler::default();
    let min_chars = ctx.state.config.stream_min_chars;
    let (conversation_id, reply_id, ai_user_id) = ctx
        .live
        .as_ref()
        .map_or((0, 0, 0), |l| (l.conversation_id, l.reply_id, l.ai_user_id));
    let typing = serde_json::json!({
        "conversationId": conversation_id.to_string(),
        "userId": ai_user_id.to_string(),
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
                if !matches!(event, StreamEvent::Usage(_) | StreamEvent::Finish(_))
                    && out.first_delta_at.is_none()
                {
                    out.first_delta_at = Some(Instant::now());
                }
                match event {
                    StreamEvent::Reasoning(text) => {
                        got_content = true;
                        let chars = text.chars().count();
                        out.received_chars += chars;
                        out.reasoning.push_str(&text);
                        workflow.reasoning(chars);
                    }
                    StreamEvent::Content(text) => {
                        got_content = true;
                        out.received_chars += text.chars().count();
                        out.content.push_str(&text);
                        workflow.content(&text);
                        if ctx.live.is_some() {
                            let shown = shown_text(ctx, prefix, &out.content, glue);
                            publish_append(ctx, &shown, conversation_id, reply_id);
                            let total = out.content.chars().count();
                            let due = total >= pushed_chars + min_chars
                                || (total > pushed_chars && pushed_at.elapsed() >= STREAM_PUSH_EVERY);
                            if due {
                                pushed_chars = total;
                                pushed_at = Instant::now();
                                publish_delta(ctx, &shown, conversation_id, reply_id);
                            }
                        }
                    }
                    StreamEvent::ToolCallDelta { index, id, name, arguments } => {
                        got_content = true;
                        out.received_chars += arguments.as_deref().map_or(0, |a| a.chars().count());
                        assembler.push(index, id, name, arguments);
                    }
                    StreamEvent::Usage(usage) => out.usage = Some(usage),
                    StreamEvent::Finish(reason) => out.finish = Some(reason),
                }
                ctx.publish_workflow(workflow);
                if already + out.received_chars > MAX_STREAM_CHARS {
                    out.end = End::TooLarge;
                    break;
                }
            }
            () = stop_signal(stop) => {
                out.end = End::Stopped;
                break;
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
    // 把节流攒下的尾巴推出去：正常结束时后面可能还要跑工具，断流时下一轮要接着写，都不能让它等到定稿。
    if ctx.live.is_some()
        && matches!(out.end, End::Finished | End::Failed(_))
        && out.content.chars().count() > pushed_chars
    {
        let shown = shown_text(ctx, prefix, &out.content, glue);
        publish_delta(ctx, &shown, conversation_id, reply_id);
    }
    workflow.flush();
    ctx.publish_workflow(workflow);
    if matches!(out.end, End::Finished) {
        out.tool_calls = assembler
            .finish()
            .into_iter()
            .filter(|c| !c.name.is_empty())
            .collect();
    }
    out
}

/// 到目前为止要展示的正文：前几轮 + 本轮，整体清洗。
fn shown_text(ctx: &StreamCtx<'_>, prefix: &str, content: &str, glue: bool) -> String {
    sanitize::sanitize(&combine(prefix, content, glue), ctx.mode, ctx.max_chars)
}
/// 逐块推送正文增量（GEN-009）：清洗后的正文只是在上次基础上往后长，就只发新增的 `text`；
/// 清洗改动了前文（标签闭合后被剥掉等）、第一条和每隔 [`APPEND_SNAPSHOT_EVERY`] 条发全文 `content`。
/// `seq` 从 1 连续递增，客户端发现断号就等下一份全文。
fn publish_append(ctx: &StreamCtx<'_>, shown: &str, conversation_id: i64, reply_id: i64) {
    let Some(live) = ctx.live.as_ref() else {
        return;
    };
    let Some(mut payload) = next_append(&mut live.appended.lock(), shown) else {
        return;
    };
    payload["conversationId"] = conversation_id.to_string().into();
    payload["messageId"] = reply_id.to_string().into();
    ctx.publish("dm.message.append", &payload);
}

/// 算下一条 `dm.message.append` 的 `{seq, text | content}`；正文没变返回 None。
fn next_append(appended: &mut Appended, shown: &str) -> Option<serde_json::Value> {
    if appended.text == shown {
        return None;
    }
    appended.seq += 1;
    let seq = appended.seq;
    let payload = if seq % APPEND_SNAPSHOT_EVERY != 1 && shown.starts_with(appended.text.as_str()) {
        serde_json::json!({ "seq": seq, "text": &shown[appended.text.len()..] })
    } else {
        serde_json::json!({ "seq": seq, "content": shown })
    };
    shown.clone_into(&mut appended.text);
    Some(payload)
}

/// 推送累计正文（GEN-004：累计全文、已清洗、不落库）。旧客户端（安卓）用。
fn publish_delta(ctx: &StreamCtx<'_>, shown: &str, conversation_id: i64, reply_id: i64) {
    ctx.publish(
        "dm.message.delta",
        &serde_json::json!({
            "conversationId": conversation_id.to_string(),
            "messageId": reply_id.to_string(),
            "content": shown,
        }),
    );
}

fn join_rounds(prefix: &str, current: &str) -> String {
    match (prefix.trim().is_empty(), current.trim().is_empty()) {
        (true, _) => current.to_owned(),
        (false, true) => prefix.to_owned(),
        (false, false) => format!("{prefix}\n\n{current}"),
    }
}

/// 一轮的计费用量（LLM-006、GEN-080/081、D-083 方案 C）：上游报了就用上游的；没报但调用已开始，
/// 按估算；没开始记 0。
fn charge_round(state: &AiState, messages: &[ChatMessage], round: &Round) -> Usage {
    if let Some(usage) = round.usage.filter(|u| !u.is_zero()) {
        return usage;
    }
    if !round.started {
        return Usage::default();
    }
    tracing::info!("provider reported no usage, charging estimate");
    Usage {
        prompt_tokens: prompt_tokens_estimate(state, messages),
        completion_tokens: quota::estimate_text_tokens(&round.content)
            .max(i64::try_from(round.received_chars / 4).unwrap_or(0)),
        cached_tokens: 0,
    }
}

pub(crate) fn prompt_tokens_estimate(state: &AiState, messages: &[ChatMessage]) -> i64 {
    state.config.estimated_context_tokens
        + messages
            .iter()
            .map(|m| {
                quota::estimate_text_tokens(m.text())
                    + i64::try_from(m.image_count()).unwrap_or(0) * quota::IMAGE_TOKENS
            })
            .sum::<i64>()
}

fn add(a: Usage, b: Usage) -> Usage {
    Usage {
        prompt_tokens: a.prompt_tokens.saturating_add(b.prompt_tokens),
        completion_tokens: a.completion_tokens.saturating_add(b.completion_tokens),
        cached_tokens: a.cached_tokens.saturating_add(b.cached_tokens),
    }
}

/// `search_posts` 的检索词（工作流事件用）。
fn search_query(arguments: &str) -> Option<String> {
    let args = tools::parse_args(arguments);
    match args.get("queries") {
        Some(serde_json::Value::Array(items)) if !items.is_empty() => Some(
            items
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("、"),
        ),
        _ => args
            .get("query")
            .and_then(|q| q.as_str())
            .map(|q| q.trim().to_owned()),
    }
}

type ForumCallKey = (String, String);
type SharedToolCall<'a> = Shared<BoxFuture<'a, tools::ToolOutput>>;

/// 仅在本次回复内复用论坛只读工具；记忆写入、剧情库和站外工具始终独立执行。
/// 同轮重复调用共享执行，跨轮只复用成功结果。参数键顺序和 JSON 空白不影响命中。
fn share_forum_call<'a>(
    call: &ToolCall,
    cache: &mut HashMap<ForumCallKey, SharedToolCall<'a>>,
    execute: impl Future<Output = tools::ToolOutput> + Send + 'a,
) -> (SharedToolCall<'a>, bool) {
    let key = tools::TOOL_NAMES.contains(&call.name.as_str()).then(|| {
        let args: BTreeMap<_, _> = tools::parse_args(&call.arguments).into_iter().collect();
        (
            call.name.clone(),
            serde_json::to_string(&args).unwrap_or_default(),
        )
    });
    if let Some(output) = key.as_ref().and_then(|key| cache.get(key))
        && output.peek().is_none_or(|result| !result.is_error)
    {
        return (output.clone(), true);
    }
    let output = execute.boxed().shared();
    if let Some(key) = key {
        cache.insert(key, output.clone());
    }
    (output, false)
}

/// 截断时工具参数是否还能用：必须是完整的 JSON 对象。
fn complete_args(arguments: &str) -> bool {
    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(arguments).is_ok()
}

/// 重发前等一会儿（GEN-025）；期间收到停止返回 true。
async fn pause(ctx: &StreamCtx<'_>, stop: Option<&StopWatch>, delay: Duration) -> bool {
    tokio::select! {
        () = tokio::time::sleep(delay) => ctx.stop_requested().await,
        () = stop_signal(stop) => true,
    }
}

/// 用一个模型跑完整个工具循环。
#[allow(clippy::too_many_lines)]
async fn run_rounds(
    ctx: &StreamCtx<'_>,
    stop: Option<&StopWatch>,
    workflow: &mut Workflow,
    model: &Model,
    base: &[ChatMessage],
    deadline: Instant,
    first_delta: Option<Duration>,
) -> Streamed {
    let state = ctx.state;
    let provider = match ctx.providers.get(&model.provider) {
        Ok(provider) => provider,
        Err(error) => return Streamed::failed(error),
    };
    let max_rounds = if ctx.tools.is_some() {
        state.config.max_tool_rounds
    } else {
        0
    };
    let specs = if max_rounds > 0 {
        ctx.toolbox.specs()
    } else {
        Vec::new()
    };
    let params = model.gen_params(ctx.card_params.as_ref());
    let budget = context::budget(model, &specs);
    // 不识图的模型（包括首帧超时后退回的默认模型）收不到图片，只看正文里的「[图片]」占位（LLM-015）。
    let mut messages: Vec<ChatMessage> = if model.vision {
        base.to_vec()
    } else {
        base.iter().map(ChatMessage::without_images).collect()
    };
    let mut keep = messages.len();
    let trimmed = context::trim_history(
        &mut messages,
        context::history_target(budget, !specs.is_empty()),
        keep,
    );
    keep -= trimmed;
    if trimmed > 0 {
        tracing::info!(
            job = ctx.job_id,
            trimmed,
            budget,
            "上下文超出预算，裁掉最早的历史"
        );
    }
    let mut tool_images = 0usize;
    let mut seen_tool_images = HashSet::new();
    let mut out = Streamed::failed(LlmError::Timeout);
    out.end = End::Finished;
    let mut rounds = 0u32;
    let mut shared_tools = HashMap::new();
    // 本轮到目前为止的正文（断流重发、超长续写都接在它后面）。
    let mut turn = String::new();
    // 下一次请求要接着写的正文与要求。
    let mut resume: Option<&'static str> = None;
    let mut length_continues = 0u32;
    loop {
        if rounds > 0 {
            let pruned = context::prune_tool_results(&mut messages, budget);
            let removed = context::trim_history(&mut messages, budget, keep);
            keep -= removed;
            if pruned + removed > 0 {
                tracing::info!(
                    job = ctx.job_id,
                    pruned,
                    removed,
                    budget,
                    "工具循环超出上下文预算，已裁剪"
                );
            }
        }
        let offer_tools = rounds < max_rounds;
        let glue = resume.is_some();
        let mut request_messages = messages.clone();
        if let Some(note) = resume {
            request_messages.push(ChatMessage::assistant(turn.clone()));
            request_messages.push(ChatMessage::user(note));
        }
        let request = ChatRequest {
            model: model.provider_model.clone(),
            messages: request_messages,
            tools: if offer_tools {
                specs.clone()
            } else {
                Vec::new()
            },
            params: params.clone(),
        };
        let round = stream_round(
            ctx,
            stop,
            workflow,
            provider.as_ref(),
            &request,
            deadline,
            first_delta.filter(|_| out.first_delta_at.is_none()),
            &out.content,
            glue,
            out.received_chars,
        )
        .await;
        out.started |= round.started;
        out.received_chars += round.received_chars;
        out.first_delta_at = out.first_delta_at.or(round.first_delta_at);
        out.charged = add(out.charged, charge_round(state, &request.messages, &round));
        out.content = combine(&out.content, &round.content, glue);
        if !glue {
            turn.clear();
        }
        turn.push_str(&round.content);
        out.final_text.clone_from(&turn);
        if ctx.trace {
            out.request = Some(request);
        }
        if !matches!(round.end, End::Finished) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match round.end {
                End::Failed(error)
                    if error.is_retryable()
                        && out.retries < MAX_ROUND_RETRIES
                        && remaining >= RETRY_MIN_REMAINING =>
                {
                    out.retries += 1;
                    let base_delay = if error.is_rate_limited() {
                        RATE_LIMITED_RETRY_BASE
                    } else {
                        RETRY_BASE
                    };
                    let delay = base_delay * 2u32.pow(out.retries - 1);
                    tracing::warn!(job = ctx.job_id, model = %model.key, %error, retry = out.retries, "这一轮请求失败，稍后重发");
                    if pause(ctx, stop, delay.min(remaining)).await {
                        out.end = End::Stopped;
                        return out;
                    }
                    resume = (!turn.trim().is_empty()).then_some(RESUME_NOTE);
                    continue;
                }
                end => {
                    out.end = end;
                    return out;
                }
            }
        }
        let truncated = round.finish.as_deref() == Some("length");
        if truncated
            && round.tool_calls.is_empty()
            && length_continues < MAX_LENGTH_CONTINUES
            && !turn.trim().is_empty()
            && out.content.chars().count() < ctx.max_chars
        {
            length_continues += 1;
            tracing::info!(job = ctx.job_id, model = %model.key, "回复超出单次输出上限，接着写");
            resume = Some(LENGTH_NOTE);
            continue;
        }
        resume = None;
        let Some(tool_ctx) = ctx.tools.as_ref().filter(|_| offer_tools) else {
            return out;
        };
        if round.tool_calls.is_empty() {
            return out;
        }
        rounds += 1;
        out.tool_rounds = rounds;
        out.tool_calls += round.tool_calls.len();
        messages.push(ChatMessage::Assistant {
            content: turn.clone(),
            reasoning_content: (!round.reasoning.is_empty()).then(|| round.reasoning.clone()),
            tool_calls: round.tool_calls.clone(),
        });
        // 同一轮的工具并发执行（LLM-005）：开始事件按调用顺序发，结束事件按完成顺序发，
        // 结果按调用顺序回灌；执行期间照常等停止信号，总预算到点判超时。
        let started = Instant::now();
        let mut steps = Vec::with_capacity(round.tool_calls.len());
        let mut pending = FuturesUnordered::new();
        for (index, call) in round.tool_calls.iter().enumerate() {
            let query = (call.name == "search_posts")
                .then(|| search_query(&call.arguments))
                .flatten();
            let label = ctx.toolbox.label(&call.name);
            steps.push((
                workflow.tool_start(&call.name, query.as_deref(), label),
                label,
            ));
            let (output, reused) = if truncated && !complete_args(&call.arguments) {
                // 参数被输出上限截断（GEN-026）：不执行，告诉模型原因。
                let output: BoxFuture<'_, tools::ToolOutput> =
                    futures_util::future::ready(tools::ToolOutput::error(TRUNCATED_ARGS)).boxed();
                (output.shared(), false)
            } else {
                // 共享执行可跨轮保留，因此参数归 future 自己所有。
                let name = call.name.clone();
                let arguments = call.arguments.clone();
                share_forum_call(call, &mut shared_tools, async move {
                    ctx.toolbox
                        .execute(state, tool_ctx, &state.mcp, &name, &arguments, deadline)
                        .await
                })
            };
            pending.push(async move {
                let started = Instant::now();
                (index, output.await, reused, started.elapsed())
            });
        }
        ctx.publish_workflow(workflow);
        let mut outputs: Vec<Option<tools::ToolOutput>> = vec![None; round.tool_calls.len()];
        let mut stop_tick = tokio::time::interval(state.stops.poll_interval());
        loop {
            tokio::select! {
                next = pending.next() => {
                    let Some((index, output, reused, elapsed)) = next else { break };
                    let hits = match &output.meta {
                        tools::ToolMeta::Search { hits, .. } => Some(*hits),
                        _ => None,
                    };
                    tracing::info!(
                        target: "ik_ai::tool_usage",
                        tool = %round.tool_calls[index].name,
                        scene = if ctx.live.is_some() { "dm" } else { "comment" },
                        round = rounds,
                        reused,
                        error = output.is_error,
                        ms = elapsed.as_millis(),
                        result_chars = output.text.chars().count(),
                        search_hits = ?hits,
                        "AI 工具调用完成"
                    );
                    let (step, label) = &steps[index];
                    workflow.tool_finish(
                        step,
                        &round.tool_calls[index].name,
                        &output.meta,
                        started.elapsed().as_millis(),
                        *label,
                        output.is_error,
                    );
                    ctx.publish_workflow(workflow);
                    outputs[index] = Some(output);
                }
                () = stop_signal(stop) => {
                    out.end = End::Stopped;
                    return out;
                }
                _ = stop_tick.tick() => {
                    if ctx.stop_requested().await {
                        out.end = End::Stopped;
                        return out;
                    }
                }
                () = tokio::time::sleep_until(deadline) => {
                    out.end = End::Failed(LlmError::Timeout);
                    return out;
                }
            }
        }
        drop(pending);
        let mut images = Vec::new();
        for (call, output) in round.tool_calls.iter().zip(outputs) {
            let output = output.unwrap_or_else(|| tools::ToolOutput {
                text: String::new(),
                is_error: false,
                meta: tools::ToolMeta::None,
                images: Vec::new(),
            });
            messages.push(ChatMessage::Tool {
                tool_call_id: call.id.clone(),
                content: output.text,
            });
            images.extend(output.images);
        }
        // 工具消息只能是文本：识图模型的图片另起一条用户消息跟在工具结果后面（LLM-015）。
        if model.vision {
            images.retain(|url| seen_tool_images.insert(url.clone()));
            images.truncate(MAX_TOOL_IMAGES.saturating_sub(tool_images));
            if !images.is_empty() {
                tool_images += images.len();
                messages.push(ChatMessage::user_with_images(TOOL_IMAGES_NOTE, images));
            }
        }
        if rounds >= max_rounds {
            workflow.tool_limit(max_rounds);
            ctx.publish_workflow(workflow);
            messages.push(ChatMessage::user(NO_MORE_TOOLS));
        }
    }
}

/// 一次回复的生成结果。
pub(crate) struct Generated {
    /// 最终用的模型（首帧超时后可能是默认模型）。
    pub model: Model,
    pub streamed: Streamed,
}

/// 调用上游生成一条回复（LLM-007）：总预算内跑一遍工具循环；首帧超时时，剩余预算够、且当前不是
/// 默认模型，退回默认模型重试一次且不挂看门狗（GEN-020～022）。被打断的那次调用照样计费。
pub(crate) async fn generate(
    ctx: &StreamCtx<'_>,
    workflow: &mut Workflow,
    model: &Model,
    default_model: Option<&Model>,
    messages: &[ChatMessage],
) -> Generated {
    generate_with_budget(
        ctx,
        workflow,
        model,
        default_model,
        messages,
        Duration::from_secs(ctx.state.config.reply_timeout_secs),
    )
    .await
}

/// 渠道可以收紧总预算；超时仍通过同一生成出口返回已发生的用量，不能在外层丢弃 future。
pub(crate) async fn generate_with_budget(
    ctx: &StreamCtx<'_>,
    workflow: &mut Workflow,
    model: &Model,
    default_model: Option<&Model>,
    messages: &[ChatMessage],
    budget: Duration,
) -> Generated {
    let state = ctx.state;
    let started_at = Instant::now();
    let deadline = started_at + budget;
    let first_delta =
        Some(Duration::from_secs(state.config.first_delta_timeout_secs)).filter(|d| !d.is_zero());
    // 只有私信能被用户停止（GEN-011）。
    let watch = ctx.live.is_some().then(|| state.stops.watch(ctx.job_id));
    let stop = watch.as_ref();
    let mut streamed =
        run_rounds(ctx, stop, workflow, model, messages, deadline, first_delta).await;
    let mut model = model.clone();
    if matches!(streamed.end, End::FirstDeltaTimeout)
        && let Some(fallback) = default_model.filter(|d| d.key != model.key)
        && deadline.saturating_duration_since(Instant::now()) >= FALLBACK_MIN_REMAINING
        && ctx.providers.contains(&fallback.provider)
    {
        tracing::warn!(job = ctx.job_id, from = %model.key, to = %fallback.key, "首帧超时，退回默认模型");
        let earlier = streamed.charged;
        let earlier_retries = streamed.retries;
        model = fallback.clone();
        streamed = run_rounds(ctx, stop, workflow, &model, messages, deadline, None).await;
        streamed.charged = add(streamed.charged, earlier);
        streamed.retries += earlier_retries;
    }
    streamed.first_delta = streamed
        .first_delta_at
        .map(|at| at.saturating_duration_since(started_at));
    streamed.elapsed = started_at.elapsed();
    Generated { model, streamed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_join_with_a_blank_line() {
        assert_eq!(join_rounds("", "答"), "答");
        assert_eq!(join_rounds("查一下。", ""), "查一下。");
        assert_eq!(join_rounds("查一下。", "答"), "查一下。\n\n答");
    }

    #[test]
    fn search_query_reads_query_or_queries() {
        assert_eq!(
            search_query(r#"{"query":" 攻略 "}"#).as_deref(),
            Some("攻略")
        );
        assert_eq!(
            search_query(r#"{"queries":["a","b"]}"#).as_deref(),
            Some("a、b")
        );
        assert_eq!(search_query("{}"), None);
    }

    #[tokio::test]
    async fn forum_calls_share_execution_but_memory_writes_do_not() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let count = Arc::new(AtomicUsize::new(0));
        let execute = |is_error| {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                tools::ToolOutput {
                    text: "结果".into(),
                    is_error,
                    meta: tools::ToolMeta::None,
                    images: Vec::new(),
                }
            }
        };
        let mut cache = HashMap::new();
        let mut call = ToolCall {
            id: "a".into(),
            name: "get_post_content".into(),
            arguments: r#"{"document_id":"7","offset":0}"#.into(),
        };
        let (first, reused) = share_forum_call(&call, &mut cache, execute(false));
        assert!(!reused);
        call.arguments = r#"{ "offset": 0, "document_id": "7" }"#.into();
        let (second, reused) = share_forum_call(&call, &mut cache, execute(false));
        assert!(reused);
        let (a, b) = futures_util::join!(first, second);
        assert_eq!(a, b);
        assert_eq!(count.load(Ordering::SeqCst), 1);

        let (next_round, reused) = share_forum_call(&call, &mut cache, execute(false));
        assert!(reused);
        next_round.await;
        assert_eq!(count.load(Ordering::SeqCst), 1);

        // 续读参数不同，必须重新执行。
        call.arguments = r#"{"document_id":"7","offset":4000}"#.into();
        let (next_page, reused) = share_forum_call(&call, &mut cache, execute(false));
        assert!(!reused);
        next_page.await;
        assert_eq!(count.load(Ordering::SeqCst), 2);

        call.name = "remember".into();
        let (first, reused) = share_forum_call(&call, &mut cache, execute(false));
        assert!(!reused);
        let (second, reused) = share_forum_call(&call, &mut cache, execute(false));
        assert!(!reused);
        futures_util::join!(first, second);
        assert_eq!(count.load(Ordering::SeqCst), 4);

        // 已完成的失败不复用，下一轮允许重试；重试成功后再复用。
        call.name = "search_posts".into();
        let (failed, _) = share_forum_call(&call, &mut cache, execute(true));
        assert!(failed.await.is_error);
        let (retry, reused) = share_forum_call(&call, &mut cache, execute(false));
        assert!(!reused);
        assert!(!retry.await.is_error);
        let (success, reused) = share_forum_call(&call, &mut cache, execute(false));
        assert!(reused);
        assert!(!success.await.is_error);
        assert_eq!(count.load(Ordering::SeqCst), 6);
    }

    #[test]
    fn append_sends_suffix_and_snapshots() {
        let mut a = Appended::default();
        // 第一条总是全文
        assert_eq!(
            next_append(&mut a, "你好"),
            Some(serde_json::json!({ "seq": 1, "content": "你好" }))
        );
        assert_eq!(next_append(&mut a, "你好"), None);
        assert_eq!(
            next_append(&mut a, "你好呀，"),
            Some(serde_json::json!({ "seq": 2, "text": "呀，" }))
        );
        // 清洗改了前文：发全文
        assert_eq!(
            next_append(&mut a, "你好呀"),
            Some(serde_json::json!({ "seq": 3, "content": "你好呀" }))
        );
        for n in 4..=APPEND_SNAPSHOT_EVERY {
            let next = format!("{}x", a.text);
            assert_eq!(next_append(&mut a, &next).unwrap()["seq"], n);
        }
        let next = format!("{}y", a.text);
        let snap = next_append(&mut a, &next).unwrap();
        assert_eq!(snap["seq"], APPEND_SNAPSHOT_EVERY + 1);
        assert_eq!(snap["content"], next.as_str());
    }
}
