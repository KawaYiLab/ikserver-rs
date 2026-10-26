use crate::{QqState, message::MessageFormat, repo, text};
use axum::extract::{FromRef, State};
use chrono::{DateTime, Utc};
use ik_auth::{
    AuthState,
    admin::{AdminOperator, Audit, record},
};
use ik_kernel::{
    Id, Problem,
    cursor::{Paged, page_window},
    extract::{Json, Path, Query},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
#[schema(as = QqSettings)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub enabled: bool,
    /// 暂停领取新的生成和发送工作；回调继续入库，原有截止时间不延长。
    #[serde(default)]
    pub dispatch_paused: bool,
    pub guest_daily_turns: i32,
    pub guest_group_daily_turns: i32,
    pub guest_daily_tokens: i64,
    pub guest_model_key: Option<String>,
    pub group_context_turns: i32,
    pub group_context_hours: i32,
    pub c2c_context_turns: i32,
    pub reply_timeout_secs: i32,
    pub max_reply_chars: i32,
    pub reply_format: MessageFormat,
    pub link_mode: String,
    pub blocked_words: Vec<String>,
    pub guest_reads_group_memory: bool,
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}
impl Settings {
    fn validate(&self) -> Result<(), Problem> {
        for (field, valid) in [
            (
                "guestDailyTurns",
                (0..=1000).contains(&self.guest_daily_turns),
            ),
            (
                "guestGroupDailyTurns",
                (0..=100000).contains(&self.guest_group_daily_turns),
            ),
            ("guestDailyTokens", self.guest_daily_tokens >= 0),
            (
                "groupContextTurns",
                (1..=30).contains(&self.group_context_turns),
            ),
            (
                "groupContextHours",
                (1..=24).contains(&self.group_context_hours),
            ),
            (
                "c2cContextTurns",
                (1..=60).contains(&self.c2c_context_turns),
            ),
            (
                "replyTimeoutSecs",
                (10..=180).contains(&self.reply_timeout_secs),
            ),
            ("maxReplyChars", (1..=4000).contains(&self.max_reply_chars)),
            (
                "linkMode",
                matches!(self.link_mode.as_str(), "strip" | "domain" | "allow"),
            ),
            (
                "blockedWords",
                self.blocked_words.len() <= 500
                    && self
                        .blocked_words
                        .iter()
                        .all(|w| !w.trim().is_empty() && w.chars().count() <= 100),
            ),
        ] {
            if !valid {
                return Err(Problem::field(field, "INVALID", "超出允许范围"));
            }
        }
        Ok(())
    }
}
/// Older admin clients omit replyFormat; preserve the stored choice in the update transaction.
#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = QqUpdateSettings)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettings {
    pub enabled: bool,
    #[serde(default)]
    pub dispatch_paused: bool,
    pub guest_daily_turns: i32,
    pub guest_group_daily_turns: i32,
    pub guest_daily_tokens: i64,
    pub guest_model_key: Option<String>,
    pub group_context_turns: i32,
    pub group_context_hours: i32,
    pub c2c_context_turns: i32,
    pub reply_timeout_secs: i32,
    pub max_reply_chars: i32,
    pub reply_format: Option<MessageFormat>,
    pub link_mode: String,
    pub blocked_words: Vec<String>,
    pub guest_reads_group_memory: bool,
}

impl UpdateSettings {
    fn resolve(self, current_format: MessageFormat) -> Settings {
        Settings {
            enabled: self.enabled,
            dispatch_paused: self.dispatch_paused,
            guest_daily_turns: self.guest_daily_turns,
            guest_group_daily_turns: self.guest_group_daily_turns,
            guest_daily_tokens: self.guest_daily_tokens,
            guest_model_key: self.guest_model_key,
            group_context_turns: self.group_context_turns,
            group_context_hours: self.group_context_hours,
            c2c_context_turns: self.c2c_context_turns,
            reply_timeout_secs: self.reply_timeout_secs,
            max_reply_chars: self.max_reply_chars,
            reply_format: self.reply_format.unwrap_or(current_format),
            link_mode: self.link_mode,
            blocked_words: self.blocked_words,
            guest_reads_group_memory: self.guest_reads_group_memory,
            updated_at: Utc::now(),
        }
    }
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = QqBotView)]
#[serde(rename_all = "camelCase")]
pub struct BotView {
    pub app_id: String,
    pub display_name: String,
    pub default_card_slug: Option<String>,
    pub enabled: bool,
}
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[schema(as = QqUpdateBot)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBot {
    pub display_name: String,
    pub default_card_slug: Option<String>,
    pub enabled: bool,
}
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = QqBotList)]
pub struct BotList {
    pub items: Vec<BotView>,
}
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = QqGroupView)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    #[schema(value_type=String)]
    pub id: Id,
    pub app_id: String,
    pub display_name: String,
    pub card_slug: Option<String>,
    pub enabled: bool,
    pub blocked: bool,
    pub left_at: Option<DateTime<Utc>>,
    pub last_active_at: DateTime<Utc>,
    pub today_guest_turns: i32,
    pub today_tokens: i64,
}
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[schema(as = QqUpdateGroup)]
#[serde(rename_all = "camelCase")]
pub struct UpdateGroup {
    pub display_name: String,
    pub card_slug: Option<String>,
    pub enabled: bool,
    pub blocked: bool,
    #[serde(default)]
    pub reset_context: bool,
}
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in=Query)]
pub struct GroupQuery {
    pub page: Option<u32>,
    pub limit: Option<u32>,
}

pub(crate) fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    QqState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(settings, put_settings))
        .routes(routes!(bots))
        .routes(routes!(bot, put_bot))
        .routes(routes!(groups))
        .routes(routes!(put_group))
}
#[utoipa::path(get, operation_id = "qq_settings",path="/v1/admin/qq/settings",tag="admin",responses((status=200,body=Settings)))]
async fn settings(
    State(s): State<QqState>,
    AdminOperator(_): AdminOperator,
) -> Result<Json<Settings>, Problem> {
    let mut conn = s.db.acquire().await?;
    Ok(Json(repo::settings(&mut conn).await?))
}
#[utoipa::path(put, operation_id = "qq_put_settings",path="/v1/admin/qq/settings",tag="admin",request_body=UpdateSettings,responses((status=200,body=Settings)))]
async fn put_settings(
    State(s): State<QqState>,
    AdminOperator(op): AdminOperator,
    Json(input): Json<UpdateSettings>,
) -> Result<Json<Settings>, Problem> {
    if let Some(key) = input.guest_model_key.as_deref() {
        let catalog = s.ai.catalog.get().await?;
        if !catalog.models.iter().any(|m| m.key == key && m.enabled) {
            return Err(Problem::field(
                "guestModelKey",
                "INVALID",
                "请选择启用的模型",
            ));
        }
    }
    let mut tx = s.db.begin().await?;
    let format = sqlx::query_scalar::<_, MessageFormat>(
        "SELECT reply_format FROM qq_settings WHERE singleton FOR UPDATE",
    )
    .fetch_one(&mut *tx)
    .await?;
    let input = input.resolve(format);
    input.validate()?;
    repo::save_settings(&mut tx, &input).await?;
    if !input.dispatch_paused {
        s.jobs
            .enqueue(
                &mut tx,
                crate::jobs::Reconcile::default(),
                Default::default(),
            )
            .await?;
    }
    record(
        &mut tx,
        &s.ids,
        &op,
        Audit {
            action: "qq.settings.update",
            target_type: "qq_settings",
            target_id: None,
            details: serde_json::json!(input),
        },
        input.updated_at,
    )
    .await?;
    tx.commit().await?;
    s.invalidate().await;
    Ok(Json(input))
}
#[utoipa::path(get, operation_id = "qq_bots",path="/v1/admin/qq/bots",tag="admin",responses((status=200,body=BotList)))]
async fn bots(
    State(s): State<QqState>,
    AdminOperator(_): AdminOperator,
) -> Result<Json<BotList>, Problem> {
    let mut conn = s.db.acquire().await?;
    let mut items = repo::bots(&mut conn).await?;
    items.retain(|b| s.config.apps.values().any(|a| a.app_id == b.app_id));
    Ok(Json(BotList { items }))
}
#[utoipa::path(get, operation_id = "qq_bot",path="/v1/admin/qq/bots/{appId}",tag="admin",params(("appId"=String,Path)),responses((status=200,body=BotView)))]
async fn bot(
    State(s): State<QqState>,
    AdminOperator(_): AdminOperator,
    Path(app_id): Path<String>,
) -> Result<Json<BotView>, Problem> {
    configured(&s, &app_id)?;
    let mut conn = s.db.acquire().await?;
    Ok(Json(
        repo::bots(&mut conn)
            .await?
            .into_iter()
            .find(|b| b.app_id == app_id)
            .ok_or_else(Problem::not_found)?,
    ))
}
fn configured(s: &QqState, app_id: &str) -> Result<(), Problem> {
    if !s.config.apps.values().any(|a| a.app_id == app_id) {
        return Err(Problem::not_found());
    }
    Ok(())
}
async fn card(s: &QqState, slug: Option<&str>) -> Result<(), Problem> {
    if let Some(slug) = slug {
        let catalog = s.ai.catalog.get().await?;
        if !catalog
            .cards
            .iter()
            .any(|c| c.slug == slug && c.enabled && c.qq_enabled)
        {
            return Err(Problem::field(
                "cardSlug",
                "INVALID",
                "请选择启用 QQ 的角色",
            ));
        }
    }
    Ok(())
}
#[utoipa::path(put, operation_id = "qq_put_bot",path="/v1/admin/qq/bots/{appId}",tag="admin",params(("appId"=String,Path)),request_body=UpdateBot,responses((status=200,body=BotView)))]
async fn put_bot(
    State(s): State<QqState>,
    AdminOperator(op): AdminOperator,
    Path(app_id): Path<String>,
    Json(input): Json<UpdateBot>,
) -> Result<Json<BotView>, Problem> {
    configured(&s, &app_id)?;
    card(&s, input.default_card_slug.as_deref()).await?;
    if input.display_name.chars().count() > 64 {
        return Err(Problem::field("displayName", "TOO_LONG", "最多 64 字"));
    }
    let mut tx = s.db.begin().await?;
    sqlx::query!(
        "UPDATE qq_bots SET display_name=$2,default_card_slug=$3,enabled=$4 WHERE app_id=$1",
        app_id,
        input.display_name,
        input.default_card_slug,
        input.enabled
    )
    .execute(&mut *tx)
    .await?;
    record(
        &mut tx,
        &s.ids,
        &op,
        Audit {
            action: "qq.bot.update",
            target_type: "qq_bot",
            target_id: Some(app_id.clone()),
            details: serde_json::json!(input),
        },
        Utc::now(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(BotView {
        app_id,
        display_name: input.display_name,
        default_card_slug: input.default_card_slug,
        enabled: input.enabled,
    }))
}
#[utoipa::path(get, operation_id = "qq_groups",path="/v1/admin/qq/groups",tag="admin",params(GroupQuery),responses((status=200,body=Paged<GroupView>)))]
async fn groups(
    State(s): State<QqState>,
    AdminOperator(_): AdminOperator,
    Query(q): Query<GroupQuery>,
) -> Result<Json<Paged<GroupView>>, Problem> {
    let (page, limit, offset) = page_window(q.page, q.limit, 20);
    let mut conn = s.db.acquire().await?;
    let total = sqlx::query_scalar!("SELECT count(*) FROM qq_groups")
        .fetch_one(&mut *conn)
        .await?
        .unwrap_or(0);
    Ok(Json(Paged {
        items: repo::groups(&mut conn, None, i64::from(limit), offset).await?,
        total,
        page,
        limit,
    }))
}
#[utoipa::path(put, operation_id = "qq_put_group",path="/v1/admin/qq/groups/{id}",tag="admin",params(("id"=String,Path)),request_body=UpdateGroup,responses((status=200,body=GroupView)))]
async fn put_group(
    State(s): State<QqState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
    Json(input): Json<UpdateGroup>,
) -> Result<Json<GroupView>, Problem> {
    card(&s, input.card_slug.as_deref()).await?;
    if input.display_name.chars().count() > 64 {
        return Err(Problem::field("displayName", "TOO_LONG", "最多 64 字"));
    }
    let mut tx = s.db.begin().await?;
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut *tx)
        .await?;
    let display_name = text::truncate(&input.display_name, 64);
    let changed=sqlx::query!("UPDATE qq_groups SET display_name=$2,card_slug=$3,enabled=$4,blocked=$5,context_reset_at=CASE WHEN $6 THEN $7 ELSE context_reset_at END WHERE id=$1",id.get(),display_name,input.card_slug,input.enabled,input.blocked,input.reset_context,now).execute(&mut *tx).await?;
    if changed.rows_affected() == 0 {
        return Err(Problem::not_found());
    }
    record(
        &mut tx,
        &s.ids,
        &op,
        Audit {
            action: "qq.group.update",
            target_type: "qq_group",
            target_id: Some(id.to_string()),
            details: serde_json::json!(input),
        },
        now,
    )
    .await?;
    let row = repo::groups(&mut tx, Some(id.get()), 1, 0)
        .await?
        .into_iter()
        .next()
        .ok_or_else(Problem::not_found)?;
    tx.commit().await?;
    Ok(Json(row))
}
