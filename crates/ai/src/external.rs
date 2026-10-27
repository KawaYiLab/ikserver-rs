//! 渠道无关的一问一答入口（ADR 0008）。准备快照 → 调用方原子预占 → 事务外生成 → 调用方定稿结算。
//!
//! 本模块不拥有渠道的消息、任务或额度事务，也不为游客读取或写入记忆。调用方必须在成功预占后
//! 才调用 [`reply`]，且无论生成结果如何都结算 [`ExternalReply::charge`]，保留已发生的模型成本。

use std::sync::Arc;
use std::time::Duration;

use ik_kernel::Problem;

use crate::AiState;
use crate::catalog::{Model, OutputMode, RoleCard};
use crate::generate::{self, End, StreamCtx};
use crate::llm::{ChatMessage, Providers, Usage};
use crate::memory::{self, MemoryScope};
use crate::quota::{self, Limits};
use crate::toolbox::Toolbox;
use crate::tools::ToolContext;
use crate::{dm, immersion, sanitize, workflow};
use chrono::Utc;

/// 已由渠道按时间排序并去掉真实身份的历史；只允许用户和助手角色。
#[derive(Debug, Clone)]
pub struct ExternalLine {
    pub assistant: bool,
    pub content: String,
}

/// 绑定了站内账号的提问者的个人记忆（ADR 0008）：读写的是与站内私信同一份记忆。
#[derive(Debug, Clone)]
pub struct ExternalMemory {
    /// 站内账号（可信：来自渠道的绑定关系，不来自消息）。
    pub user_id: i64,
    /// 公开场合的键（QQ 群：`qq:group:<id>`）；私下场合（QQ 单聊）为空。
    pub public: Option<String>,
    /// 这次是不是开场（隔了很久或刚开始聊），可以问起到期的待跟进。只在私下场合有用。
    pub opening: bool,
}

#[derive(Debug, Clone)]
pub struct ExternalTurn {
    pub channel: &'static str,
    pub card_slug: String,
    pub model_key: Option<String>,
    /// 群聊必须为 None，即使提问者已绑定站内账号。
    pub viewer: Option<i64>,
    pub history: Vec<ExternalLine>,
    pub max_chars: usize,
    pub output_mode: OutputMode,
    pub timeout_secs: u64,
    /// 个人记忆；游客为空（不读不写）。
    pub memory: Option<ExternalMemory>,
    /// 这个场合的共享记忆（QQ 群共享记忆）：放进 system prompt；有 `memory` 时还能用 remember 写它。
    pub shared_scope: Option<String>,
}

/// 固定本次角色、工具和模型快照，保证预估与生成使用同一配置。
pub struct PreparedTurn {
    pub ai_user_id: i64,
    pub model_key: String,
    pub limits: Limits,
    channel: &'static str,
    model: Model,
    default_model: Option<Model>,
    providers: Providers,
    tools: ToolContext,
    toolbox: Toolbox,
    messages: Vec<ChatMessage>,
    prompt_text: String,
    max_chars: usize,
    output_mode: OutputMode,
    budget: Duration,
    estimate: i64,
    /// 这次提起的待跟进，回复完成后标记已问起。
    follow_ups: Vec<i64>,
    memory_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Failed(String),
}

#[derive(Debug)]
pub struct ExternalReply {
    pub segments: Vec<String>,
    pub usage: Usage,
    pub model_key: String,
    pub weight: f64,
    pub started: bool,
    pub outcome: RunOutcome,
}

impl ExternalReply {
    /// QUOTA-019：开始调用却没有用量时保留整份预占；未开始调用则退回。
    pub fn charge(&self, reserved: i64) -> i64 {
        if self.started && self.usage.is_zero() {
            reserved.max(0)
        } else {
            quota::weighted(self.usage.total(), self.weight)
        }
    }
}

fn messages(card: &RoleCard, turn: &ExternalTurn, toolbox: &Toolbox) -> Vec<ChatMessage> {
    let mut system = dm::system_prompt(card, turn.output_mode);
    if turn.channel == "qq" {
        let target = turn.max_chars.saturating_mul(9) / 10;
        system.push_str(&format!(
            "\n\n【QQ回复长度】最终回复最多 {} 字（含格式标记、链接和换行）。请尽量控制在约 {} 字以内，优先完整收尾；超长时在自然段或句号处结束，不要以未完成的标题、列表项或半句结尾。",
            turn.max_chars, target
        ));
    }
    let mut messages = vec![ChatMessage::system(system)];
    toolbox.annotate_system(&mut messages);
    messages.extend(turn.history.iter().map(|line| {
        if line.assistant {
            ChatMessage::assistant(&line.content)
        } else {
            ChatMessage::user(&line.content)
        }
    }));
    messages
}

/// 在调用方的预占事务之前准备；这里只读配置与账号，不调用模型或写入记忆。
pub async fn prepare(state: &AiState, turn: ExternalTurn) -> Result<PreparedTurn, Problem> {
    prepare_inner(state, turn, None).await
}

pub async fn prepare_authorized(
    state: &AiState,
    turn: ExternalTurn,
    authorization: Arc<dyn crate::authorization::Authorization>,
) -> Result<PreparedTurn, Problem> {
    prepare_inner(state, turn, Some(authorization)).await
}

async fn prepare_inner(
    state: &AiState,
    turn: ExternalTurn,
    authorization: crate::authorization::Guard,
) -> Result<PreparedTurn, Problem> {
    if authorization.is_some() {
        let mut conn = state.db.acquire().await?;
        crate::authorization::require(&mut conn, &authorization).await?;
    }
    if turn.max_chars == 0 || turn.max_chars > 4000 || turn.timeout_secs == 0 {
        return Err(Problem::field(
            "turn",
            "invalid",
            "回复长度或超时预算不合法",
        ));
    }
    if turn.history.is_empty() || !turn.history.iter().any(|line| !line.assistant) {
        return Err(Problem::field("history", "required", "缺少用户消息"));
    }
    let catalog = state.catalog.get().await?;
    let card = catalog
        .card_by_slug(&turn.card_slug)
        .filter(|card| card.enabled && (turn.channel != "qq" || card.qq_enabled))
        .ok_or_else(Problem::not_found)?;
    let mut conn = state.db.acquire().await?;
    if ik_auth::interop::active_users(&mut conn, &[card.bound_user_id])
        .await?
        .is_empty()
    {
        return Err(Problem::not_found());
    }
    drop(conn);
    // 外部渠道只使用角色白名单内且有提供方的模型，首帧降级也不能绕过白名单。
    let available =
        |model: &&Model| card.allows(model) && catalog.providers.contains(&model.provider);
    let model = turn
        .model_key
        .as_deref()
        .and_then(|key| catalog.model(key))
        .filter(available)
        .or_else(|| {
            card.default_model_id
                .and_then(|id| catalog.model_by_id(id))
                .filter(available)
        })
        .or_else(|| catalog.allowed_models(card).find(available))
        .ok_or_else(|| Problem::field("model", "unavailable", "角色没有可用的模型"))?
        .clone();
    let default_model = catalog.default_model().filter(available).cloned();
    // 记忆（ADR 0007、0008）：全站开关在管理后台。游客没有个人记忆，也不能写共享记忆。
    let memory_on = catalog.memory.enabled;
    let mut toolbox = Toolbox::for_card(state, &catalog, card, &state.mcp);
    if let Some(guard) = authorization.clone() {
        toolbox = toolbox.with_authorization(guard);
    }
    if memory_on && let Some(memory) = &turn.memory {
        toolbox = toolbox.with_memory(
            memory.user_id,
            card.bound_user_id,
            MemoryScope {
                conversation_id: None,
                public: memory.public.clone(),
                shared: turn.shared_scope.clone(),
            },
        );
    }
    let mut messages = messages(card, &turn, &toolbox);
    let mut follow_ups = Vec::new();
    if memory_on {
        let mut conn = state.db.acquire().await?;
        if let Some(memory) = &turn.memory {
            let (note, due) = memory::external_note(
                &mut conn,
                memory.user_id,
                card.bound_user_id,
                memory.public.as_deref(),
                memory.opening,
                Utc::now(),
            )
            .await?;
            if let Some(note) = note {
                memory::append_system(&mut messages, &note);
            }
            follow_ups = due;
        }
        if let Some(scope) = turn.shared_scope.as_deref()
            && let Some(note) =
                crate::shared_memory::note(&mut conn, scope, card.bound_user_id).await?
        {
            memory::append_system(&mut messages, &note);
        }
        drop(conn);
        // 相关往事（MEM-052）：用当前提问检索；公开场合只找在这里说过的。
        if let Some(memory) = &turn.memory
            && let Some(query) = turn.history.iter().rev().find(|line| !line.assistant)
            && let Some(note) = memory::recall_events(
                state,
                memory.user_id,
                card.bound_user_id,
                &query.content,
                memory.public.as_deref(),
            )
            .await
        {
            memory::append_system(&mut messages, &note);
        }
    }
    let tools = match turn.viewer {
        Some(viewer) => ToolContext::new(state, viewer).await?,
        None => ToolContext::guest(),
    };
    let prompt = quota::prompt_estimate(&messages, false)
        .saturating_add(toolbox.extra_estimate_tokens(state.config.max_tool_rounds))
        .saturating_add(
            if model.vision || default_model.as_ref().is_some_and(|model| model.vision) {
                quota::TOOL_IMAGE_TOKENS
            } else {
                0
            },
        );
    let estimate = quota::estimate(
        state.config.estimated_context_tokens,
        state.config.max_tool_rounds,
        prompt,
        // 预估覆盖首帧降级后的更贵模型。
        default_model.as_ref().map_or(model.weight(), |fallback| {
            model.weight().max(fallback.weight())
        }),
    );
    Ok(PreparedTurn {
        ai_user_id: card.bound_user_id,
        model_key: model.key.clone(),
        limits: catalog.limits,
        channel: turn.channel,
        model,
        default_model,
        providers: catalog.providers.clone(),
        tools,
        toolbox,
        messages,
        prompt_text: turn
            .history
            .iter()
            .filter(|line| !line.assistant)
            .map(|line| line.content.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        max_chars: turn.max_chars,
        output_mode: turn.output_mode,
        budget: Duration::from_secs(turn.timeout_secs.min(state.config.reply_timeout_secs)),
        estimate,
        follow_ups,
        memory_enabled: memory_on,
    })
}

pub fn estimate(turn: &PreparedTurn) -> i64 {
    turn.estimate
}

impl PreparedTurn {
    pub fn follow_up_ids(&self) -> &[i64] {
        &self.follow_ups
    }
    pub fn memory_enabled(&self) -> bool {
        self.memory_enabled
    }
}

pub async fn role_enabled_in(conn: &mut sqlx::PgConnection, slug: &str) -> Result<bool, Problem> {
    Ok(sqlx::query_scalar(
        "SELECT enabled AND qq_enabled FROM ai_role_cards WHERE slug=$1 FOR SHARE",
    )
    .bind(slug)
    .fetch_optional(conn)
    .await?
    .unwrap_or(false))
}

/// 仅在渠道确认至少一段已送达后，与渠道送达记录同事务标记待跟进。
pub async fn mark_delivered(
    conn: &mut sqlx::PgConnection,
    ids: &[i64],
    now: chrono::DateTime<Utc>,
) -> Result<(), Problem> {
    memory::mark_followed_up(conn, ids, now).await
}

fn clean_segments(text: &str, mode: OutputMode, max_chars: usize, channel: &str) -> Vec<String> {
    // QQ owns structural rendering and the final wire budget, including URLs and markers.
    // Preserve complete Markdown even for plain QQ replies so links can become title + URL.
    if channel == "qq" {
        let cleaned = sanitize::sanitize(text, OutputMode::Markdown, usize::MAX);
        return if cleaned.is_empty() {
            vec![]
        } else {
            vec![cleaned]
        };
    }
    let mut remaining = max_chars;
    let mut out = Vec::new();
    // 先分段再清洗，保留 Markdown 被移除前的气泡边界（同私信工作流）。
    for segment in workflow::split_segments(text) {
        if remaining == 0 {
            break;
        }
        let text = sanitize::sanitize(&segment, mode, remaining);
        remaining = remaining.saturating_sub(text.chars().count());
        if !text.is_empty() {
            out.push(text);
        }
    }
    out
}

/// 不推流的生成，所有外部 IO 在事务外完成。失败也返回用量，调用方必须结算。
pub async fn reply(state: &AiState, turn: &PreparedTurn, job_id: i64) -> ExternalReply {
    let ctx = StreamCtx {
        state,
        job_id,
        live: None,
        providers: turn.providers.clone(),
        mode: turn.output_mode,
        max_chars: turn.max_chars,
        tools: Some(turn.tools.clone()),
        toolbox: turn.toolbox.clone(),
    };
    let generated = generate::generate_with_budget(
        &ctx,
        &mut workflow::Workflow::disabled(),
        &turn.model,
        turn.default_model.as_ref(),
        &turn.messages,
        turn.budget,
    )
    .await;
    let streamed = generated.streamed;
    let mut segments = Vec::new();
    let outcome = match streamed.end {
        End::Finished => {
            segments = clean_segments(
                &streamed.final_text,
                turn.output_mode,
                turn.max_chars,
                turn.channel,
            );
            match immersion::check(&segments.join("\n\n"), &turn.prompt_text) {
                immersion::Verdict::Keep => {}
                immersion::Verdict::Replace { text, reason } => {
                    tracing::warn!(
                        channel = turn.channel,
                        job = job_id,
                        reason,
                        "外部渠道回复触发沉浸守卫"
                    );
                    segments = clean_segments(text, turn.output_mode, turn.max_chars, turn.channel);
                }
            }
            if segments.is_empty() {
                RunOutcome::Failed("empty_reply".to_owned())
            } else {
                RunOutcome::Completed
            }
        }
        End::Failed(error) => RunOutcome::Failed(error.to_string()),
        End::FirstDeltaTimeout => RunOutcome::Failed("first_delta_timeout".to_owned()),
        End::TooLarge => RunOutcome::Failed("upstream_output_too_large".to_owned()),
        End::Stopped => RunOutcome::Failed("stopped".to_owned()),
    };
    ExternalReply {
        segments,
        usage: streamed.charged,
        model_key: generated.model.key.clone(),
        weight: generated.model.weight(),
        started: streamed.started,
        outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::clean_segments;
    use crate::catalog::OutputMode;

    #[test]
    fn segments_keep_report_boundary_and_share_character_budget() {
        let segments = clean_segments(
            "正在整理。\n\n## 结果\n去六分街散步吧。",
            OutputMode::Plain,
            600,
            "other",
        );
        assert_eq!(segments, ["正在整理。", "结果\n去六分街散步吧。"]);
        let segments = clean_segments(
            "正在整理。\n\n## 结果\n去六分街散步吧。",
            OutputMode::Plain,
            8,
            "other",
        );
        assert!(segments.iter().map(|s| s.chars().count()).sum::<usize>() <= 8);
    }
}
