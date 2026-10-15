//! SQL。表都归 ik-ai：`ai_settings`、`ai_models`、`ai_role_cards`、`ai_quota_*`。

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

pub(crate) struct ModelRow {
    pub id: i64,
    pub key: String,
    pub display_name: String,
    pub description: String,
    pub badge: Option<String>,
    pub provider: String,
    pub provider_model: String,
    pub cost_weight: f64,
    pub enabled: bool,
    pub is_default: bool,
    pub sort_order: i32,
}

pub(crate) async fn models(conn: &mut PgConnection) -> sqlx::Result<Vec<ModelRow>> {
    sqlx::query_as!(
        ModelRow,
        "SELECT id, key, display_name, description, badge, provider, provider_model, cost_weight,
                enabled, is_default, sort_order
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
    pub enabled: bool,
    pub is_default: bool,
    pub sort_order: i32,
    pub now: DateTime<Utc>,
}

/// 按 key 新增或更新，返回 id。
pub(crate) async fn upsert_model(conn: &mut PgConnection, m: &NewModel<'_>) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        "INSERT INTO ai_models (id, key, display_name, description, badge, provider, provider_model,
                                cost_weight, enabled, is_default, sort_order, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12)
         ON CONFLICT (key) DO UPDATE SET display_name = $3, description = $4, badge = $5,
             provider = $6, provider_model = $7, cost_weight = $8, enabled = $9,
             is_default = $10, sort_order = $11, updated_at = $12
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
}

pub(crate) async fn cards(conn: &mut PgConnection) -> sqlx::Result<Vec<CardRow>> {
    sqlx::query_as!(
        CardRow,
        "SELECT id, slug, display_name, bio, avatar_media_id, bound_user_id, enabled, knock_visible,
                dm_enabled, comment_enabled, sort_order, output_mode, system_prompt, examples,
                default_model_id, allowed_model_ids, suggested_questions
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
    pub now: DateTime<Utc>,
}

/// 按 slug 新增或更新，返回 id。
pub(crate) async fn upsert_card(conn: &mut PgConnection, c: &NewCard<'_>) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        "INSERT INTO ai_role_cards (id, slug, display_name, bio, avatar_media_id, bound_user_id,
                                    enabled, knock_visible, dm_enabled, comment_enabled, sort_order,
                                    output_mode, system_prompt, examples, default_model_id,
                                    allowed_model_ids, suggested_questions, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $18)
         ON CONFLICT (slug) DO UPDATE SET display_name = $3, bio = $4, avatar_media_id = $5,
             bound_user_id = $6, enabled = $7, knock_visible = $8, dm_enabled = $9,
             comment_enabled = $10, sort_order = $11, output_mode = $12, system_prompt = $13,
             examples = $14, default_model_id = $15, allowed_model_ids = $16,
             suggested_questions = $17, updated_at = $18
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
