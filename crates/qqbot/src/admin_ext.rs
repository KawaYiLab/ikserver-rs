//! 管理后台第二、三期（ADR 0008 §11.3）：QQ 身份与绑定、回复记录、群共享记忆。权限同 AI 后台，写操作记审计。
use axum::extract::{FromRef, State};
use chrono::{DateTime, Utc};
use http::StatusCode;
use ik_ai::shared_memory::{self, SharedMemoryInput, SharedMemoryView};
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
use sqlx::Row;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{QqState, me, repo};

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = QqIdentityView)]
#[serde(rename_all = "camelCase")]
pub struct IdentityView {
    #[schema(value_type = String)]
    pub id: Id,
    pub app_id: String,
    /// `group` / `c2c`。
    pub scene: String,
    #[schema(value_type = Option<String>)]
    pub group_id: Option<Id>,
    pub group_name: Option<String>,
    /// openid 只给后 6 位，用于区分。
    pub openid_tail: String,
    /// 最近一次的群昵称。
    pub nickname: String,
    #[schema(value_type = Option<String>)]
    pub user_id: Option<Id>,
    pub bound_at: Option<DateTime<Utc>>,
    pub blocked: bool,
    /// 单聊身份是否还是机器人的好友（删好友后为 false）。
    pub available: bool,
    pub last_active_at: DateTime<Utc>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct IdentityQuery {
    /// 绳网用户 id 或用户名：只看这个账号绑定的身份。
    pub user: Option<String>,
    /// 群 id：只看这个群里的身份。
    #[param(value_type = Option<String>)]
    pub group: Option<Id>,
    pub page: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[schema(as = QqUpdateIdentity)]
#[serde(rename_all = "camelCase")]
pub struct UpdateIdentity {
    /// 封禁这个 QQ 身份（不再回复）。
    pub blocked: bool,
    /// 强制解绑。
    #[serde(default)]
    pub unbind: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = QqTurnView)]
#[serde(rename_all = "camelCase")]
pub struct TurnView {
    #[schema(value_type = String)]
    pub id: Id,
    pub scene: String,
    #[schema(value_type = Option<String>)]
    pub group_id: Option<Id>,
    #[schema(value_type = String)]
    pub identity_id: Id,
    #[schema(value_type = Option<String>)]
    pub ai_user_id: Option<Id>,
    /// 按谁的个人额度计；游客为空。
    #[schema(value_type = Option<String>)]
    pub payer_user_id: Option<Id>,
    pub status: String,
    pub reason: Option<String>,
    pub model_key: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub segments_sent: i32,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TurnQuery {
    /// queued / processing / completed / failed / skipped。
    pub status: Option<String>,
    #[param(value_type = Option<String>)]
    pub group: Option<Id>,
    pub page: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
#[schema(as = QqDispatchStatus)]
#[serde(rename_all = "camelCase")]
pub struct DispatchStatus {
    pub paused: bool,
    pub queued: i64,
    pub generating: i64,
    pub delivering: i64,
    pub active_leases: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = QqTurnDetail)]
#[serde(rename_all = "camelCase")]
pub struct TurnDetail {
    pub turn: TurnView,
    pub phase: String,
    pub conversation_seq: Option<i64>,
    pub platform_deadline_at: Option<DateTime<Utc>>,
    pub generation_deadline_at: Option<DateTime<Utc>>,
    pub delivery_deadline_at: Option<DateTime<Utc>>,
    pub cost_finalized_at: Option<DateTime<Utc>>,
    pub quota_finalized_at: Option<DateTime<Utc>>,
    pub segments: Vec<SegmentView>,
    pub attempts: Vec<AttemptView>,
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
#[schema(as = QqSegmentView)]
#[serde(rename_all = "camelCase")]
pub struct SegmentView {
    pub msg_seq: i32,
    pub status: String,
    pub payload_version: i32,
    pub payload_hash: String,
    pub attempt_count: i32,
    pub retry_at: Option<DateTime<Utc>>,
    pub auth_corrected: bool,
    pub url_corrected: bool,
    pub accepted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
#[schema(as = QqAttemptView)]
#[serde(rename_all = "camelCase")]
pub struct AttemptView {
    pub msg_seq: i32,
    pub attempt_no: i32,
    pub payload_hash: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub result: String,
    pub endpoint: Option<String>,
    pub http_status: Option<i32>,
    pub code: Option<i64>,
    pub err_code: Option<i64>,
    pub trace_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = QqGroupMemories)]
pub struct GroupMemories {
    pub items: Vec<SharedMemoryView>,
}

pub(crate) fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    QqState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(identities))
        .routes(routes!(put_identity))
        .routes(routes!(turns))
        .routes(routes!(turn_detail))
        .routes(routes!(dispatch_status))
        .routes(routes!(group_memories, create_group_memory))
        .routes(routes!(update_group_memory, delete_group_memory))
}

/// QQ 身份与绑定
#[utoipa::path(get, operation_id = "qq_identities", path = "/v1/admin/qq/identities", tag = "admin", params(IdentityQuery),
    responses((status = 200, body = Paged<IdentityView>)))]
async fn identities(
    State(s): State<QqState>,
    AdminOperator(_): AdminOperator,
    Query(q): Query<IdentityQuery>,
) -> Result<Json<Paged<IdentityView>>, Problem> {
    let user_id = match q.user.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        Some(query) => Some(ik_ai::memory_admin::find_user(&s.ai, query).await?.0),
        None => None,
    };
    let group_id = q.group.map(|g| g.get());
    let (page, limit, offset) = page_window(q.page, q.limit, 20);
    let mut conn = s.db.acquire().await?;
    let total = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM qq_identities
           WHERE ($1::bigint IS NULL OR user_id = $1) AND ($2::bigint IS NULL OR group_id = $2)"#,
        user_id,
        group_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let rows = sqlx::query!(
        r#"SELECT i.id, i.app_id, i.scene, i.group_id, i.openid, i.nickname, i.user_id, i.bound_at, i.blocked,
                  i.available, i.last_active_at, g.display_name AS "display_name?", g.group_openid AS "group_openid?"
           FROM qq_identities i LEFT JOIN qq_groups g ON g.id = i.group_id
           WHERE ($1::bigint IS NULL OR i.user_id = $1) AND ($2::bigint IS NULL OR i.group_id = $2)
           ORDER BY i.last_active_at DESC, i.id DESC LIMIT $3 OFFSET $4"#,
        user_id,
        group_id,
        i64::from(limit),
        offset
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(Json(Paged {
        items: rows
            .into_iter()
            .map(|r| IdentityView {
                id: Id::new(r.id),
                app_id: r.app_id,
                scene: r.scene,
                group_id: r.group_id.map(Id::new),
                group_name: r
                    .group_openid
                    .map(|openid| repo::group_name(r.display_name.unwrap_or_default(), &openid)),
                openid_tail: {
                    let tail: Vec<char> = r.openid.chars().rev().take(6).collect();
                    tail.into_iter().rev().collect()
                },
                nickname: r.nickname,
                user_id: r.user_id.map(Id::new),
                bound_at: r.bound_at,
                blocked: r.blocked,
                available: r.available,
                last_active_at: r.last_active_at,
            })
            .collect(),
        total,
        page,
        limit,
    }))
}

/// 封禁 / 解封 QQ 身份、强制解绑
#[utoipa::path(put, operation_id = "qq_put_identity", path = "/v1/admin/qq/identities/{id}", tag = "admin", params(("id" = String, Path)),
    request_body = UpdateIdentity, responses((status = 204), (status = 404)))]
async fn put_identity(
    State(s): State<QqState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
    Json(input): Json<UpdateIdentity>,
) -> Result<StatusCode, Problem> {
    let mut tx = s.db.begin().await?;
    let changed = sqlx::query!(
        "UPDATE qq_identities SET blocked=$2 WHERE id=$1",
        id.get(),
        input.blocked
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(Problem::not_found());
    }
    if input.unbind {
        me::release(&mut tx, id.get(), None).await?;
    }
    record(
        &mut tx,
        &s.ids,
        &op,
        Audit {
            action: "qq.identity.update",
            target_type: "qq_identity",
            target_id: Some(id.to_string()),
            details: serde_json::json!(input),
        },
        Utc::now(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// QQ 回复记录（排障：状态、原因、模型、用量；不含消息正文）
#[utoipa::path(get, operation_id = "qq_turns", path = "/v1/admin/qq/turns", tag = "admin", params(TurnQuery),
    responses((status = 200, body = Paged<TurnView>)))]
async fn turns(
    State(s): State<QqState>,
    AdminOperator(_): AdminOperator,
    Query(q): Query<TurnQuery>,
) -> Result<Json<Paged<TurnView>>, Problem> {
    let (page, limit, offset) = page_window(q.page, q.limit, 20);
    let group_id = q.group.map(|g| g.get());
    let status = q.status.as_deref().filter(|s| !s.is_empty());
    let mut conn = s.db.acquire().await?;
    let total = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM qq_turns
           WHERE ($1::text IS NULL OR status = $1) AND ($2::bigint IS NULL OR group_id = $2)"#,
        status,
        group_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let rows = sqlx::query!(
        "SELECT t.id, i.scene, t.group_id, t.identity_id, t.ai_user_id, t.payer_user_id, t.status, t.reason,
                t.model_key, t.prompt_tokens, t.completion_tokens, t.segments_sent, t.created_at, t.finished_at
         FROM qq_turns t JOIN qq_identities i ON i.id = t.identity_id
         WHERE ($1::text IS NULL OR t.status = $1) AND ($2::bigint IS NULL OR t.group_id = $2)
         ORDER BY t.created_at DESC, t.id DESC LIMIT $3 OFFSET $4",
        status,
        group_id,
        i64::from(limit),
        offset
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(Json(Paged {
        items: rows
            .into_iter()
            .map(|r| TurnView {
                id: Id::new(r.id),
                scene: r.scene,
                group_id: r.group_id.map(Id::new),
                identity_id: Id::new(r.identity_id),
                ai_user_id: r.ai_user_id.map(Id::new),
                payer_user_id: r.payer_user_id.map(Id::new),
                status: r.status,
                reason: r.reason,
                model_key: r.model_key,
                prompt_tokens: r.prompt_tokens,
                completion_tokens: r.completion_tokens,
                segments_sent: r.segments_sent,
                created_at: r.created_at,
                finished_at: r.finished_at,
            })
            .collect(),
        total,
        page,
        limit,
    }))
}

/// 调度暂停和排空进度。只读，不包含消息正文或平台身份。
#[utoipa::path(get, operation_id = "qq_dispatch_status", path = "/v1/admin/qq/dispatch", tag = "admin",
    responses((status = 200, body = DispatchStatus)))]
async fn dispatch_status(
    State(s): State<QqState>,
    AdminOperator(_): AdminOperator,
) -> Result<Json<DispatchStatus>, Problem> {
    let status = sqlx::query_as::<_, DispatchStatus>("SELECT dispatch_paused AS paused,
        (SELECT count(*) FROM qq_turns WHERE status='queued') AS queued,
        (SELECT count(*) FROM qq_turns WHERE phase='generating' AND status='processing') AS generating,
        (SELECT count(*) FROM qq_turns WHERE phase='delivering' AND status='processing') AS delivering,
        (SELECT count(*) FROM qq_conversations WHERE lease_until>clock_timestamp()) AS active_leases
        FROM qq_settings WHERE singleton")
        .fetch_one(&s.db).await?;
    Ok(Json(status))
}

/// 一轮回复的截止时间、分段状态与每次发送结果。正文、token、openid 不返回。
#[utoipa::path(get, operation_id = "qq_turn_detail", path = "/v1/admin/qq/turns/{id}", tag = "admin", params(("id" = String, Path)),
    responses((status = 200, body = TurnDetail), (status = 404)))]
async fn turn_detail(
    State(s): State<QqState>,
    AdminOperator(_): AdminOperator,
    Path(id): Path<Id>,
) -> Result<Json<TurnDetail>, Problem> {
    // A single consistent snapshot prevents a detail page combining old segment
    // status with newer attempts while the sender commits its result.
    let mut tx = s.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    let r = sqlx::query("SELECT t.id,i.scene,t.group_id,t.identity_id,t.ai_user_id,t.payer_user_id,t.status,t.reason,
        t.model_key,t.prompt_tokens,t.completion_tokens,t.segments_sent,t.created_at,t.finished_at,
        t.phase,t.conversation_seq,t.platform_deadline_at,t.generation_deadline_at,t.delivery_deadline_at,t.cost_finalized_at,t.quota_finalized_at
        FROM qq_turns t JOIN qq_identities i ON i.id=t.identity_id WHERE t.id=$1")
        .bind(id.get()).fetch_optional(&mut *tx).await?.ok_or_else(Problem::not_found)?;
    let segments = sqlx::query_as::<_, SegmentView>("SELECT msg_seq,status,payload_version,encode(payload_hash,'hex') AS payload_hash,
        attempt_count,retry_at,auth_corrected,url_corrected,accepted_at FROM qq_reply_segments WHERE turn_id=$1 ORDER BY msg_seq")
        .bind(id.get()).fetch_all(&mut *tx).await?;
    let attempts = sqlx::query_as::<_, AttemptView>(
        "SELECT msg_seq,attempt_no,encode(payload_hash,'hex') AS payload_hash,
        started_at,finished_at,result,endpoint,http_status,code,err_code,trace_id
        FROM qq_send_attempts WHERE turn_id=$1 ORDER BY msg_seq,attempt_no",
    )
    .bind(id.get())
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(TurnDetail {
        turn: TurnView {
            id,
            scene: r.try_get("scene")?,
            group_id: r.try_get::<Option<i64>, _>("group_id")?.map(Id::new),
            identity_id: Id::new(r.try_get("identity_id")?),
            ai_user_id: r.try_get::<Option<i64>, _>("ai_user_id")?.map(Id::new),
            payer_user_id: r.try_get::<Option<i64>, _>("payer_user_id")?.map(Id::new),
            status: r.try_get("status")?,
            reason: r.try_get("reason")?,
            model_key: r.try_get("model_key")?,
            prompt_tokens: r.try_get("prompt_tokens")?,
            completion_tokens: r.try_get("completion_tokens")?,
            segments_sent: r.try_get("segments_sent")?,
            created_at: r.try_get("created_at")?,
            finished_at: r.try_get("finished_at")?,
        },
        phase: r.try_get("phase")?,
        conversation_seq: r.try_get("conversation_seq")?,
        platform_deadline_at: r.try_get("platform_deadline_at")?,
        generation_deadline_at: r.try_get("generation_deadline_at")?,
        delivery_deadline_at: r.try_get("delivery_deadline_at")?,
        cost_finalized_at: r.try_get("cost_finalized_at")?,
        quota_finalized_at: r.try_get("quota_finalized_at")?,
        segments,
        attempts,
    }))
}

async fn group_exists(s: &QqState, id: i64) -> Result<(), Problem> {
    let found = sqlx::query_scalar!("SELECT id FROM qq_groups WHERE id=$1", id)
        .fetch_optional(&s.db)
        .await?;
    found.map(|_| ()).ok_or_else(Problem::not_found)
}

/// 群共享记忆
#[utoipa::path(get, operation_id = "qq_group_memories", path = "/v1/admin/qq/groups/{id}/memories", tag = "admin", params(("id" = String, Path)),
    responses((status = 200, body = GroupMemories), (status = 404)))]
async fn group_memories(
    State(s): State<QqState>,
    AdminOperator(_): AdminOperator,
    Path(id): Path<Id>,
) -> Result<Json<GroupMemories>, Problem> {
    group_exists(&s, id.get()).await?;
    Ok(Json(GroupMemories {
        items: shared_memory::list(&s.ai, &repo::group_scope(id.get())).await?,
    }))
}

async fn audit_memory(
    s: &QqState,
    tx: &mut sqlx::PgConnection,
    op: &ik_auth::admin::Operator,
    action: &'static str,
    group: Id,
    details: serde_json::Value,
) -> Result<(), Problem> {
    record(
        tx,
        &s.ids,
        op,
        Audit {
            action,
            target_type: "qq_group",
            target_id: Some(group.to_string()),
            details,
        },
        Utc::now(),
    )
    .await
}

/// 新建一条群共享记忆
#[utoipa::path(post, operation_id = "qq_create_group_memory", path = "/v1/admin/qq/groups/{id}/memories", tag = "admin", params(("id" = String, Path)),
    request_body = SharedMemoryInput, responses((status = 201, body = SharedMemoryView), (status = 404)))]
async fn create_group_memory(
    State(s): State<QqState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
    Json(input): Json<SharedMemoryInput>,
) -> Result<(StatusCode, Json<SharedMemoryView>), Problem> {
    group_exists(&s, id.get()).await?;
    let mut tx = s.db.begin().await?;
    let view =
        shared_memory::save(&mut tx, &s.ai, &repo::group_scope(id.get()), None, &input).await?;
    audit_memory(
        &s,
        &mut tx,
        &op,
        "qq.group_memory.create",
        id,
        serde_json::json!({ "memoryId": view.id.to_string(), "content": view.content }),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// 修改一条群共享记忆（正文、置顶）
#[utoipa::path(put, operation_id = "qq_update_group_memory", path = "/v1/admin/qq/groups/{id}/memories/{memoryId}", tag = "admin",
    params(("id" = String, Path), ("memoryId" = String, Path)),
    request_body = SharedMemoryInput, responses((status = 200, body = SharedMemoryView), (status = 404)))]
async fn update_group_memory(
    State(s): State<QqState>,
    AdminOperator(op): AdminOperator,
    Path((id, memory_id)): Path<(Id, Id)>,
    Json(input): Json<SharedMemoryInput>,
) -> Result<Json<SharedMemoryView>, Problem> {
    let mut tx = s.db.begin().await?;
    let view = shared_memory::save(
        &mut tx,
        &s.ai,
        &repo::group_scope(id.get()),
        Some(memory_id.get()),
        &input,
    )
    .await?;
    audit_memory(
        &s,
        &mut tx,
        &op,
        "qq.group_memory.update",
        id,
        serde_json::json!({ "memoryId": memory_id.to_string(), "content": input.content, "pinned": input.pinned }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(view))
}

/// 删除一条群共享记忆
#[utoipa::path(delete, operation_id = "qq_delete_group_memory", path = "/v1/admin/qq/groups/{id}/memories/{memoryId}", tag = "admin",
    params(("id" = String, Path), ("memoryId" = String, Path)), responses((status = 204), (status = 404)))]
async fn delete_group_memory(
    State(s): State<QqState>,
    AdminOperator(op): AdminOperator,
    Path((id, memory_id)): Path<(Id, Id)>,
) -> Result<StatusCode, Problem> {
    let mut tx = s.db.begin().await?;
    let content =
        shared_memory::delete(&mut tx, &repo::group_scope(id.get()), memory_id.get()).await?;
    audit_memory(
        &s,
        &mut tx,
        &op,
        "qq.group_memory.delete",
        id,
        serde_json::json!({ "memoryId": memory_id.to_string(), "content": content }),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
