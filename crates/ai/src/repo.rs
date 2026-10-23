//! SQL。表都归 ik-ai：`ai_settings`、`ai_providers`、`ai_models`、`ai_role_cards`、`ai_quota_*`。

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgConnection;

// ---------- 配置 ----------

pub(crate) struct SettingsRow {
    pub global_enabled: bool,
    pub daily_tokens_per_user: i64,
    pub daily_tokens: i64,
}

pub(crate) async fn settings(conn: &mut PgConnection) -> sqlx::Result<Option<SettingsRow>> {
    sqlx::query_as!(
        SettingsRow,
        "SELECT global_enabled, daily_tokens_per_user, daily_tokens FROM ai_settings WHERE id = 1"
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn upsert_settings(
    conn: &mut PgConnection,
    global_enabled: bool,
    daily_tokens_per_user: i64,
    daily_tokens: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO ai_settings (id, global_enabled, daily_tokens_per_user, daily_tokens, updated_at)
         VALUES (1, $1, $2, $3, $4)
         ON CONFLICT (id) DO UPDATE SET global_enabled = $1, daily_tokens_per_user = $2,
             daily_tokens = $3, updated_at = $4",
        global_enabled,
        daily_tokens_per_user,
        daily_tokens,
        now,
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
}

pub(crate) async fn models(conn: &mut PgConnection) -> sqlx::Result<Vec<ModelRow>> {
    sqlx::query_as!(
        ModelRow,
        "SELECT id, key, display_name, description, badge, provider, provider_model, cost_weight,
                vision, enabled, is_default, sort_order
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
    pub now: DateTime<Utc>,
}

/// 按 key 新增或更新，返回 id。
pub(crate) async fn upsert_model(conn: &mut PgConnection, m: &NewModel<'_>) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        "INSERT INTO ai_models (id, key, display_name, description, badge, provider, provider_model,
                                cost_weight, enabled, is_default, sort_order, created_at, updated_at,
                                vision)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12, $13)
         ON CONFLICT (key) DO UPDATE SET display_name = $3, description = $4, badge = $5,
             provider = $6, provider_model = $7, cost_weight = $8, enabled = $9,
             is_default = $10, sort_order = $11, updated_at = $12, vision = $13
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
    pub sort_order: i32,
    pub output_mode: String,
    pub system_prompt: String,
    pub examples: serde_json::Value,
    pub default_model_id: Option<i64>,
    pub allowed_model_ids: Vec<i64>,
    pub suggested_questions: Vec<String>,
    pub story_tools: bool,
    pub mcp_server_ids: Vec<i64>,
}

pub(crate) async fn cards(conn: &mut PgConnection) -> sqlx::Result<Vec<CardRow>> {
    sqlx::query_as!(
        CardRow,
        "SELECT id, slug, display_name, bio, avatar_media_id, bound_user_id, enabled, knock_visible,
                dm_enabled, comment_enabled, sort_order, output_mode, system_prompt, examples,
                default_model_id, allowed_model_ids, suggested_questions, story_tools, mcp_server_ids
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
    pub sort_order: i32,
    pub output_mode: &'a str,
    pub system_prompt: &'a str,
    pub examples: &'a serde_json::Value,
    pub default_model_id: Option<i64>,
    pub allowed_model_ids: &'a [i64],
    pub suggested_questions: &'a [String],
    pub story_tools: bool,
    pub mcp_server_ids: &'a [i64],
    pub now: DateTime<Utc>,
}

/// 按 slug 新增或更新，返回 id。
pub(crate) async fn upsert_card(conn: &mut PgConnection, c: &NewCard<'_>) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        "INSERT INTO ai_role_cards (id, slug, display_name, bio, avatar_media_id, bound_user_id,
                                    enabled, knock_visible, dm_enabled, comment_enabled, sort_order,
                                    output_mode, system_prompt, examples, default_model_id,
                                    allowed_model_ids, suggested_questions, story_tools,
                                    mcp_server_ids, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $19,
                 $20, $18, $18)
         ON CONFLICT (slug) DO UPDATE SET display_name = $3, bio = $4, avatar_media_id = $5,
             bound_user_id = $6, enabled = $7, knock_visible = $8, dm_enabled = $9,
             comment_enabled = $10, sort_order = $11, output_mode = $12, system_prompt = $13,
             examples = $14, default_model_id = $15, allowed_model_ids = $16,
             suggested_questions = $17, story_tools = $19, mcp_server_ids = $20, updated_at = $18
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
        "SELECT conversation_id, user_id, ai_user_id, context_epoch, context_from, model_key
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
        "SELECT conversation_id, user_id, ai_user_id, context_epoch, context_from, model_key
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

/// 开始一次尝试。
pub(crate) async fn start_attempt(
    conn: &mut PgConnection,
    id: i64,
    attempt: i16,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE ai_jobs SET status = 'processing', attempts = $2, updated_at = $3 WHERE id = $1",
        id,
        attempt,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
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
        "SELECT id, kind, status, reason, quota_user_id, ai_user_id, conversation_id, post_id,
                attempts, model_key, prompt_tokens, completion_tokens, created_at, updated_at
         FROM ai_jobs
         WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR kind = $2)
         ORDER BY updated_at DESC, id DESC
         LIMIT $3 OFFSET $4",
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
    pub max_result_chars: i32,
    pub enabled: bool,
    pub sort_order: i32,
    pub last_listed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

pub(crate) async fn mcp_servers(conn: &mut PgConnection) -> sqlx::Result<Vec<McpServerRow>> {
    sqlx::query_as!(
        McpServerRow,
        "SELECT id, name, display_name, url, auth_header, timeout_ms, max_result_chars, enabled,
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
        "SELECT id, name, display_name, url, auth_header, timeout_ms, max_result_chars, enabled,
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
    pub max_result_chars: i32,
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
                                     max_result_chars, enabled, sort_order, created_at, updated_at)
         VALUES ($1, $2, $3, $4, COALESCE($5, ''), $6, $7, $8, $9, $10, $10)
         ON CONFLICT (name) DO UPDATE SET display_name = $3, url = $4,
             auth_header = COALESCE($5, ai_mcp_servers.auth_header), timeout_ms = $6,
             max_result_chars = $7, enabled = $8, sort_order = $9, updated_at = $10
         RETURNING id",
        s.id,
        s.name,
        s.display_name,
        s.url,
        s.auth_header,
        s.timeout_ms,
        s.max_result_chars,
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
