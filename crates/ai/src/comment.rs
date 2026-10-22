//! AI 评论回复（ai-assistant.md 第 3.2、4.1、6.4、6.7 节）。
//!
//! 发表评论时经 [`ik_comments::CommentHooks`] 在同一事务里判定目标并建任务（TRIG-010～014）；
//! 后台任务 `ai.comment_reply` 执行，三段各自独立：
//! 1. 准备（短事务）：复核触发条件（TRIG-016），决定模型（CTX-031：不看会话选择），组装上下文
//!    （帖子、父评论、同一评论者在本帖与这个 AI 之前的问答，CTX-001～003），预占额度；
//! 2. 生成（事务之外）：调用上游，不推增量（GEN-030、D-089 方案 A），首帧看门狗与总预算同私信；
//! 3. 定稿（短事务）：清洗（plain，上限 1500，GEN-031）、沉浸守卫（GEN-060～063）、以 AI 的名义
//!    回复（D-004 方案 A：挂到触发评论所在的一级评论下，开头提及触发者），结算额度，记审计。
//!
//! 失败交给后台任务重试；首帧超时是终态，不重试（TRIG-034）。额度不足、全局关闭时静默跳过
//! （评论区没有私信那样的系统提示位）。

use chrono::Utc;
use ik_comments::{CommentHooks, CreatedComment};
use ik_kernel::bus::Effects;
use ik_kernel::jobs::{IntoTaskHandlerResult, JobSpec, Jobs, TaskHandler, WorkerContext};
use ik_kernel::{Problem, Snowflake};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::AiState;
use crate::catalog::{Catalog, CatalogCache, Model, OutputMode, RoleCard};
use crate::dm::{self, RunOutcome};
use crate::generate::{self, End, StreamCtx};
use crate::immersion;
use crate::llm::{ChatMessage, Providers};
use crate::quota::{self, Hold};
use crate::repo::{self, JobOutcome, JobRow, NewJob};
use crate::sanitize;
use crate::toolbox::Toolbox;

/// 帖子正文进上下文的上限（CTX-001）。
const POST_BODY_MAX_CHARS: usize = 4000;
/// 父评论进上下文的上限（CTX-001）。
const PARENT_MAX_CHARS: usize = 500;

/// 后台任务：生成一条 AI 评论回复。重试次数取配置 `ai.max_attempts`（TRIG-033）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentReply {
    pub job_id: i64,
}

/// 在调用方的事务里入队（TRIG-010：与触发评论同一事务）。
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
    jobs.enqueue(conn, CommentReply { job_id }, spec).await
}

impl TaskHandler for CommentReply {
    const IDENTIFIER: &'static str = "ai.comment_reply";

    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(state) = ctx.get_ext::<AiState>() else {
            return Err("执行器没有注册 AiState".to_owned());
        };
        let attempt = *ctx.job().attempts();
        let last = attempt >= *ctx.job().max_attempts();
        match run_job(state, self.job_id, attempt, last).await {
            Ok(RunOutcome::Retry(reason)) => Err(reason),
            Ok(outcome) => {
                tracing::debug!(job = self.job_id, ?outcome, "AI 评论任务结束");
                Ok(())
            }
            Err(problem) => Err(format!("{problem:?}")),
        }
    }
}

/// 回复目标（TRIG-013、TRIG-014）：正文前 10 个提及里第一个有启用角色卡的 → 父评论作者有角色卡 →
/// 帖子作者有角色卡；选中的角色还须开放评论回复。返回 AI 账号 id。
pub(crate) fn target(
    catalog: &Catalog,
    mentioned: &[i64],
    parent_author: Option<i64>,
    post_author: i64,
) -> Option<i64> {
    let has_card = |id: &i64| catalog.card_for_user(*id).is_some();
    let chosen = mentioned
        .iter()
        .copied()
        .find(has_card)
        .or_else(|| parent_author.filter(has_card))
        .or_else(|| Some(post_author).filter(has_card))?;
    catalog
        .card_for_user(chosen)
        .filter(|c| c.comment_enabled)
        .map(|c| c.bound_user_id)
}

/// 评论的钩子：判定是否触发 AI，触发时建任务并入队。
pub(crate) struct Hooks {
    catalog: CatalogCache,
    jobs: Jobs,
    ids: std::sync::Arc<Snowflake>,
    max_attempts: i16,
}

impl Hooks {
    pub(crate) fn new(state: &AiState) -> Self {
        Self {
            catalog: state.catalog.clone(),
            jobs: state.jobs.clone(),
            ids: state.ids.clone(),
            max_attempts: state.config.max_attempts,
        }
    }
}

#[async_trait::async_trait]
impl CommentHooks for Hooks {
    async fn created(
        &self,
        conn: &mut PgConnection,
        comment: &CreatedComment,
        _effects: &mut Effects,
    ) -> Result<(), Problem> {
        // TRIG-012：评论、帖子、父评论任一匿名，或作者本身是 AI（防 AI 互相接龙），都不触发。
        if comment.is_anonymous
            || comment.post_is_anonymous
            || comment.parent.as_ref().is_some_and(|p| p.is_anonymous)
            || comment.author_is_ai
        {
            return Ok(());
        }
        let catalog = match self.catalog.get().await {
            Ok(catalog) => catalog,
            Err(problem) => {
                // 触发判定失败不影响评论写入（TRIG-002）。
                tracing::warn!(?problem, "读取 AI 配置失败，本条评论不触发 AI");
                return Ok(());
            }
        };
        if !catalog.limits.global_enabled {
            return Ok(());
        }
        let mentioned = ik_comments::text::mentioned_user_ids(&comment.body);
        let Some(ai_user_id) = target(
            &catalog,
            &mentioned,
            comment.parent.as_ref().map(|p| p.author_id),
            comment.post_author_id,
        ) else {
            return Ok(());
        };
        if ai_user_id == comment.author_id {
            return Ok(());
        }
        let created = repo::insert_job(
            conn,
            &NewJob {
                id: self.ids.next_raw(),
                kind: "comment",
                execution_key: &format!("comment:{}", comment.id),
                quota_user_id: comment.author_id,
                ai_user_id,
                conversation_id: None,
                trigger_message_id: None,
                post_id: Some(comment.post_id),
                trigger_comment_id: Some(comment.id),
                generation_epoch: 0,
                model_override: None,
                now: comment.created_at,
            },
        )
        .await?;
        if let Some(job_id) = created {
            enqueue(&self.jobs, conn, job_id, self.max_attempts).await?;
        }
        Ok(())
    }
}

/// 评论正文进上下文的纯文本：提及与表情 token 转成可读文本，纯图片评论为「[图片]」。
fn comment_text(body: &str) -> String {
    let text = ik_comments::text::plain_text(body);
    let text = text.trim();
    if text.is_empty() {
        "[图片]".to_owned()
    } else {
        text.to_owned()
    }
}

/// AI 回复去掉开头系统加的提及（TRIG-017），作为记忆里 AI 自己的话。
fn reply_text(body: &str) -> String {
    let body = body.trim_start();
    let rest = if body.starts_with("@[") {
        body.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        body
    };
    ik_comments::text::plain_text(rest).trim().to_owned()
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 帖子上下文。
pub(crate) struct PostContext {
    pub id: i64,
    pub title: String,
    pub body: String,
}

/// 一轮记忆：用户的评论与 AI 当时的回复（纯文本）。
pub(crate) struct Turn {
    pub user: String,
    pub assistant: String,
}

/// 发给模型的消息（LLM-002、CTX-002）：system（人格 + 评论回复要求 + 当前帖子）→ 之前的问答 →
/// 本次评论（`当前帖子：…`、可选的 `父评论：…`、`<评论者> 说：<评论>`）。
pub(crate) fn build_messages(
    card: &RoleCard,
    post: &PostContext,
    parent: Option<&str>,
    commenter: &str,
    comment: &str,
    memory: &[Turn],
) -> Vec<ChatMessage> {
    let mut system = dm::system_prompt(card, OutputMode::Plain);
    system.push_str(
        "\n\n【评论回复】你正在帖子的评论区回复用户。不要在开头写 @ 或提及对方（系统会自动提及），\
         一般写 2～5 句。",
    );
    let title = if post.title.trim().is_empty() {
        "（无标题）"
    } else {
        post.title.trim()
    };
    system.push_str(&format!("\n\n【当前帖子】{title}"));
    if !post.body.trim().is_empty() {
        system.push_str(&format!("\n{}", post.body.trim()));
    }
    let mut messages = Vec::with_capacity(memory.len() * 2 + 2);
    messages.push(ChatMessage::system(system));
    for turn in memory {
        messages.push(ChatMessage::user(format!("{commenter} 说：{}", turn.user)));
        messages.push(ChatMessage::assistant(turn.assistant.clone()));
    }
    let mut last = format!("当前帖子：{title}（document_id: {}）", post.id);
    if let Some(parent) = parent {
        last.push_str(&format!("\n父评论：{parent}"));
    }
    last.push_str(&format!("\n{commenter} 说：{comment}"));
    messages.push(ChatMessage::user(last));
    messages
}

/// 准备阶段的结果。
struct Prepared {
    job: JobRow,
    trigger_id: i64,
    trigger_author: (i64, String),
    model: Model,
    default_model: Option<Model>,
    /// 这次快照里可用的上游。
    providers: Providers,
    hold: Hold,
    messages: Vec<ChatMessage>,
    /// 本次可用的工具（按角色卡组装，ADR 0004）。
    toolbox: Toolbox,
    /// 帖子标题 + 正文（沉浸守卫判定防诈骗帖用）。
    post_text: String,
}

enum Prepare {
    Ready(Box<Prepared>),
    Done(RunOutcome),
}

async fn skip(
    tx: &mut PgConnection,
    job_id: i64,
    reason: &'static str,
) -> Result<Prepare, Problem> {
    repo::record_outcome(
        tx,
        &JobOutcome {
            id: job_id,
            status: "skipped",
            reason: Some(reason),
            model_key: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            now: Utc::now(),
        },
    )
    .await?;
    tracing::info!(job = job_id, reason, "AI 评论任务跳过");
    Ok(Prepare::Done(RunOutcome::Skipped(reason)))
}

async fn prepare(state: &AiState, job_id: i64, attempt: i16) -> Result<Prepare, Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let Some(job) = repo::lock_job(&mut tx, job_id).await? else {
        return Ok(Prepare::Done(RunOutcome::Skipped("missing_job")));
    };
    // 已有结果（TRIG-037）：不再调用模型、不再计费。
    if matches!(job.status.as_str(), "completed" | "failed" | "skipped")
        || job.reply_comment_id.is_some()
    {
        return Ok(Prepare::Done(RunOutcome::AlreadyDone));
    }
    repo::start_attempt(&mut tx, job_id, attempt, now).await?;

    macro_rules! skip_with {
        ($reason:expr) => {{
            let outcome = skip(&mut tx, job_id, $reason).await?;
            tx.commit().await?;
            return Ok(outcome);
        }};
    }

    // 复核（TRIG-016）：触发评论仍可见且不匿名，所在一级评论同样，帖子公开且不匿名。
    let (Some(post_id), Some(trigger_id)) = (job.post_id, job.trigger_comment_id) else {
        skip_with!("trigger_removed");
    };
    let Some(trigger) = ik_comments::interop::ai_comment(&mut tx, trigger_id)
        .await?
        .filter(|c| c.visible && !c.is_anonymous)
    else {
        skip_with!("trigger_removed");
    };
    let parent = match trigger.parent_id {
        None => None,
        Some(parent_id) => {
            let Some(parent) = ik_comments::interop::ai_comment(&mut tx, parent_id)
                .await?
                .filter(|c| c.visible && !c.is_anonymous)
            else {
                skip_with!("trigger_removed");
            };
            Some(parent)
        }
    };
    let Some(post) = ik_content::interop::post(&mut tx, post_id)
        .await?
        .filter(|p| p.is_public && !p.is_anonymous)
    else {
        skip_with!("post_unavailable");
    };
    let users =
        ik_auth::interop::active_users(&mut tx, &[job.quota_user_id, job.ai_user_id]).await?;
    let Some(commenter) = users.iter().find(|u| u.id == job.quota_user_id).cloned() else {
        skip_with!("sender_unavailable");
    };
    if !users.iter().any(|u| u.id == job.ai_user_id) {
        skip_with!("no_agent_target");
    }
    let catalog = state.catalog.get().await?;
    if !catalog.limits.global_enabled {
        skip_with!("global_disabled");
    }
    let Some(card) = catalog
        .card_for_user(job.ai_user_id)
        .filter(|c| c.comment_enabled)
        .cloned()
    else {
        skip_with!("no_agent_target");
    };
    // 评论回复不看会话级选择（CTX-031）。
    let Some(model) = dm::pick_model(&catalog, &card, None, None, |p| {
        catalog.providers.contains(p)
    })
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

    // 上下文（CTX-001～003）。
    let body = ik_content::interop::search_docs(&mut tx, &[post_id])
        .await?
        .into_iter()
        .next()
        .map(|d| truncate(&d.body, POST_BODY_MAX_CHARS))
        .unwrap_or_default();
    let post_context = PostContext {
        id: post.id,
        title: post.title.clone(),
        body,
    };
    let turns = repo::comment_memory(
        &mut tx,
        post_id,
        job.quota_user_id,
        job.ai_user_id,
        job.created_at,
        i64::try_from(state.config.max_history_messages / 2).unwrap_or(20),
    )
    .await?;
    let ids: Vec<i64> = turns
        .iter()
        .flat_map(|t| [t.trigger_comment_id, t.reply_comment_id])
        .collect();
    let bodies = ik_comments::interop::visible_bodies(&mut tx, &ids).await?;
    let memory: Vec<Turn> = turns
        .iter()
        .filter_map(|t| {
            let user = bodies.get(&t.trigger_comment_id)?;
            let assistant = reply_text(bodies.get(&t.reply_comment_id)?);
            (!assistant.is_empty()).then(|| Turn {
                user: comment_text(user),
                assistant,
            })
        })
        .collect();
    let parent_text = parent.map(|p| truncate(&comment_text(&p.body), PARENT_MAX_CHARS));
    let mut messages = build_messages(
        &card,
        &post_context,
        parent_text.as_deref(),
        &commenter.username,
        &comment_text(&trigger.body),
        &memory,
    );
    let toolbox = Toolbox::for_card(state, &catalog, &card, &state.mcp);
    toolbox.annotate_system(&mut messages);

    // 预占（QUOTA-005～010）：调用模型之前，按本次模型的倍率估高。不足时静默跳过。
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
        job.quota_user_id,
        amount,
        now,
        ttl,
    )
    .await?
    {
        Ok(hold) => hold,
        Err(denial) => skip_with!(denial.as_str()),
    };
    tx.commit().await?;
    Ok(Prepare::Ready(Box::new(Prepared {
        trigger_id,
        trigger_author: (commenter.id, commenter.username),
        model,
        default_model,
        providers: catalog.providers.clone(),
        hold,
        messages,
        toolbox,
        post_text: format!("{}\n{}", post_context.title, post_context.body),
        job,
    })))
}

/// 执行一个评论任务。`last_attempt` 时失败记为最终失败（TRIG-034）。
pub async fn run_job(
    state: &AiState,
    job_id: i64,
    attempt: i16,
    last_attempt: bool,
) -> Result<RunOutcome, Problem> {
    let prepared = match prepare(state, job_id, attempt).await? {
        Prepare::Done(outcome) => return Ok(outcome),
        Prepare::Ready(prepared) => *prepared,
    };
    // 评论回复同样可以用论坛工具（查看者 = 评论者），但不产生时间线（D-089 方案 A）。
    let ctx = StreamCtx {
        state,
        job_id,
        live: None,
        providers: prepared.providers.clone(),
        mode: OutputMode::Plain,
        max_chars: sanitize::COMMENT_MAX_CHARS,
        tools: Some(crate::tools::ToolContext::new(state, prepared.job.quota_user_id).await?),
        toolbox: prepared.toolbox.clone(),
    };
    let mut workflow = crate::workflow::Workflow::disabled();
    let generated = generate::generate(
        &ctx,
        &mut workflow,
        &prepared.model,
        prepared.default_model.as_ref(),
        &prepared.messages,
    )
    .await;
    finalize(
        state,
        &prepared,
        &generated.model,
        generated.streamed,
        attempt,
        last_attempt,
    )
    .await
}

/// 清洗并过沉浸守卫后的回复正文（不含开头的提及）。空串表示没有可用的回复。
pub(crate) fn finished_reply(content: &str, post_text: &str, job_id: i64) -> String {
    let text = sanitize::sanitize(content, OutputMode::Plain, sanitize::COMMENT_MAX_CHARS);
    if text.is_empty() {
        return text;
    }
    match immersion::check(&text, post_text) {
        immersion::Verdict::Keep => text,
        immersion::Verdict::Replace { text, reason } => {
            tracing::warn!(job = job_id, reason, "AI 评论回复触发沉浸守卫，已替换");
            text.to_owned()
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn finalize(
    state: &AiState,
    prepared: &Prepared,
    model: &Model,
    streamed: generate::Streamed,
    attempt: i16,
    last_attempt: bool,
) -> Result<RunOutcome, Problem> {
    let job_id = prepared.job.id;
    let usage = streamed.charged;
    let charge = quota::weighted(usage.total(), model.weight());
    let text = match streamed.end {
        // 只用最后一轮：前几轮多是调用工具前的过程话。
        End::Finished => finished_reply(&streamed.final_text, &prepared.post_text, job_id),
        _ => String::new(),
    };
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    repo::lock_job(&mut tx, job_id)
        .await?
        .ok_or_else(Problem::internal)?;
    let (status, reason, outcome) = if text.is_empty() {
        // 空回复（GEN-024/044）、上游错误、超时、超长都按失败；首帧超时与超长不重试（TRIG-034）。
        let (message, retryable) = match &streamed.end {
            End::Failed(error) => (error.to_string(), true),
            End::FirstDeltaTimeout => ("first_delta_timeout".to_owned(), false),
            End::TooLarge => ("upstream output too large".to_owned(), false),
            End::Finished | End::Stopped => ("empty reply after sanitize".to_owned(), true),
        };
        if retryable && !last_attempt {
            (
                "processing",
                Some(message.clone()),
                RunOutcome::Retry(message),
            )
        } else {
            ("failed", Some(message.clone()), RunOutcome::Failed(message))
        }
    } else {
        let (author_id, author_name) = &prepared.trigger_author;
        let body = format!(
            "{}\n{text}",
            ik_comments::text::mention_token(author_name, *author_id)
        );
        let inserted = ik_comments::interop::insert_ai_reply(
            &mut tx,
            &state.ids,
            ik_comments::interop::AiReply {
                trigger_id: prepared.trigger_id,
                ai_user_id: prepared.job.ai_user_id,
                body: &body,
            },
            now,
            &mut effects,
        )
        .await?;
        match inserted {
            Some(reply_id) => {
                repo::set_job_reply_comment(&mut tx, job_id, reply_id).await?;
                ("completed", None, RunOutcome::Completed)
            }
            // 生成期间触发评论被删除、隐藏，或帖子不再公开：不回复，但调用已经发生，照常计费。
            None => (
                "skipped",
                Some("trigger_removed".to_owned()),
                RunOutcome::Skipped("trigger_removed"),
            ),
        }
    };
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
            tracing::warn!(job = job_id, attempt, %reason, "AI 评论回复失败，稍后重试");
        }
        RunOutcome::Failed(reason) => {
            tracing::error!(job = job_id, attempt, %reason, "AI 评论回复最终失败");
        }
        _ => {}
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn reply_text_drops_the_system_mention() {
        assert_eq!(
            reply_text("@[绳匠](5)\n你好，**绳匠**。"),
            "你好，**绳匠**。"
        );
        assert_eq!(reply_text("@[绳匠](5)"), "");
        assert_eq!(reply_text("没有提及"), "没有提及");
    }

    #[test]
    fn comment_text_handles_tokens_and_images() {
        assert_eq!(
            comment_text("@[Fairy](9) 在吗 :ik-smile:"),
            "@Fairy 在吗 [表情:smile]"
        );
        assert_eq!(comment_text("  "), "[图片]");
    }

    #[test]
    fn finished_reply_sanitizes_and_guards() {
        assert_eq!(finished_reply("## 好的\n**收到**", "帖子", 1), "好的\n收到");
        assert_eq!(
            finished_reply("这是同人创作。", "帖子", 1),
            immersion::RUMOR_REPLY
        );
        assert!(
            finished_reply(&"字".repeat(3000), "帖子", 1)
                .chars()
                .count()
                <= 1500
        );
        assert_eq!(finished_reply("<br>", "帖子", 1), "");
    }
}
