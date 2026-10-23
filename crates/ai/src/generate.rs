//! 一次回复的生成（私信与评论共用，LLM 第 10 节）：流式调用上游、工具调用循环、首帧看门狗与总预算、
//! 退回默认模型、计费用量。全部在事务之外进行（LLM-009）。
//!
//! - 工具循环（LLM-005）：模型返回 tool_calls 时在进程内执行（[`crate::tools`]），结果回灌后继续请求；
//!   最多 `ai.max_tool_rounds` 轮，用完后不再提供工具并要求模型基于已有结果作答；
//! - 用量（LLM-006）：每一轮请求单独计费再累加；没报用量的轮次按估算（D-083 方案 C）；
//! - 私信有推送目标（[`Live`]）：推累计正文（GEN-004）、「正在输入」（GEN-005）、工作流事件（WF-003），
//!   并轮询停止信号（GEN-011）；评论没有。

use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use ik_kernel::bus::{Audience, Event};
use tokio::time::Instant;

use crate::AiState;
use crate::catalog::{Model, OutputMode};
use crate::llm::{
    ChatMessage, ChatProvider, ChatRequest, LlmError, Providers, StreamEvent, ToolCall,
    ToolCallAssembler, Usage,
};
use crate::quota;
use crate::repo;
use crate::sanitize;
use crate::toolbox::Toolbox;
use crate::tools::{self, ToolContext};
use crate::workflow::Workflow;

/// 停止信号的轮询间隔（GEN-011）。
const STOP_POLL: Duration = Duration::from_millis(600);
/// 生成期间「正在输入」的间隔（GEN-005）。
const TYPING_EVERY: Duration = Duration::from_secs(2);
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
            end: End::Failed(error),
        }
    }
}

/// 私信回复的实时推送目标。评论回复没有（GEN-030：一次性发出）。
pub(crate) struct Live {
    pub conversation_id: i64,
    pub reply_id: i64,
    pub ai_user_id: i64,
    /// 推送对象（会话里的人类成员）。
    pub audience: Vec<i64>,
}

pub(crate) struct StreamCtx<'a> {
    pub state: &'a AiState,
    pub job_id: i64,
    /// 有推送目标时推送累计正文、「正在输入」与工作流事件，并轮询停止信号。
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

    async fn stop_requested(&self) -> bool {
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

/// 一轮请求的产出。
struct Round {
    content: String,
    /// 这一轮的思考原文：只在带工具调用时回传给上游（LLM-013），不下发、不落库。
    reasoning: String,
    received_chars: usize,
    usage: Option<Usage>,
    started: bool,
    tool_calls: Vec<ToolCall>,
    end: End,
}

/// 一轮流式请求（LLM-004）：节流推送累计正文（GEN-004），每 600ms 查停止信号（GEN-011），
/// 每 2 秒推「正在输入」（GEN-005），首帧看门狗（GEN-021）与总预算（GEN-020）。
/// `prefix` 是之前各轮的正文（推送的是累计全文），`already` 是之前各轮收到的字符数。
#[allow(clippy::too_many_arguments)]
async fn stream_round(
    ctx: &StreamCtx<'_>,
    workflow: &mut Workflow,
    provider: &dyn ChatProvider,
    request: &ChatRequest,
    deadline: Instant,
    first_delta: Option<Duration>,
    prefix: &str,
    already: usize,
) -> Round {
    let mut out = Round {
        content: String::new(),
        reasoning: String::new(),
        received_chars: 0,
        usage: None,
        started: false,
        tool_calls: Vec::new(),
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
                        let total = out.content.chars().count();
                        if ctx.live.is_some() && total >= pushed_chars + min_chars {
                            pushed_chars = total;
                            let full = join_rounds(prefix, &out.content);
                            let shown = sanitize::sanitize(&full, ctx.mode, ctx.max_chars);
                            ctx.publish("dm.message.delta", &serde_json::json!({
                                "conversationId": conversation_id.to_string(),
                                "messageId": reply_id.to_string(),
                                "content": shown,
                            }));
                        }
                    }
                    StreamEvent::ToolCallDelta { index, id, name, arguments } => {
                        got_content = true;
                        out.received_chars += arguments.as_deref().map_or(0, |a| a.chars().count());
                        assembler.push(index, id, name, arguments);
                    }
                    StreamEvent::Usage(usage) => out.usage = Some(usage),
                    StreamEvent::Finish(_) => {}
                }
                ctx.publish_workflow(workflow);
                if already + out.received_chars > MAX_STREAM_CHARS {
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

/// 用一个模型跑完整个工具循环。
async fn run_rounds(
    ctx: &StreamCtx<'_>,
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
    // 不识图的模型（包括首帧超时后退回的默认模型）收不到图片，只看正文里的「[图片]」占位（LLM-015）。
    let mut messages: Vec<ChatMessage> = if model.vision {
        base.to_vec()
    } else {
        base.iter().map(ChatMessage::without_images).collect()
    };
    let mut tool_images = 0usize;
    let mut out = Streamed::failed(LlmError::Timeout);
    out.end = End::Finished;
    let mut rounds = 0u32;
    loop {
        let offer_tools = rounds < max_rounds;
        let request = ChatRequest {
            model: model.provider_model.clone(),
            messages: messages.clone(),
            tools: if offer_tools {
                ctx.toolbox.specs()
            } else {
                Vec::new()
            },
        };
        let round = stream_round(
            ctx,
            workflow,
            provider.as_ref(),
            &request,
            deadline,
            if rounds == 0 { first_delta } else { None },
            &out.content,
            out.received_chars,
        )
        .await;
        out.started |= round.started;
        out.received_chars += round.received_chars;
        out.charged = add(out.charged, charge_round(state, &request.messages, &round));
        out.content = join_rounds(&out.content, &round.content);
        out.final_text.clone_from(&round.content);
        if !matches!(round.end, End::Finished) {
            out.end = round.end;
            return out;
        }
        let Some(tool_ctx) = ctx.tools.as_ref().filter(|_| offer_tools) else {
            return out;
        };
        if round.tool_calls.is_empty() {
            return out;
        }
        rounds += 1;
        out.tool_calls += round.tool_calls.len();
        messages.push(ChatMessage::Assistant {
            content: round.content.clone(),
            reasoning_content: (!round.reasoning.is_empty()).then(|| round.reasoning.clone()),
            tool_calls: round.tool_calls.clone(),
        });
        // 同一轮的工具并发执行（LLM-005）：开始事件按调用顺序发，结束事件按完成顺序发，
        // 结果按调用顺序回灌；执行期间照常轮询停止信号，总预算到点判超时。
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
            pending.push(async move {
                let output = ctx
                    .toolbox
                    .execute(
                        state,
                        tool_ctx,
                        &state.mcp,
                        &call.name,
                        &call.arguments,
                        deadline,
                    )
                    .await;
                (index, output)
            });
        }
        ctx.publish_workflow(workflow);
        let mut outputs: Vec<Option<tools::ToolOutput>> = vec![None; round.tool_calls.len()];
        let mut stop_tick = tokio::time::interval(STOP_POLL);
        loop {
            tokio::select! {
                next = pending.next() => {
                    let Some((index, output)) = next else { break };
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
    let state = ctx.state;
    let deadline = Instant::now() + Duration::from_secs(state.config.reply_timeout_secs);
    let first_delta =
        Some(Duration::from_secs(state.config.first_delta_timeout_secs)).filter(|d| !d.is_zero());
    let mut streamed = run_rounds(ctx, workflow, model, messages, deadline, first_delta).await;
    let mut model = model.clone();
    if matches!(streamed.end, End::FirstDeltaTimeout)
        && let Some(fallback) = default_model.filter(|d| d.key != model.key)
        && deadline.saturating_duration_since(Instant::now()) >= FALLBACK_MIN_REMAINING
        && ctx.providers.contains(&fallback.provider)
    {
        tracing::warn!(job = ctx.job_id, from = %model.key, to = %fallback.key, "首帧超时，退回默认模型");
        let earlier = streamed.charged;
        model = fallback.clone();
        streamed = run_rounds(ctx, workflow, &model, messages, deadline, None).await;
        streamed.charged = add(streamed.charged, earlier);
    }
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
}
