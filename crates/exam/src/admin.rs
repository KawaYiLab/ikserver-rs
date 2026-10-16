//! 入站题库后台（moderation-exam.md 第 4 节 EXAMADM、media-ops.md ADMINCRUD-001～003）。
//!
//! 查看：版主或管理员；新增、编辑、删除、启停：只有管理员。写操作与审计日志同一事务。
//! 正确答案与解析只在这里下发（ADMINCRUD-002），考生接口永不返回。

use axum::extract::{FromRef, State};
use chrono::{DateTime, Utc};
use ik_auth::admin::{self, Audit};
use ik_auth::{AdminOperator, AuthState, StaffOperator};
use ik_kernel::cursor::{Paged, page_window};
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::{Id, Problem};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::ExamState;
use crate::dto::QuestionInput;
use crate::paper::{ExamOption, QuestionKind};
use crate::repo::{self, AdminQuestionRow, QuestionFilter};
use crate::service::validate_question;

fn question_not_found() -> Problem {
    Problem::new(http::StatusCode::NOT_FOUND, "NOT_FOUND", "题目不存在")
}

/// 后台看到的完整题目（含答案与解析）。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminQuestion {
    #[schema(value_type = String)]
    pub id: Id,
    pub question: String,
    pub kind: QuestionKind,
    pub options: Vec<ExamOption>,
    pub correct_keys: Vec<String>,
    pub explanation: Option<String>,
    pub weight: i32,
    pub enabled: bool,
    pub tag: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn view(row: AdminQuestionRow) -> AdminQuestion {
    AdminQuestion {
        id: Id::new(row.id),
        question: row.question,
        kind: QuestionKind::parse(&row.kind).unwrap_or(QuestionKind::Single),
        options: serde_json::from_value(row.options).unwrap_or_default(),
        correct_keys: row.correct_keys,
        explanation: row.explanation,
        weight: row.weight,
        enabled: row.enabled,
        tag: row.tag,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct QuestionQuery {
    /// `single` | `multiple` | `boolean`；其他值忽略。
    #[serde(rename = "type")]
    #[param(rename = "type")]
    pub kind: Option<String>,
    /// `true` | `false`。
    pub enabled: Option<bool>,
    /// 题干或标签包含（不区分大小写）。
    pub q: Option<String>,
    pub page: Option<u32>,
    /// 默认 20，最大 100。
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuestionStats {
    pub total: i64,
    pub enabled: i64,
    pub single: i64,
    pub multiple: i64,
    pub boolean: i64,
    /// 每场考试抽多少题；已启用题数低于它时考试不可用，后台应预警（ADMINCRUD-001）。
    pub question_count: u32,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BulkDeleteRequest {
    #[schema(value_type = Vec<String>)]
    pub ids: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkDeleteResult {
    pub deleted_count: u64,
}

/// ILIKE 包含模式：转义通配符。
fn contains_pattern(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 2);
    out.push('%');
    for c in q.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

pub async fn list(
    state: &ExamState,
    query: QuestionQuery,
) -> Result<Paged<AdminQuestion>, Problem> {
    let (page, limit, offset) = page_window(query.page, query.limit, 20);
    let filter = QuestionFilter {
        kind: query
            .kind
            .as_deref()
            .and_then(QuestionKind::parse)
            .map(|k| k.as_str().to_owned()),
        enabled: query.enabled,
        pattern: query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(contains_pattern),
    };
    let mut conn = state.db.acquire().await?;
    let total = repo::count_questions(&mut conn, &filter).await?;
    let rows = repo::list_questions(&mut conn, &filter, i64::from(limit), offset).await?;
    Ok(Paged {
        items: rows.into_iter().map(view).collect(),
        total,
        page,
        limit,
    })
}

pub async fn stats(state: &ExamState) -> Result<QuestionStats, Problem> {
    let mut conn = state.db.acquire().await?;
    let row = repo::question_stats(&mut conn).await?;
    Ok(QuestionStats {
        total: row.total,
        enabled: row.enabled,
        single: row.single,
        multiple: row.multiple,
        boolean: row.boolean,
        question_count: state.settings.question_count,
    })
}

pub async fn get(state: &ExamState, id: i64) -> Result<AdminQuestion, Problem> {
    let mut conn = state.db.acquire().await?;
    repo::admin_question(&mut conn, id)
        .await?
        .map(view)
        .ok_or_else(question_not_found)
}

fn audit_details(input: &QuestionInput) -> serde_json::Value {
    serde_json::json!({
        "question": input.question.chars().take(80).collect::<String>(),
        "kind": input.kind.as_str(),
        "enabled": input.enabled,
    })
}

pub async fn create(
    state: &ExamState,
    op: &ik_auth::Operator,
    input: QuestionInput,
) -> Result<AdminQuestion, Problem> {
    let valid = validate_question(&input)?;
    let id = state.ids.next_raw();
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    repo::insert_question(&mut tx, &valid.row(id, now)).await?;
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "exam.question.create",
            target_type: "exam_question",
            target_id: Some(id.to_string()),
            details: audit_details(&input),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    get(state, id).await
}

/// 整体覆盖（EXAMADM-004）。
pub async fn update(
    state: &ExamState,
    op: &ik_auth::Operator,
    id: i64,
    input: QuestionInput,
) -> Result<AdminQuestion, Problem> {
    let valid = validate_question(&input)?;
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    if !repo::update_question(&mut tx, &valid.row(id, now)).await? {
        return Err(question_not_found());
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "exam.question.update",
            target_type: "exam_question",
            target_id: Some(id.to_string()),
            details: audit_details(&input),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    get(state, id).await
}

/// 启用 / 停用取反（EXAMADM-006）。
pub async fn toggle(
    state: &ExamState,
    op: &ik_auth::Operator,
    id: i64,
) -> Result<AdminQuestion, Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let Some(enabled) = repo::toggle_question(&mut tx, id, now).await? else {
        return Err(question_not_found());
    };
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "exam.question.toggle",
            target_type: "exam_question",
            target_id: Some(id.to_string()),
            details: serde_json::json!({ "enabled": enabled }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    get(state, id).await
}

/// 批量物理删除（EXAMADM-007）。与旧系统不同，返回实际删除的条数（不存在的 id 不计）。
pub async fn bulk_delete(
    state: &ExamState,
    op: &ik_auth::Operator,
    request: BulkDeleteRequest,
) -> Result<BulkDeleteResult, Problem> {
    let mut ids: Vec<i64> = Vec::with_capacity(request.ids.len());
    for raw in request
        .ids
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        let id = raw
            .parse::<i64>()
            .map_err(|_| Problem::field("ids", "INVALID", "不是合法的题目 id"))?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return Err(Problem::field("ids", "REQUIRED", "缺少要删除的题目"));
    }
    if ids.len() > 500 {
        return Err(Problem::field("ids", "TOO_MANY", "一次最多删除 500 道题"));
    }
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let deleted = repo::delete_questions(&mut tx, &ids).await?;
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "exam.question.delete",
            target_type: "exam_question",
            target_id: None,
            details: serde_json::json!({
                "ids": ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "deleted": deleted,
            }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(BulkDeleteResult {
        deleted_count: deleted,
    })
}

// ---------- HTTP ----------

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    ExamState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(list_questions, create_question))
        .routes(routes!(question_stats))
        .routes(routes!(get_question, update_question))
        .routes(routes!(toggle_question))
        .routes(routes!(bulk_delete_questions))
}

/// 题库列表
///
/// 筛选题型、启用状态、关键词（题干或标签），按更新时间倒序，页码分页。
#[utoipa::path(
    get,
    path = "/v1/admin/exam/questions",
    tag = "admin",
    params(QuestionQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = Paged<AdminQuestion>),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn list_questions(
    State(state): State<ExamState>,
    StaffOperator(_): StaffOperator,
    Query(query): Query<QuestionQuery>,
) -> Result<Json<Paged<AdminQuestion>>, Problem> {
    Ok(Json(list(&state, query).await?))
}

/// 新建题目（只有管理员）
#[utoipa::path(
    post,
    path = "/v1/admin/exam/questions",
    tag = "admin",
    request_body = QuestionInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = AdminQuestion),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn create_question(
    State(state): State<ExamState>,
    AdminOperator(op): AdminOperator,
    Json(input): Json<QuestionInput>,
) -> Result<(http::StatusCode, Json<AdminQuestion>), Problem> {
    Ok((
        http::StatusCode::CREATED,
        Json(create(&state, &op, input).await?),
    ))
}

/// 题库统计
#[utoipa::path(
    get,
    path = "/v1/admin/exam/questions/stats",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = QuestionStats),
        (status = 403, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn question_stats(
    State(state): State<ExamState>,
    StaffOperator(_): StaffOperator,
) -> Result<Json<QuestionStats>, Problem> {
    Ok(Json(stats(&state).await?))
}

/// 题目详情
#[utoipa::path(
    get,
    path = "/v1/admin/exam/questions/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminQuestion),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn get_question(
    State(state): State<ExamState>,
    StaffOperator(_): StaffOperator,
    Path(id): Path<Id>,
) -> Result<Json<AdminQuestion>, Problem> {
    Ok(Json(get(&state, id.get()).await?))
}

/// 修改题目（整体覆盖，只有管理员）
#[utoipa::path(
    put,
    path = "/v1/admin/exam/questions/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    request_body = QuestionInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminQuestion),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn update_question(
    State(state): State<ExamState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
    Json(input): Json<QuestionInput>,
) -> Result<Json<AdminQuestion>, Problem> {
    Ok(Json(update(&state, &op, id.get(), input).await?))
}

/// 启用 / 停用题目（只有管理员）
#[utoipa::path(
    post,
    path = "/v1/admin/exam/questions/{id}/toggle",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = AdminQuestion),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn toggle_question(
    State(state): State<ExamState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
) -> Result<Json<AdminQuestion>, Problem> {
    Ok(Json(toggle(&state, &op, id.get()).await?))
}

/// 批量删除题目（只有管理员）
///
/// 物理删除；历史考试有快照，不受影响。一次最多 500 道。
#[utoipa::path(
    post,
    path = "/v1/admin/exam/questions/bulk-delete",
    tag = "admin",
    request_body = BulkDeleteRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = BulkDeleteResult),
        (status = 400, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn bulk_delete_questions(
    State(state): State<ExamState>,
    AdminOperator(op): AdminOperator,
    Json(request): Json<BulkDeleteRequest>,
) -> Result<Json<BulkDeleteResult>, Problem> {
    Ok(Json(bulk_delete(&state, &op, request).await?))
}
