//! SQL。表都归 ik-ai：`ai_settings`、`ai_providers`、`ai_models`、`ai_role_cards`、`ai_quota_*`、`ai_memories`、`ai_memory_settings`、`ai_memory_pairs`。

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgConnection;

// ---------- 配置 ----------

pub(crate) struct SettingsRow {
    pub global_enabled: bool,
    pub daily_tokens_per_user: i64,
    pub daily_tokens: i64,
    pub trace_enabled: bool,
}

pub(crate) async fn settings(conn: &mut PgConnection) -> sqlx::Result<Option<SettingsRow>> {
    sqlx::query_as!(
        SettingsRow,
        "SELECT global_enabled, daily_tokens_per_user, daily_tokens, trace_enabled
         FROM ai_settings WHERE id = 1"
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn upsert_settings(
    conn: &mut PgConnection,
    global_enabled: bool,
    daily_tokens_per_user: i64,
    daily_tokens: i64,
    trace_enabled: bool,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ai_settings (id, global_enabled, daily_tokens_per_user, daily_tokens, updated_at,
                                  trace_enabled)
         VALUES (1, $1, $2, $3, $4, $5)
         ON CONFLICT (id) DO UPDATE SET global_enabled = $1, daily_tokens_per_user = $2,
             daily_tokens = $3, updated_at = $4, trace_enabled = $5",
        global_enabled,
        daily_tokens_per_user,
        daily_tokens,
        now,
        trace_enabled,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct ProviderRow {
    pub name: String,
    pub display_name: String,
    pub base_url: String,
    pub api_key: String,
    pub enabled: bool,
    pub sort_order: i32,
}

pub(crate) async fn providers(conn: &mut PgConnection) -> sqlx::Result<Vec<ProviderRow>> {
    sqlx::query_as!(
        ProviderRow,
        "SELECT name, display_name, base_url, api_key, enabled, sort_order
         FROM ai_providers ORDER BY sort_order, id"
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct NewProvider<'a> {
    pub id: i64,
    pub name: &'a str,
    pub display_name: &'a str,
    pub base_url: &'a str,
    /// None = 保留原有密钥（新建时为空串）。
    pub api_key: Option<&'a str>,
    pub enabled: bool,
    pub sort_order: i32,
    pub now: DateTime<Utc>,
}

/// 按名字新增或更新。
pub(crate) async fn upsert_provider(
    conn: &mut PgConnection,
    p: &NewProvider<'_>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ai_providers (id, name, display_name, base_url, api_key, enabled, sort_order,
                                   created_at, updated_at)
         VALUES ($1, $2, $3, $4, COALESCE($5, ''), $6, $7, $8, $8)
         ON CONFLICT (name) DO UPDATE SET display_name = $3, base_url = $4,
             api_key = COALESCE($5, ai_providers.api_key), enabled = $6, sort_order = $7,
             updated_at = $8",
        p.id,
        p.name,
        p.display_name,
        p.base_url,
        p.api_key,
        p.enabled,
        p.sort_order,
        p.now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn models_using_provider(
    conn: &mut PgConnection,
    name: &str,
) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar!(
        "SELECT key FROM ai_models WHERE provider = $1 ORDER BY sort_order, id",
        name
    )
    .fetch_all(conn)
    .await
}

/// 返回是否删到了。
pub(crate) async fn delete_provider(conn: &mut PgConnection, name: &str) -> sqlx::Result<bool> {
    Ok(
        sqlx::query!("DELETE FROM ai_providers WHERE name = $1", name)
            .execute(conn)
            .await?
            .rows_affected()
            > 0,
    )
}

pub(crate) struct ModelRow {
    pub id: i64,
    pub key: String,
    pub display_name: String,
    pub description: String,
    pub badge: Option<String>,
    pub provider: String,
    pub provider_model: String,
    pub cost_weight: f64,
    pub vision: bool,
    pub enabled: bool,
    pub is_default: bool,
    pub sort_order: i32,
    pub params: serde_json::Value,
}

pub(crate) async fn models(conn: &mut PgConnection) -> sqlx::Result<Vec<ModelRow>> {
    sqlx::query_as!(
        ModelRow,
        "SELECT id, key, display_name, description, badge, provider, provider_model, cost_weight,
                vision, enabled, is_default, sort_order, params
         FROM ai_models ORDER BY sort_order, id"
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct NewModel<'a> {
    pub id: i64,
    pub key: &'a str,
    pub display_name: &'a str,
    pub description: &'a str,
    pub badge: Option<&'a str>,
    pub provider: &'a str,
    pub provider_model: &'a str,
    pub cost_weight: f64,
    pub vision: bool,
    pub enabled: bool,
    pub is_default: bool,
    pub sort_order: i32,
    pub params: &'a serde_json::Value,
    pub now: DateTime<Utc>,
}

/// 按 key 新增或更新，返回 id。
pub(crate) async fn upsert_model(conn: &mut PgConnection, m: &NewModel<'_>) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        "INSERT INTO ai_models (id, key, display_name, description, badge, provider, provider_model,
                                cost_weight, enabled, is_default, sort_order, created_at, updated_at,
                                vision, params)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12, $13, $14)
         ON CONFLICT (key) DO UPDATE SET display_name = $3, description = $4, badge = $5,
             provider = $6, provider_model = $7, cost_weight = $8, enabled = $9,
             is_default = $10, sort_order = $11, updated_at = $12, vision = $13, params = $14
         RETURNING id",
        m.id,
        m.key,
        m.display_name,
        m.description,
        m.badge,
        m.provider,
        m.provider_model,
        m.cost_weight,
        m.enabled,
        m.is_default,
        m.sort_order,
        m.now,
        m.vision,
        m.params,
    )
    .fetch_one(conn)
    .await
}

/// 清掉现有默认模型（换默认前先清，避开部分唯一索引）。
pub(crate) async fn clear_default_model(conn: &mut PgConnection) -> sqlx::Result<()> {
    sqlx::query!("UPDATE ai_models SET is_default = FALSE WHERE is_default")
        .execute(conn)
        .await?;
    Ok(())
}

pub(crate) struct CardRow {
    pub id: i64,
    pub slug: String,
    pub display_name: String,
    pub bio: String,
    pub avatar_media_id: Option<i64>,
    pub bound_user_id: i64,
    pub enabled: bool,
    pub knock_visible: bool,
    pub dm_enabled: bool,
    pub comment_enabled: bool,
    pub qq_enabled: bool,
    pub sort_order: i32,
    pub output_mode: String,
    pub system_prompt: String,
    pub examples: serde_json::Value,
    pub default_model_id: Option<i64>,
    pub allowed_model_ids: Vec<i64>,
    pub suggested_questions: Vec<String>,
    pub story_tools: bool,
    pub mcp_server_ids: Vec<i64>,
    pub params: serde_json::Value,
}

pub(crate) async fn cards(conn: &mut PgConnection) -> sqlx::Result<Vec<CardRow>> {
    sqlx::query_as!(
        CardRow,
        "SELECT id, slug, display_name, bio, avatar_media_id, bound_user_id, enabled, knock_visible,
                dm_enabled, comment_enabled, qq_enabled, sort_order, output_mode, system_prompt, examples,
                default_model_id, allowed_model_ids, suggested_questions, story_tools, mcp_server_ids,
                params
         FROM ai_role_cards ORDER BY sort_order, id"
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct NewCard<'a> {
    pub id: i64,
    pub slug: &'a str,
    pub display_name: &'a str,
    pub bio: &'a str,
    pub avatar_media_id: Option<i64>,
    pub bound_user_id: i64,
    pub enabled: bool,
    pub knock_visible: bool,
    pub dm_enabled: bool,
    pub comment_enabled: bool,
    pub qq_enabled: bool,
    pub sort_order: i32,
    pub output_mode: &'a str,
    pub system_prompt: &'a str,
    pub examples: &'a serde_json::Value,
    pub default_model_id: Option<i64>,
    pub allowed_model_ids: &'a [i64],
    pub suggested_questions: &'a [String],
    pub story_tools: bool,
    pub mcp_server_ids: &'a [i64],
    pub params: &'a serde_json::Value,
    pub now: DateTime<Utc>,
}

/// 按 slug 新增或更新，返回 id。
pub(crate) async fn upsert_card(conn: &mut PgConnection, c: &NewCard<'_>) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        "INSERT INTO ai_role_cards (id, slug, display_name, bio, avatar_media_id, bound_user_id,
                                    enabled, knock_visible, dm_enabled, comment_enabled, sort_order,
                                    output_mode, system_prompt, examples, default_model_id,
                                    allowed_model_ids, suggested_questions, story_tools,
                                    mcp_server_ids, created_at, updated_at, qq_enabled, params)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $19,
                 $20, $18, $18, $21, $22)
         ON CONFLICT (slug) DO UPDATE SET display_name = $3, bio = $4, avatar_media_id = $5,
             bound_user_id = $6, enabled = $7, knock_visible = $8, dm_enabled = $9,
             comment_enabled = $10, sort_order = $11, output_mode = $12, system_prompt = $13,
             examples = $14, default_model_id = $15, allowed_model_ids = $16,
             suggested_questions = $17, story_tools = $19, mcp_server_ids = $20, updated_at = $18,
             qq_enabled = $21, params = $22
         RETURNING id",
        c.id,
        c.slug,
        c.display_name,
        c.bio,
        c.avatar_media_id,
        c.bound_user_id,
        c.enabled,
        c.knock_visible,
        c.dm_enabled,
        c.comment_enabled,
        c.sort_order,
        c.output_mode,
        c.system_prompt,
        c.examples,
        c.default_model_id,
        c.allowed_model_ids,
        c.suggested_questions,
        c.now,
        c.story_tools,
        c.mcp_server_ids,
        c.qq_enabled,
        c.params,
    )
    .fetch_one(conn)
    .await
}

// ---------- 额度 ----------

/// 给某天某个计数器加 `delta`（可负，结果不低于 0），返回加完后的值。
pub(crate) async fn add_used(
    conn: &mut PgConnection,
    day: NaiveDate,
    user_id: i64,
    delta: i64,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        "INSERT INTO ai_quota_counters (day, user_id, used) VALUES ($1, $2, GREATEST($3::bigint, 0))
         ON CONFLICT (day, user_id) DO UPDATE SET used = GREATEST(0, ai_quota_counters.used + $3::bigint)
         RETURNING used",
        day,
        user_id,
        delta,
    )
    .fetch_one(conn)
    .await
}

pub(crate) async fn used(
    conn: &mut PgConnection,
    day: NaiveDate,
    user_id: i64,
) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar!(
        "SELECT used FROM ai_quota_counters WHERE day = $1 AND user_id = $2",
        day,
        user_id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn insert_hold(
    conn: &mut PgConnection,
    id: i64,
    user_id: i64,
    day: NaiveDate,
    amount: i64,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ai_quota_holds (id, user_id, day, amount, expires_at, created_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
        id,
        user_id,
        day,
        amount,
        expires_at,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct HoldRow {
    pub user_id: i64,
    pub day: NaiveDate,
    pub amount: i64,
}

/// 认领一份预占（删除即锁，QUOTA-014）。
pub(crate) async fn take_hold(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<HoldRow>> {
    sqlx::query_as!(
        HoldRow,
        "DELETE FROM ai_quota_holds WHERE id = $1 RETURNING user_id, day, amount",
        id
    )
    .fetch_optional(conn)
    .await
}

/// 认领一批过期的预占（QUOTA-016）。
pub(crate) async fn take_expired_holds(
    conn: &mut PgConnection,
    now: DateTime<Utc>,
    limit: i64,
) -> sqlx::Result<Vec<HoldRow>> {
    sqlx::query_as!(
        HoldRow,
        "DELETE FROM ai_quota_holds WHERE id IN (
             SELECT id FROM ai_quota_holds WHERE expires_at <= $1
             ORDER BY expires_at LIMIT $2 FOR UPDATE SKIP LOCKED)
         RETURNING user_id, day, amount",
        now,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 写结算去重标记；已经结算过时返回 false。
pub(crate) async fn mark_settled(
    conn: &mut PgConnection,
    key: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let inserted = sqlx::query!(
        "INSERT INTO ai_quota_settlements (key, created_at) VALUES ($1, $2)
         ON CONFLICT (key) DO NOTHING",
        key,
        now,
    )
    .execute(conn)
    .await?;
    Ok(inserted.rows_affected() == 1)
}

pub(crate) async fn prune_settlements(
    conn: &mut PgConnection,
    before: DateTime<Utc>,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "DELETE FROM ai_quota_settlements WHERE created_at < $1",
        before
    )
    .execute(conn)
    .await?
    .rows_affected())
}

/// 删掉很久以前的计数器（额度条只读当天）。
pub(crate) async fn prune_counters(
    conn: &mut PgConnection,
    before: NaiveDate,
) -> sqlx::Result<u64> {
    Ok(
        sqlx::query!("DELETE FROM ai_quota_counters WHERE day < $1", before)
            .execute(conn)
            .await?
            .rows_affected(),
    )
}

// ---------- AI 会话 ----------

pub(crate) struct SessionRow {
    pub conversation_id: i64,
    pub user_id: i64,
    pub ai_user_id: i64,
    pub context_epoch: i32,
    pub context_from: Option<DateTime<Utc>>,
    pub model_key: Option<String>,
    /// 会话滚动摘要（MEM-001）：`summary_epoch` 与当前代际一致时才有效。
    pub summary: Option<String>,
    pub summary_until: Option<DateTime<Utc>>,
    pub summary_epoch: Option<i32>,
}

impl SessionRow {
    /// 当前代际的摘要与它覆盖到的时刻（MEM-001：重置之后旧摘要作废）。
    pub fn current_summary(&self) -> Option<(&str, DateTime<Utc>)> {
        if self.summary_epoch != Some(self.context_epoch) {
            return None;
        }
        Some((self.summary.as_deref()?, self.summary_until?))
    }
}

pub(crate) async fn insert_session(
    conn: &mut PgConnection,
    conversation_id: i64,
    user_id: i64,
    ai_user_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ai_sessions (conversation_id, user_id, ai_user_id, created_at)
         VALUES ($1, $2, $3, $4)",
        conversation_id,
        user_id,
        ai_user_id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn session(
    conn: &mut PgConnection,
    conversation_id: i64,
) -> sqlx::Result<Option<SessionRow>> {
    sqlx::query_as!(
        SessionRow,
        "SELECT conversation_id, user_id, ai_user_id, context_epoch, context_from, model_key,
                summary, summary_until, summary_epoch
         FROM ai_sessions WHERE conversation_id = $1",
        conversation_id,
    )
    .fetch_optional(conn)
    .await
}

/// 锁住会话行：代际变更与「创建占位 / 定稿」在这里串行（CTX-024、GEN-006）。
pub(crate) async fn lock_session(
    conn: &mut PgConnection,
    conversation_id: i64,
) -> sqlx::Result<Option<SessionRow>> {
    sqlx::query_as!(
        SessionRow,
        "SELECT conversation_id, user_id, ai_user_id, context_epoch, context_from, model_key,
                summary, summary_until, summary_epoch
         FROM ai_sessions WHERE conversation_id = $1 FOR UPDATE",
        conversation_id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn set_session_model(
    conn: &mut PgConnection,
    conversation_id: i64,
    model_key: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE ai_sessions SET model_key = $2 WHERE conversation_id = $1",
        conversation_id,
        model_key,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 代际加一（CTX-021），返回新代际。
pub(crate) async fn bump_epoch(
    conn: &mut PgConnection,
    conversation_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<i32> {
    sqlx::query_scalar!(
        "UPDATE ai_sessions SET context_epoch = context_epoch + 1, context_from = $2
         WHERE conversation_id = $1 RETURNING context_epoch",
        conversation_id,
        now,
    )
    .fetch_one(conn)
    .await
}

// ---------- AI 任务 ----------

pub(crate) struct NewJob<'a> {
    pub id: i64,
    pub kind: &'a str,
    pub execution_key: &'a str,
    pub quota_user_id: i64,
    pub ai_user_id: i64,
    pub conversation_id: Option<i64>,
    pub trigger_message_id: Option<i64>,
    pub post_id: Option<i64>,
    pub trigger_comment_id: Option<i64>,
    pub generation_epoch: i32,
    pub model_override: Option<&'a str>,
    pub now: DateTime<Utc>,
}

/// 新建任务；去重键已存在时不写，返回 None（TRIG-030）。
pub(crate) async fn insert_job(
    conn: &mut PgConnection,
    j: &NewJob<'_>,
) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar!(
        "INSERT INTO ai_jobs (id, kind, execution_key, quota_user_id, ai_user_id, conversation_id,
                              trigger_message_id, post_id, trigger_comment_id, generation_epoch,
                              model_override, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12)
         ON CONFLICT (execution_key) DO NOTHING
         RETURNING id",
        j.id,
        j.kind,
        j.execution_key,
        j.quota_user_id,
        j.ai_user_id,
        j.conversation_id,
        j.trigger_message_id,
        j.post_id,
        j.trigger_comment_id,
        j.generation_epoch,
        j.model_override,
        j.now,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct JobRow {
    pub id: i64,
    pub status: String,
    pub quota_user_id: i64,
    pub ai_user_id: i64,
    pub conversation_id: Option<i64>,
    pub trigger_message_id: Option<i64>,
    pub generation_epoch: i32,
    pub model_override: Option<String>,
    pub reply_message_id: Option<i64>,
    pub stop_requested_at: Option<DateTime<Utc>>,
    pub post_id: Option<i64>,
    pub trigger_comment_id: Option<i64>,
    pub reply_comment_id: Option<i64>,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn lock_job(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<JobRow>> {
    sqlx::query_as!(
        JobRow,
        "SELECT id, status, quota_user_id, ai_user_id, conversation_id, trigger_message_id,
                generation_epoch, model_override, reply_message_id, stop_requested_at,
                post_id, trigger_comment_id, reply_comment_id, created_at
         FROM ai_jobs WHERE id = $1 FOR UPDATE",
        id,
    )
    .fetch_optional(conn)
    .await
}

/// 后台手动重试：把失败的任务改回排队，返回类型与会话（重新入队用）。不是失败状态时为 `None`。
pub(crate) async fn requeue_failed(
    conn: &mut PgConnection,
    id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<Option<(String, Option<i64>)>> {
    let row = sqlx::query!(
        "UPDATE ai_jobs SET status = 'queued', reason = NULL, finished_at = NULL, updated_at = $2
         WHERE id = $1 AND status = 'failed'
         RETURNING kind, conversation_id",
        id,
        now,
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|r| (r.kind, r.conversation_id)))
}

/// 后台删除失败的任务（调用记录一起删）。返回是否删掉了。
pub(crate) async fn delete_failed(conn: &mut PgConnection, id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "DELETE FROM ai_jobs WHERE id = $1 AND status = 'failed'",
        id
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn job_by_reply(
    conn: &mut PgConnection,
    reply_message_id: i64,
) -> sqlx::Result<Option<JobRow>> {
    sqlx::query_as!(
        JobRow,
        "SELECT id, status, quota_user_id, ai_user_id, conversation_id, trigger_message_id,
                generation_epoch, model_override, reply_message_id, stop_requested_at,
                post_id, trigger_comment_id, reply_comment_id, created_at
         FROM ai_jobs WHERE reply_message_id = $1",
        reply_message_id,
    )
    .fetch_optional(conn)
    .await
}

/// 开始一次尝试，返回这是第几次。次数一直往上加，后台手动重试后也不从 1 重来
/// （调用记录按次数存、时间线的 seq 按次数分段，WF-006）。
pub(crate) async fn start_attempt(
    conn: &mut PgConnection,
    id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<i16> {
    sqlx::query_scalar!(
        "UPDATE ai_jobs SET status = 'processing', attempts = attempts + 1, updated_at = $2
         WHERE id = $1 RETURNING attempts",
        id,
        now,
    )
    .fetch_one(conn)
    .await
}

pub(crate) async fn set_job_reply(
    conn: &mut PgConnection,
    id: i64,
    reply_message_id: i64,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE ai_jobs SET reply_message_id = $2 WHERE id = $1",
        id,
        reply_message_id,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn set_job_reply_comment(
    conn: &mut PgConnection,
    id: i64,
    reply_comment_id: i64,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE ai_jobs SET reply_comment_id = $2 WHERE id = $1",
        id,
        reply_comment_id,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 评论记忆里的一轮问答。
pub(crate) struct CommentTurn {
    pub trigger_comment_id: i64,
    pub reply_comment_id: i64,
}

/// 同一（帖子、评论者、AI）之前已经回复过的问答（CTX-003），最近的 `limit` 轮，按时间正序。
pub(crate) async fn comment_memory(
    conn: &mut PgConnection,
    post_id: i64,
    user_id: i64,
    ai_user_id: i64,
    before: DateTime<Utc>,
    limit: i64,
) -> sqlx::Result<Vec<CommentTurn>> {
    let mut turns = sqlx::query_as!(
        CommentTurn,
        r#"SELECT trigger_comment_id AS "trigger_comment_id!", reply_comment_id AS "reply_comment_id!"
           FROM ai_jobs
           WHERE post_id = $1 AND quota_user_id = $2 AND ai_user_id = $3 AND created_at < $4
             AND trigger_comment_id IS NOT NULL AND reply_comment_id IS NOT NULL
           ORDER BY created_at DESC, id DESC
           LIMIT $5"#,
        post_id,
        user_id,
        ai_user_id,
        before,
        limit,
    )
    .fetch_all(conn)
    .await?;
    turns.reverse();
    Ok(turns)
}

pub(crate) struct JobOutcome<'a> {
    pub id: i64,
    /// `processing`（还会重试）| `completed` | `failed` | `skipped`
    pub status: &'a str,
    pub reason: Option<&'a str>,
    pub model_key: Option<&'a str>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub now: DateTime<Utc>,
}

/// 记一次尝试的结果（TRIG-035：审计）。用量逐次累加。
pub(crate) async fn record_outcome(
    conn: &mut PgConnection,
    o: &JobOutcome<'_>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE ai_jobs SET status = $2, reason = left($3, 1000), model_key = COALESCE($4, model_key),
             prompt_tokens = prompt_tokens + $5, completion_tokens = completion_tokens + $6,
             updated_at = $7::timestamptz,
             finished_at = CASE WHEN $2 = 'processing' THEN NULL ELSE $7::timestamptz END
         WHERE id = $1",
        o.id,
        o.status,
        o.reason,
        o.model_key,
        o.prompt_tokens,
        o.completion_tokens,
        o.now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 一次尝试的运行指标（OBS-001）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct JobMetrics {
    pub cached_tokens: i64,
    pub first_delta_ms: Option<i32>,
    pub elapsed_ms: i32,
    pub tool_rounds: i16,
    pub tool_calls: i16,
    pub retries: i16,
}

/// 记一次尝试的指标：缓存命中、调用次数与重试逐次累加，延迟与耗时记最后一次。
pub(crate) async fn record_metrics(
    conn: &mut PgConnection,
    id: i64,
    m: &JobMetrics,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE ai_jobs SET cached_tokens = cached_tokens + $2, first_delta_ms = $3, elapsed_ms = $4,
             tool_rounds = tool_rounds + $5, tool_calls = tool_calls + $6, retries = retries + $7
         WHERE id = $1",
        id,
        m.cached_tokens,
        m.first_delta_ms,
        m.elapsed_ms,
        m.tool_rounds,
        m.tool_calls,
        m.retries,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct NewTrace<'a> {
    pub job_id: i64,
    pub attempt: i16,
    pub model_key: &'a str,
    pub request: &'a serde_json::Value,
    pub reply: &'a str,
    pub outcome: &'a str,
    pub now: DateTime<Utc>,
}

/// 存一次尝试的调用记录（OBS-002）。同一次尝试重复写时覆盖。
pub(crate) async fn insert_trace(conn: &mut PgConnection, t: &NewTrace<'_>) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ai_job_traces (job_id, attempt, model_key, request, reply, outcome, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (job_id, attempt) DO UPDATE
         SET model_key = EXCLUDED.model_key, request = EXCLUDED.request, reply = EXCLUDED.reply,
             outcome = EXCLUDED.outcome, created_at = EXCLUDED.created_at",
        t.job_id,
        t.attempt,
        t.model_key,
        t.request,
        t.reply,
        t.outcome,
        t.now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) struct TraceRow {
    pub attempt: i16,
    pub model_key: String,
    pub request: serde_json::Value,
    pub reply: String,
    pub outcome: String,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn traces(conn: &mut PgConnection, job_id: i64) -> sqlx::Result<Vec<TraceRow>> {
    sqlx::query_as!(
        TraceRow,
        "SELECT attempt, model_key, request, reply, outcome, created_at
         FROM ai_job_traces WHERE job_id = $1 ORDER BY attempt",
        job_id,
    )
    .fetch_all(conn)
    .await
}

/// 删掉过期的调用记录（OBS-002：保留 7 天）。
pub(crate) async fn prune_traces(
    conn: &mut PgConnection,
    before: DateTime<Utc>,
) -> sqlx::Result<u64> {
    Ok(
        sqlx::query!("DELETE FROM ai_job_traces WHERE created_at < $1", before)
            .execute(conn)
            .await?
            .rows_affected(),
    )
}

/// 同一会话里有没有更新的消息触发的回复任务（连发合并，GEN-027）。只看普通消息触发的（`dm:`），
/// 重新生成不参与合并。
pub(crate) async fn superseded(conn: &mut PgConnection, id: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM ai_jobs me
               JOIN ai_jobs n ON n.conversation_id = me.conversation_id
                             AND n.ai_user_id = me.ai_user_id
                             AND n.generation_epoch = me.generation_epoch
                             AND n.trigger_message_id > me.trigger_message_id
               WHERE me.id = $1 AND me.execution_key LIKE 'dm:%' AND n.execution_key LIKE 'dm:%'
                 AND n.status IN ('queued', 'processing', 'completed')
           ) AS "superseded!""#,
        id,
    )
    .fetch_one(conn)
    .await
}

pub(crate) async fn stop_requested(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<DateTime<Utc>>> {
    Ok(
        sqlx::query_scalar!("SELECT stop_requested_at FROM ai_jobs WHERE id = $1", id)
            .fetch_optional(conn)
            .await?
            .flatten(),
    )
}

/// 停止某条回复的生成（GEN-010）。返回是否有未结束的任务。
pub(crate) async fn request_stop(
    conn: &mut PgConnection,
    id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE ai_jobs SET stop_requested_at = COALESCE(stop_requested_at, $2)
         WHERE id = $1 AND finished_at IS NULL",
        id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 撤回触发消息时停止它未结束的生成（CTX-026）。
pub(crate) async fn stop_by_trigger(
    conn: &mut PgConnection,
    trigger_message_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "UPDATE ai_jobs SET stop_requested_at = COALESCE(stop_requested_at, $2)
         WHERE trigger_message_id = $1 AND finished_at IS NULL",
        trigger_message_id,
        now,
    )
    .execute(conn)
    .await?
    .rows_affected())
}

/// 重置上下文时停止旧代际未结束的生成（CTX-022）。
pub(crate) async fn stop_older_epochs(
    conn: &mut PgConnection,
    conversation_id: i64,
    epoch: i32,
    now: DateTime<Utc>,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "UPDATE ai_jobs SET stop_requested_at = COALESCE(stop_requested_at, $3)
         WHERE conversation_id = $1 AND generation_epoch < $2 AND finished_at IS NULL",
        conversation_id,
        epoch,
        now,
    )
    .execute(conn)
    .await?
    .rows_affected())
}

// ---------- 管理后台 ----------

/// 删除模型；返回 (id, 是否默认)。默认模型由调用方先拦住。
pub(crate) async fn model_by_key(
    conn: &mut PgConnection,
    key: &str,
) -> sqlx::Result<Option<(i64, bool)>> {
    Ok(
        sqlx::query!("SELECT id, is_default FROM ai_models WHERE key = $1", key)
            .fetch_optional(conn)
            .await?
            .map(|r| (r.id, r.is_default)),
    )
}

/// 删除模型：角色卡的默认模型由外键置空，白名单里去掉它。
pub(crate) async fn delete_model(conn: &mut PgConnection, id: i64) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE ai_role_cards SET allowed_model_ids = array_remove(allowed_model_ids, $1)
         WHERE $1 = ANY(allowed_model_ids)",
        id,
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!("DELETE FROM ai_models WHERE id = $1", id)
        .execute(conn)
        .await?;
    Ok(())
}

pub(crate) async fn delete_card(conn: &mut PgConnection, slug: &str) -> sqlx::Result<bool> {
    Ok(
        sqlx::query!("DELETE FROM ai_role_cards WHERE slug = $1", slug)
            .execute(conn)
            .await?
            .rows_affected()
            > 0,
    )
}

pub(crate) struct JobFilter {
    pub status: Option<String>,
    pub kind: Option<String>,
}

pub(crate) struct AdminJobRow {
    pub id: i64,
    pub kind: String,
    pub status: String,
    pub reason: Option<String>,
    pub quota_user_id: i64,
    pub ai_user_id: i64,
    pub conversation_id: Option<i64>,
    pub post_id: Option<i64>,
    pub attempts: i16,
    pub model_key: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cached_tokens: i64,
    pub first_delta_ms: Option<i32>,
    pub elapsed_ms: Option<i32>,
    pub tool_rounds: i16,
    pub tool_calls: i16,
    pub retries: i16,
    pub has_trace: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) async fn count_jobs(conn: &mut PgConnection, f: &JobFilter) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM ai_jobs
           WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR kind = $2)"#,
        f.status,
        f.kind,
    )
    .fetch_one(conn)
    .await
}

/// 任务审计，最近更新的在前（AI-027 死信查看：`status = failed`）。
pub(crate) async fn list_jobs(
    conn: &mut PgConnection,
    f: &JobFilter,
    limit: i64,
    offset: i64,
) -> sqlx::Result<Vec<AdminJobRow>> {
    sqlx::query_as!(
        AdminJobRow,
        r#"SELECT id, kind, status, reason, quota_user_id, ai_user_id, conversation_id, post_id,
                attempts, model_key, prompt_tokens, completion_tokens, cached_tokens, first_delta_ms,
                elapsed_ms, tool_rounds, tool_calls, retries,
                EXISTS (SELECT 1 FROM ai_job_traces t WHERE t.job_id = ai_jobs.id) AS "has_trace!",
                created_at, updated_at
         FROM ai_jobs
         WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR kind = $2)
         ORDER BY updated_at DESC, id DESC
         LIMIT $3 OFFSET $4"#,
        f.status,
        f.kind,
        limit,
        offset,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn failed_jobs_since(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM ai_jobs WHERE status = 'failed' AND updated_at >= $1"#,
        since,
    )
    .fetch_one(conn)
    .await
}

// ---------- MCP 服务（ADR 0004） ----------

pub(crate) struct McpServerRow {
    pub id: i64,
    pub name: String,
    pub display_name: String,
    pub url: String,
    pub auth_header: String,
    pub timeout_ms: i32,
    pub enabled: bool,
    pub sort_order: i32,
    pub last_listed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

pub(crate) async fn mcp_servers(conn: &mut PgConnection) -> sqlx::Result<Vec<McpServerRow>> {
    sqlx::query_as!(
        McpServerRow,
        "SELECT id, name, display_name, url, auth_header, timeout_ms, enabled,
                sort_order, last_listed_at, last_error
         FROM ai_mcp_servers ORDER BY sort_order, id"
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn mcp_server(
    conn: &mut PgConnection,
    name: &str,
) -> sqlx::Result<Option<McpServerRow>> {
    sqlx::query_as!(
        McpServerRow,
        "SELECT id, name, display_name, url, auth_header, timeout_ms, enabled,
                sort_order, last_listed_at, last_error
         FROM ai_mcp_servers WHERE name = $1",
        name,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct NewMcpServer<'a> {
    pub id: i64,
    pub name: &'a str,
    pub display_name: &'a str,
    pub url: &'a str,
    /// None = 保留原有的（新建时为空串）。
    pub auth_header: Option<&'a str>,
    pub timeout_ms: i32,
    pub enabled: bool,
    pub sort_order: i32,
    pub now: DateTime<Utc>,
}

pub(crate) async fn upsert_mcp_server(
    conn: &mut PgConnection,
    s: &NewMcpServer<'_>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        "INSERT INTO ai_mcp_servers (id, name, display_name, url, auth_header, timeout_ms,
                                     enabled, sort_order, created_at, updated_at)
         VALUES ($1, $2, $3, $4, COALESCE($5, ''), $6, $7, $8, $9, $9)
         ON CONFLICT (name) DO UPDATE SET display_name = $3, url = $4,
             auth_header = COALESCE($5, ai_mcp_servers.auth_header), timeout_ms = $6,
             enabled = $7, sort_order = $8, updated_at = $9
         RETURNING id",
        s.id,
        s.name,
        s.display_name,
        s.url,
        s.auth_header,
        s.timeout_ms,
        s.enabled,
        s.sort_order,
        s.now,
    )
    .fetch_one(conn)
    .await
}

/// 删除服务（工具级联删除），并从角色卡里去掉。返回是否删到。
pub(crate) async fn delete_mcp_server(conn: &mut PgConnection, name: &str) -> sqlx::Result<bool> {
    let Some(id) = sqlx::query_scalar!(
        "DELETE FROM ai_mcp_servers WHERE name = $1 RETURNING id",
        name,
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(false);
    };
    sqlx::query!(
        "UPDATE ai_role_cards SET mcp_server_ids = array_remove(mcp_server_ids, $1)
         WHERE $1 = ANY(mcp_server_ids)",
        id,
    )
    .execute(conn)
    .await?;
    Ok(true)
}

/// 记一次拉取清单的结果（成功时清空错误）。
pub(crate) async fn set_mcp_listed(
    conn: &mut PgConnection,
    id: i64,
    error: Option<&str>,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE ai_mcp_servers SET last_listed_at = $2, last_error = $3 WHERE id = $1",
        id,
        now,
        error,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 串行化工具清单的写入（暴露名跨服务唯一：先查冲突再写）。
pub(crate) async fn lock_mcp_tools(conn: &mut PgConnection) -> sqlx::Result<()> {
    sqlx::query!("SELECT pg_advisory_xact_lock(hashtextextended('ik:ai_mcp_tools', 0))")
        .execute(conn)
        .await?;
    Ok(())
}

pub(crate) struct McpToolRow {
    pub id: i64,
    pub server_id: i64,
    pub remote_name: String,
    pub exposed_name: String,
    pub remote: serde_json::Value,
    pub remote_digest: Vec<u8>,
    pub approved_digest: Option<Vec<u8>>,
    pub enabled: bool,
    pub timeline_text: Option<String>,
    pub missing: bool,
}

pub(crate) async fn mcp_tools(conn: &mut PgConnection) -> sqlx::Result<Vec<McpToolRow>> {
    sqlx::query_as!(
        McpToolRow,
        "SELECT id, server_id, remote_name, exposed_name, remote, remote_digest, approved_digest,
                enabled, timeline_text, missing
         FROM ai_mcp_tools ORDER BY server_id, remote_name"
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct RemoteToolUpsert<'a> {
    pub id: i64,
    pub server_id: i64,
    pub remote_name: &'a str,
    pub exposed_name: &'a str,
    pub remote: &'a serde_json::Value,
    pub remote_digest: &'a [u8],
    pub now: DateTime<Utc>,
}

/// 写入拉到的定义：新工具默认未启用；已有的只更新 `remote`，是否生效由摘要比较决定。
pub(crate) async fn upsert_remote_tool(
    conn: &mut PgConnection,
    t: &RemoteToolUpsert<'_>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ai_mcp_tools (id, server_id, remote_name, exposed_name, remote, remote_digest,
                                   created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $7)
         ON CONFLICT (server_id, remote_name) DO UPDATE SET remote = $5, remote_digest = $6,
             missing = FALSE,
             updated_at = CASE WHEN ai_mcp_tools.remote_digest = $6 AND NOT ai_mcp_tools.missing
                               THEN ai_mcp_tools.updated_at ELSE $7 END",
        t.id,
        t.server_id,
        t.remote_name,
        t.exposed_name,
        t.remote,
        t.remote_digest,
        t.now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 远端清单里没有的工具标记失效，有的清除标记。
pub(crate) async fn mark_missing_tools(
    conn: &mut PgConnection,
    server_id: i64,
    present: &[String],
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE ai_mcp_tools SET missing = NOT (remote_name = ANY($2)), updated_at = $3
         WHERE server_id = $1 AND missing IS DISTINCT FROM NOT (remote_name = ANY($2))",
        server_id,
        present,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 改一个工具：`approve` 时把当前远端定义记为已确认。返回改到的行数。
pub(crate) async fn update_tool(
    conn: &mut PgConnection,
    id: i64,
    enabled: bool,
    approve: bool,
    timeline_text: Option<&str>,
    now: DateTime<Utc>,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "UPDATE ai_mcp_tools SET enabled = $2, timeline_text = $4, updated_at = $5,
             approved = CASE WHEN $3 THEN remote ELSE approved END,
             approved_digest = CASE WHEN $3 THEN remote_digest ELSE approved_digest END
         WHERE id = $1",
        id,
        enabled,
        approve,
        timeline_text,
        now,
    )
    .execute(conn)
    .await?
    .rows_affected())
}

/// 运行时生效的工具：服务启用、工具启用、远端还在、远端定义与确认过的一致。
pub(crate) struct ActiveToolRow {
    pub server_id: i64,
    pub remote_name: String,
    pub exposed_name: String,
    pub approved: serde_json::Value,
    pub timeline_text: Option<String>,
}

pub(crate) async fn active_mcp_tools(conn: &mut PgConnection) -> sqlx::Result<Vec<ActiveToolRow>> {
    sqlx::query_as!(
        ActiveToolRow,
        r#"SELECT t.server_id, t.remote_name, t.exposed_name, t.approved AS "approved!",
                  t.timeline_text
           FROM ai_mcp_tools t JOIN ai_mcp_servers s ON s.id = t.server_id
           WHERE s.enabled AND t.enabled AND NOT t.missing AND t.approved IS NOT NULL
             AND t.approved_digest = t.remote_digest
           ORDER BY s.sort_order, s.id, t.remote_name"#
    )
    .fetch_all(conn)
    .await
}

/// 审批时锁住这一行（先查摘要再写）。
pub(crate) async fn mcp_tool_for_update(
    conn: &mut PgConnection,
    server_id: i64,
    remote_name: &str,
) -> sqlx::Result<Option<McpToolRow>> {
    sqlx::query_as!(
        McpToolRow,
        "SELECT id, server_id, remote_name, exposed_name, remote, remote_digest, approved_digest,
                enabled, timeline_text, missing
         FROM ai_mcp_tools WHERE server_id = $1 AND remote_name = $2 FOR UPDATE",
        server_id,
        remote_name,
    )
    .fetch_optional(conn)
    .await
}

// ---------- 记忆（ADR 0007） ----------

/// 写会话摘要（MEM-002）。代际已经变了（期间被重置）就不写，返回 false。
pub(crate) async fn set_summary(
    conn: &mut PgConnection,
    conversation_id: i64,
    epoch: i32,
    summary: &str,
    until: DateTime<Utc>,
) -> sqlx::Result<bool> {
    Ok(sqlx::query!(
        "UPDATE ai_sessions SET summary = $3, summary_until = $4, summary_epoch = $2
         WHERE conversation_id = $1 AND context_epoch = $2",
        conversation_id,
        epoch,
        summary,
        until,
    )
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// 长期记忆的提炼进度（MEM-020）。
pub(crate) async fn memory_until(
    conn: &mut PgConnection,
    conversation_id: i64,
) -> sqlx::Result<Option<DateTime<Utc>>> {
    Ok(sqlx::query_scalar!(
        "SELECT memory_until FROM ai_sessions WHERE conversation_id = $1",
        conversation_id,
    )
    .fetch_optional(conn)
    .await?
    .flatten())
}

/// 推进提炼进度（只前进）。
pub(crate) async fn advance_memory_until(
    conn: &mut PgConnection,
    conversation_id: i64,
    until: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE ai_sessions SET memory_until = GREATEST(COALESCE(memory_until, $2), $2)
         WHERE conversation_id = $1",
        conversation_id,
        until,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 同一用户与同一 AI 的其他会话（跨会话衔接，MEM-005）：会话 id 与各自的上下文起点。
pub(crate) async fn other_sessions(
    conn: &mut PgConnection,
    user_id: i64,
    ai_user_id: i64,
    exclude: i64,
) -> sqlx::Result<Vec<(i64, Option<DateTime<Utc>>)>> {
    Ok(sqlx::query!(
        "SELECT conversation_id, context_from FROM ai_sessions
         WHERE user_id = $1 AND ai_user_id = $2 AND conversation_id <> $3",
        user_id,
        ai_user_id,
        exclude,
    )
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|r| (r.conversation_id, r.context_from))
    .collect())
}

/// 全站的记忆设置（MEM-030）。
pub(crate) struct MemorySettingsRow {
    pub enabled: bool,
    pub model_key: Option<String>,
    pub extract_delay_secs: i32,
    pub bridge_hours: i32,
    pub embedding_provider: Option<String>,
    pub embedding_model: Option<String>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) async fn memory_settings(
    conn: &mut PgConnection,
) -> sqlx::Result<Option<MemorySettingsRow>> {
    sqlx::query_as!(
        MemorySettingsRow,
        "SELECT enabled, model_key, extract_delay_secs, bridge_hours, embedding_provider,
                embedding_model, updated_at
         FROM ai_memory_settings WHERE id = 1"
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct NewMemorySettings<'a> {
    pub enabled: bool,
    pub model_key: Option<&'a str>,
    pub extract_delay_secs: i32,
    pub bridge_hours: i32,
    pub embedding_provider: Option<&'a str>,
    pub embedding_model: Option<&'a str>,
    pub now: DateTime<Utc>,
}

pub(crate) async fn upsert_memory_settings(
    conn: &mut PgConnection,
    settings: &NewMemorySettings<'_>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ai_memory_settings (id, enabled, model_key, extract_delay_secs, bridge_hours,
                                         embedding_provider, embedding_model, updated_at)
         VALUES (1, $1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (id) DO UPDATE SET
             enabled = EXCLUDED.enabled, model_key = EXCLUDED.model_key,
             extract_delay_secs = EXCLUDED.extract_delay_secs, bridge_hours = EXCLUDED.bridge_hours,
             embedding_provider = EXCLUDED.embedding_provider,
             embedding_model = EXCLUDED.embedding_model, updated_at = EXCLUDED.updated_at",
        settings.enabled,
        settings.model_key,
        settings.extract_delay_secs,
        settings.bridge_hours,
        settings.embedding_provider,
        settings.embedding_model,
        settings.now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 记忆的全站统计（管理后台）。
pub(crate) struct MemoryStatsRow {
    pub users: i64,
    pub profile: i64,
    pub relation: i64,
    pub event: i64,
    pub pending_follow_ups: i64,
    pub with_embedding: i64,
}

pub(crate) async fn memory_stats(conn: &mut PgConnection) -> sqlx::Result<MemoryStatsRow> {
    sqlx::query_as!(
        MemoryStatsRow,
        r#"SELECT count(DISTINCT user_id) AS "users!",
                  count(*) FILTER (WHERE kind = 'profile') AS "profile!",
                  count(*) FILTER (WHERE kind = 'relation') AS "relation!",
                  count(*) FILTER (WHERE kind = 'event') AS "event!",
                  count(*) FILTER (WHERE follow_up_on IS NOT NULL AND followed_up_at IS NULL)
                      AS "pending_follow_ups!",
                  count(*) FILTER (WHERE embedding IS NOT NULL) AS "with_embedding!"
           FROM ai_memories"#
    )
    .fetch_one(conn)
    .await
}

/// 同一用户的记忆写入串行（去重、条数上限在事务里判断）。
pub(crate) async fn lock_memories(conn: &mut PgConnection, user_id: i64) -> sqlx::Result<()> {
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended('ik:ai_memories:' || $1::bigint, 0))",
        user_id,
    )
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryRow {
    pub id: i64,
    pub user_id: i64,
    pub ai_user_id: Option<i64>,
    pub kind: String,
    pub content: String,
    pub pinned: bool,
    pub origin: String,
    pub sensitivity: String,
    pub importance: i16,
    pub happened_on: Option<NaiveDate>,
    pub follow_up_on: Option<NaiveDate>,
    pub followed_up_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// 在哪个公开场合说的（ADR 0008）；为空是私下说的。
    pub source_scope: Option<String>,
}

/// 事件记忆的检索候选（MEM-052）：带向量。
#[derive(Debug, Clone)]
pub(crate) struct EventRow {
    pub id: i64,
    pub content: String,
    pub pinned: bool,
    pub importance: i16,
    pub happened_on: Option<NaiveDate>,
    pub updated_at: DateTime<Utc>,
    pub embedding: Option<Vec<f32>>,
    pub embedding_model: Option<String>,
}

/// 一次对话总会带上的记忆：用户档案 + 与这个 AI 的关系记忆（不含事件）。置顶的在前，其余新的在前。
pub(crate) async fn memories_for(
    conn: &mut PgConnection,
    user_id: i64,
    ai_user_id: i64,
) -> sqlx::Result<Vec<MemoryRow>> {
    sqlx::query_as!(
        MemoryRow,
        "SELECT id, user_id, ai_user_id, kind, content, pinned, origin, sensitivity, importance,
                happened_on, follow_up_on, followed_up_at, created_at, updated_at, source_scope
         FROM ai_memories
         WHERE user_id = $1 AND kind <> 'event' AND (ai_user_id IS NULL OR ai_user_id = $2)
         ORDER BY pinned DESC, updated_at DESC, id DESC",
        user_id,
        ai_user_id,
    )
    .fetch_all(conn)
    .await
}

/// 同一范围、同一种类的记忆（判重用）。
pub(crate) async fn scope_memories(
    conn: &mut PgConnection,
    user_id: i64,
    ai_user_id: Option<i64>,
    kind: &str,
) -> sqlx::Result<Vec<MemoryRow>> {
    sqlx::query_as!(
        MemoryRow,
        "SELECT id, user_id, ai_user_id, kind, content, pinned, origin, sensitivity, importance,
                happened_on, follow_up_on, followed_up_at, created_at, updated_at, source_scope
         FROM ai_memories
         WHERE user_id = $1 AND ai_user_id IS NOT DISTINCT FROM $2 AND kind = $3",
        user_id,
        ai_user_id,
        kind,
    )
    .fetch_all(conn)
    .await
}

/// 事件检索的候选：与这个 AI 的事件，最近更新的 `limit` 条。
/// `scope` 不为空时只取来源是这个公开场合的（ADR 0008）。
pub(crate) async fn events_for(
    conn: &mut PgConnection,
    user_id: i64,
    ai_user_id: i64,
    scope: Option<&str>,
    limit: i64,
) -> sqlx::Result<Vec<EventRow>> {
    sqlx::query_as!(
        EventRow,
        "SELECT id, content, pinned, importance, happened_on, updated_at, embedding, embedding_model
         FROM ai_memories
         WHERE user_id = $1 AND ai_user_id = $2 AND kind = 'event'
           AND ($4::text IS NULL OR source_scope = $4)
         ORDER BY updated_at DESC, id DESC
         LIMIT $3",
        user_id,
        ai_user_id,
        limit,
        scope,
    )
    .fetch_all(conn)
    .await
}

/// 提炼时给模型看的事件（MEM-021）：`since` 之后更新过的，或还有没问起的待跟进的。新的在前。
pub(crate) async fn recent_events(
    conn: &mut PgConnection,
    user_id: i64,
    ai_user_id: i64,
    since: DateTime<Utc>,
    limit: i64,
) -> sqlx::Result<Vec<MemoryRow>> {
    sqlx::query_as!(
        MemoryRow,
        "SELECT id, user_id, ai_user_id, kind, content, pinned, origin, sensitivity, importance,
                happened_on, follow_up_on, followed_up_at, created_at, updated_at, source_scope
         FROM ai_memories
         WHERE user_id = $1 AND ai_user_id = $2 AND kind = 'event'
           AND (updated_at >= $3 OR (follow_up_on IS NOT NULL AND followed_up_at IS NULL))
         ORDER BY updated_at DESC, id DESC
         LIMIT $4",
        user_id,
        ai_user_id,
        since,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 到期、还没问起的待跟进（MEM-055）：`follow_up_on` 在 `[oldest, today]` 之间，重要的、早到期的在前。
pub(crate) async fn due_follow_ups(
    conn: &mut PgConnection,
    user_id: i64,
    ai_user_id: i64,
    today: NaiveDate,
    oldest: NaiveDate,
    limit: i64,
) -> sqlx::Result<Vec<MemoryRow>> {
    sqlx::query_as!(
        MemoryRow,
        "SELECT id, user_id, ai_user_id, kind, content, pinned, origin, sensitivity, importance,
                happened_on, follow_up_on, followed_up_at, created_at, updated_at, source_scope
         FROM ai_memories
         WHERE user_id = $1 AND ai_user_id = $2 AND kind = 'event'
           AND follow_up_on BETWEEN $4 AND $3 AND followed_up_at IS NULL
         ORDER BY importance DESC, follow_up_on, id
         LIMIT $5",
        user_id,
        ai_user_id,
        today,
        oldest,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 记下这些待跟进已经问起过（MEM-056）。
pub(crate) async fn mark_followed_up(
    conn: &mut PgConnection,
    ids: &[i64],
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE ai_memories SET followed_up_at = $2
         WHERE id = ANY($1) AND followed_up_at IS NULL",
        ids,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 用户的全部记忆（管理后台）。
pub(crate) async fn all_memories(
    conn: &mut PgConnection,
    user_id: i64,
) -> sqlx::Result<Vec<MemoryRow>> {
    sqlx::query_as!(
        MemoryRow,
        "SELECT id, user_id, ai_user_id, kind, content, pinned, origin, sensitivity, importance,
                happened_on, follow_up_on, followed_up_at, created_at, updated_at, source_scope
         FROM ai_memories WHERE user_id = $1
         ORDER BY pinned DESC, updated_at DESC, id DESC",
        user_id,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn lock_memory(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<MemoryRow>> {
    sqlx::query_as!(
        MemoryRow,
        "SELECT id, user_id, ai_user_id, kind, content, pinned, origin, sensitivity, importance,
                happened_on, follow_up_on, followed_up_at, created_at, updated_at, source_scope
         FROM ai_memories WHERE id = $1 FOR UPDATE",
        id,
    )
    .fetch_optional(conn)
    .await
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct NewMemory<'a> {
    pub id: i64,
    pub user_id: i64,
    pub ai_user_id: Option<i64>,
    pub kind: &'a str,
    pub content: &'a str,
    pub origin: &'a str,
    pub sensitivity: &'a str,
    pub importance: i16,
    pub happened_on: Option<NaiveDate>,
    pub follow_up_on: Option<NaiveDate>,
    pub source_conversation_id: Option<i64>,
    /// 公开场合说的才有（ADR 0008），见 [`MemoryRow::source_scope`]。
    pub source_scope: Option<&'a str>,
    pub now: DateTime<Utc>,
}

pub(crate) async fn insert_memory(
    conn: &mut PgConnection,
    memory: &NewMemory<'_>,
) -> sqlx::Result<MemoryRow> {
    sqlx::query_as!(
        MemoryRow,
        "INSERT INTO ai_memories (id, user_id, ai_user_id, kind, content, origin, sensitivity,
                                  importance, happened_on, follow_up_on, source_conversation_id,
                                  source_scope, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $13, $12, $12)
         RETURNING id, user_id, ai_user_id, kind, content, pinned, origin, sensitivity, importance,
                   happened_on, follow_up_on, followed_up_at, created_at, updated_at, source_scope",
        memory.id,
        memory.user_id,
        memory.ai_user_id,
        memory.kind,
        memory.content,
        memory.origin,
        memory.sensitivity,
        memory.importance,
        memory.happened_on,
        memory.follow_up_on,
        memory.source_conversation_id,
        memory.now,
        memory.source_scope,
    )
    .fetch_one(conn)
    .await
}

/// 改一条记忆。字段为 None 的不改；日期是 `Some(None)` 时清除。改了正文时向量作废（重算），
/// 改了待跟进日期时「已问起」清除。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MemoryPatch<'a> {
    pub content: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub pinned: Option<bool>,
    pub sensitivity: Option<&'a str>,
    pub importance: Option<i16>,
    pub happened_on: Option<Option<NaiveDate>>,
    pub follow_up_on: Option<Option<NaiveDate>>,
}

pub(crate) async fn update_memory(
    conn: &mut PgConnection,
    id: i64,
    patch: &MemoryPatch<'_>,
    now: DateTime<Utc>,
) -> sqlx::Result<MemoryRow> {
    sqlx::query_as!(
        MemoryRow,
        "UPDATE ai_memories SET
             content = COALESCE($2, content),
             origin = COALESCE($3, origin),
             pinned = COALESCE($4, pinned),
             sensitivity = COALESCE($5, sensitivity),
             importance = COALESCE($6, importance),
             happened_on = CASE WHEN $7 THEN $8 ELSE happened_on END,
             follow_up_on = CASE WHEN $9 THEN $10 ELSE follow_up_on END,
             followed_up_at = CASE WHEN $9 THEN NULL ELSE followed_up_at END,
             embedding = CASE WHEN $2::text IS NULL THEN embedding END,
             embedding_model = CASE WHEN $2::text IS NULL THEN embedding_model END,
             updated_at = $11
         WHERE id = $1
         RETURNING id, user_id, ai_user_id, kind, content, pinned, origin, sensitivity, importance,
                   happened_on, follow_up_on, followed_up_at, created_at, updated_at, source_scope",
        id,
        patch.content,
        patch.origin,
        patch.pinned,
        patch.sensitivity,
        patch.importance,
        patch.happened_on.is_some(),
        patch.happened_on.flatten(),
        patch.follow_up_on.is_some(),
        patch.follow_up_on.flatten(),
        now,
    )
    .fetch_one(conn)
    .await
}

/// 删掉用户自己的几条记忆，返回实际删掉的。
pub(crate) async fn delete_memories(
    conn: &mut PgConnection,
    user_id: i64,
    ids: &[i64],
) -> sqlx::Result<Vec<MemoryRow>> {
    sqlx::query_as!(
        MemoryRow,
        "DELETE FROM ai_memories WHERE user_id = $1 AND id = ANY($2)
         RETURNING id, user_id, ai_user_id, kind, content, pinned, origin, sensitivity, importance,
                   happened_on, follow_up_on, followed_up_at, created_at, updated_at, source_scope",
        user_id,
        ids,
    )
    .fetch_all(conn)
    .await
}

/// 清空记忆（MEM-032）：给了 AI 只清与它的关系与事件记忆，否则清空全部。返回删掉的条数。
pub(crate) async fn clear_memories(
    conn: &mut PgConnection,
    user_id: i64,
    ai_user_id: Option<i64>,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "DELETE FROM ai_memories WHERE user_id = $1 AND ($2::bigint IS NULL OR ai_user_id = $2)",
        user_id,
        ai_user_id,
    )
    .execute(conn)
    .await?
    .rows_affected())
}

/// 超出条数上限时淘汰（MEM-013、MEM-053）：不动置顶的；先淘汰后台提炼的，再按重要度从低到高、
/// 最久没更新的。返回淘汰的条数。
pub(crate) async fn trim_memories(
    conn: &mut PgConnection,
    user_id: i64,
    ai_user_id: Option<i64>,
    kind: &str,
    limit: i64,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "DELETE FROM ai_memories WHERE id IN (
             SELECT id FROM ai_memories
             WHERE user_id = $1 AND ai_user_id IS NOT DISTINCT FROM $2 AND kind = $3 AND NOT pinned
             ORDER BY (origin = 'extracted') DESC, importance, updated_at, id
             LIMIT GREATEST((SELECT count(*) FROM ai_memories
                             WHERE user_id = $1 AND ai_user_id IS NOT DISTINCT FROM $2
                               AND kind = $3) - $4, 0))",
        user_id,
        ai_user_id,
        kind,
        limit,
    )
    .execute(conn)
    .await?
    .rows_affected())
}

/// 还没有（或模型不同的）向量的事件（MEM-054），最多 `limit` 条。
pub(crate) async fn missing_embeddings(
    conn: &mut PgConnection,
    user_id: i64,
    ai_user_id: i64,
    model: &str,
    limit: i64,
) -> sqlx::Result<Vec<(i64, String)>> {
    Ok(sqlx::query!(
        "SELECT id, content FROM ai_memories
         WHERE user_id = $1 AND ai_user_id = $2 AND kind = 'event'
           AND embedding_model IS DISTINCT FROM $3
         ORDER BY updated_at DESC, id DESC
         LIMIT $4",
        user_id,
        ai_user_id,
        model,
        limit,
    )
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|r| (r.id, r.content))
    .collect())
}

/// 写向量。正文在算向量期间被改过（`content` 不一致）就不写。
pub(crate) async fn set_embedding(
    conn: &mut PgConnection,
    id: i64,
    content: &str,
    embedding: &[f32],
    model: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE ai_memories SET embedding = $3, embedding_model = $4
         WHERE id = $1 AND content = $2",
        id,
        content,
        embedding,
        model,
    )
    .execute(conn)
    .await?;
    Ok(())
}

// ---------- 记忆第三期：整理、遗忘、翻聊天记录（ADR 0007） ----------

/// 同一用户与同一 AI 的全部会话（久别重逢算上次聊天的时刻，MEM-100）。
pub(crate) async fn pair_sessions(
    conn: &mut PgConnection,
    user_id: i64,
    ai_user_id: i64,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT conversation_id FROM ai_sessions WHERE user_id = $1 AND ai_user_id = $2",
        user_id,
        ai_user_id,
    )
    .fetch_all(conn)
    .await
}

/// 翻聊天记录能看的会话（MEM-095）：同一用户与同一 AI 的会话与各自的上下文起点。
pub(crate) async fn recall_sessions(
    conn: &mut PgConnection,
    user_id: i64,
    ai_user_id: i64,
) -> sqlx::Result<Vec<(i64, Option<DateTime<Utc>>)>> {
    Ok(sqlx::query!(
        "SELECT conversation_id, context_from FROM ai_sessions WHERE user_id = $1 AND ai_user_id = $2",
        user_id,
        ai_user_id,
    )
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|r| (r.conversation_id, r.context_from))
    .collect())
}

/// 记下这些事件刚被想起（MEM-090）。
pub(crate) async fn touch_recalled(
    conn: &mut PgConnection,
    ids: &[i64],
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE ai_memories SET recalled_at = $2 WHERE id = ANY($1)",
        ids,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 遗忘（MEM-090）：后台提炼、没置顶、没有待跟进的事件，重要度 ≤2 的半年没更新且 90 天没被想起、
/// 重要度 3 的一年没更新且半年没被想起，就删掉。返回删掉的条数。
pub(crate) async fn forget_stale(conn: &mut PgConnection, now: DateTime<Utc>) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "DELETE FROM ai_memories m
         WHERE m.kind = 'event' AND m.origin = 'extracted' AND NOT m.pinned
           AND (m.follow_up_on IS NULL OR m.followed_up_at IS NOT NULL)
           AND ((m.importance <= 2 AND m.updated_at < $1::timestamptz - interval '180 days'
                 AND (m.recalled_at IS NULL OR m.recalled_at < $1::timestamptz - interval '90 days'))
             OR (m.importance = 3 AND m.updated_at < $1::timestamptz - interval '365 days'
                 AND (m.recalled_at IS NULL OR m.recalled_at < $1::timestamptz - interval '180 days')))",
        now,
    )
    .execute(conn)
    .await?
    .rows_affected())
}

/// 该整理的（用户, AI）（MEM-080）：`old_before` 之前的、可以整理的事件不少于 `min_events` 条，且上次整理在
/// `stale_before` 之前（或从没整理过）。可以整理的多的在前。
pub(crate) async fn consolidation_candidates(
    conn: &mut PgConnection,
    min_events: i64,
    old_before: DateTime<Utc>,
    stale_before: DateTime<Utc>,
    limit: i64,
) -> sqlx::Result<Vec<(i64, i64)>> {
    Ok(sqlx::query!(
        r#"SELECT m.user_id, m.ai_user_id AS "ai_user_id!" FROM ai_memories m
           LEFT JOIN ai_memory_pairs p ON p.user_id = m.user_id AND p.ai_user_id = m.ai_user_id
           WHERE m.kind = 'event' AND m.origin = 'extracted' AND NOT m.pinned AND m.updated_at < $2
             AND (m.follow_up_on IS NULL OR m.followed_up_at IS NOT NULL)
             AND (p.consolidated_at IS NULL OR p.consolidated_at < $3)
           GROUP BY m.user_id, m.ai_user_id
           HAVING count(*) >= $1
           ORDER BY count(*) DESC, m.user_id, m.ai_user_id
           LIMIT $4"#,
        min_events,
        old_before,
        stale_before,
        limit,
    )
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|r| (r.user_id, r.ai_user_id))
    .collect())
}

/// 可以整理的事件（MEM-081）：后台提炼、没置顶、没有待跟进、`before` 之前更新的，按时间从早到晚。
pub(crate) async fn consolidatable_events(
    conn: &mut PgConnection,
    user_id: i64,
    ai_user_id: i64,
    before: DateTime<Utc>,
    limit: i64,
) -> sqlx::Result<Vec<MemoryRow>> {
    sqlx::query_as!(
        MemoryRow,
        "SELECT id, user_id, ai_user_id, kind, content, pinned, origin, sensitivity, importance,
                happened_on, follow_up_on, followed_up_at, created_at, updated_at, source_scope
         FROM ai_memories
         WHERE user_id = $1 AND ai_user_id = $2 AND kind = 'event' AND origin = 'extracted'
           AND NOT pinned AND updated_at < $3
           AND (follow_up_on IS NULL OR followed_up_at IS NOT NULL)
         ORDER BY happened_on NULLS LAST, updated_at, id
         LIMIT $4",
        user_id,
        ai_user_id,
        before,
        limit,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn mark_consolidated(
    conn: &mut PgConnection,
    user_id: i64,
    ai_user_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ai_memory_pairs (user_id, ai_user_id, consolidated_at) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, ai_user_id) DO UPDATE SET consolidated_at = EXCLUDED.consolidated_at",
        user_id,
        ai_user_id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

// ---------- 按场合共享的记忆（ADR 0008 第三期，QQ 群共享记忆） ----------

#[derive(Debug, Clone)]
pub(crate) struct SharedMemoryRow {
    pub id: i64,
    pub ai_user_id: i64,
    pub content: String,
    pub origin: String,
    pub pinned: bool,
    pub created_by_user_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 同一场合的共享记忆串行写入（与个人记忆的锁分开）。
pub(crate) async fn lock_shared_memories(conn: &mut PgConnection, scope: &str) -> sqlx::Result<()> {
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended('ik:ai_shared_memories:' || $1::text, 0))",
        scope,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 一个场合的共享记忆；`ai_user_id` 为空时取全部角色的。置顶的在前，其余新的在前。
pub(crate) async fn shared_memories(
    conn: &mut PgConnection,
    scope: &str,
    ai_user_id: Option<i64>,
) -> sqlx::Result<Vec<SharedMemoryRow>> {
    sqlx::query_as!(
        SharedMemoryRow,
        "SELECT id, ai_user_id, content, origin, pinned, created_by_user_id, created_at, updated_at
         FROM ai_shared_memories
         WHERE scope = $1 AND ($2::bigint IS NULL OR ai_user_id = $2)
         ORDER BY pinned DESC, updated_at DESC, id DESC",
        scope,
        ai_user_id,
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct NewSharedMemory<'a> {
    pub id: i64,
    pub scope: &'a str,
    pub ai_user_id: i64,
    pub content: &'a str,
    pub origin: &'a str,
    pub pinned: bool,
    pub created_by_user_id: Option<i64>,
    pub now: DateTime<Utc>,
}

pub(crate) async fn insert_shared_memory(
    conn: &mut PgConnection,
    memory: &NewSharedMemory<'_>,
) -> sqlx::Result<SharedMemoryRow> {
    sqlx::query_as!(
        SharedMemoryRow,
        "INSERT INTO ai_shared_memories (id, scope, ai_user_id, content, origin, pinned,
                                         created_by_user_id, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
         RETURNING id, ai_user_id, content, origin, pinned, created_by_user_id, created_at, updated_at",
        memory.id,
        memory.scope,
        memory.ai_user_id,
        memory.content,
        memory.origin,
        memory.pinned,
        memory.created_by_user_id,
        memory.now,
    )
    .fetch_one(conn)
    .await
}

/// 改正文与置顶（`None` 不改）。返回改后的行；不在这个场合里时为 None。
pub(crate) async fn update_shared_memory(
    conn: &mut PgConnection,
    scope: &str,
    id: i64,
    content: Option<&str>,
    origin: Option<&str>,
    pinned: Option<bool>,
    now: DateTime<Utc>,
) -> sqlx::Result<Option<SharedMemoryRow>> {
    sqlx::query_as!(
        SharedMemoryRow,
        "UPDATE ai_shared_memories
         SET content = COALESCE($3, content), origin = COALESCE($4, origin),
             pinned = COALESCE($5, pinned), updated_at = $6
         WHERE scope = $1 AND id = $2
         RETURNING id, ai_user_id, content, origin, pinned, created_by_user_id, created_at, updated_at",
        scope,
        id,
        content,
        origin,
        pinned,
        now,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn delete_shared_memories(
    conn: &mut PgConnection,
    scope: &str,
    ids: &[i64],
) -> sqlx::Result<Vec<SharedMemoryRow>> {
    sqlx::query_as!(
        SharedMemoryRow,
        "DELETE FROM ai_shared_memories WHERE scope = $1 AND id = ANY($2)
         RETURNING id, ai_user_id, content, origin, pinned, created_by_user_id, created_at, updated_at",
        scope,
        ids,
    )
    .fetch_all(conn)
    .await
}

/// 超出上限时淘汰最旧的未置顶记忆。
pub(crate) async fn trim_shared_memories(
    conn: &mut PgConnection,
    scope: &str,
    ai_user_id: i64,
    limit: i64,
) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        "DELETE FROM ai_shared_memories WHERE id IN (
             SELECT id FROM ai_shared_memories
             WHERE scope = $1 AND ai_user_id = $2 AND NOT pinned
             ORDER BY updated_at DESC, id DESC
             OFFSET GREATEST(0, $3 - (SELECT count(*) FROM ai_shared_memories
                                      WHERE scope = $1 AND ai_user_id = $2 AND pinned)))",
        scope,
        ai_user_id,
        limit,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 删掉一个场合的全部共享记忆（机器人退群很久以后清理）。
pub(crate) async fn clear_shared_memories(
    conn: &mut PgConnection,
    scope: &str,
) -> sqlx::Result<u64> {
    let result = sqlx::query!("DELETE FROM ai_shared_memories WHERE scope = $1", scope)
        .execute(conn)
        .await?;
    Ok(result.rows_affected())
}
