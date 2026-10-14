//! HTTP 接口：`/v1/exam`、`/v1/exam/attempts`、`/v1/exam/attempts/{id}/submit`、`/v1/exam/review`。
//!
//! 只要求登录，不受入站考试门控（EXAM-010）。

use axum::extract::{FromRef, State};
use http::StatusCode;
use ik_auth::{AuthState, ClientMeta, CurrentUser};
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path, Query};
use ik_kernel::id::marker::ExamAttempt;
use ik_kernel::{Id, Problem};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::ExamState;
use crate::dto::{ExamStatusView, ReviewQuery, ReviewView, StartView, SubmitRequest, SubmitResult};
use crate::service;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    ExamState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(status))
        .routes(routes!(start))
        .routes(routes!(submit))
        .routes(routes!(review))
}

/// 考试状态
///
/// 是否已通过、冷却剩余时间、进行中的场次与考试参数。
#[utoipa::path(
    get,
    path = "/v1/exam",
    tag = "exam",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ExamStatusView),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn status(
    State(state): State<ExamState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<ExamStatusView>, Problem> {
    Ok(Json(service::status(&state, &principal).await?))
}

/// 开考
///
/// 有未交卷且未过期的场次时原样恢复（`resumed: true`，题目与选项顺序不变）；否则随机抽题新建。
/// 最近 3 次都未通过时进入 30 分钟冷却。
#[utoipa::path(
    post,
    path = "/v1/exam/attempts",
    tag = "exam",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, description = "新开一场", body = StartView),
        (status = 200, description = "恢复进行中的场次", body = StartView),
        (status = 403, description = "ACCOUNT_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "EXAM_ALREADY_PASSED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "EXAM_COOLDOWN（带 Retry-After）", body = ProblemBody, content_type = "application/problem+json"),
        (status = 503, description = "EXAM_NOT_AVAILABLE：题库为空", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn start(
    State(state): State<ExamState>,
    CurrentUser(principal): CurrentUser,
    ClientMeta(meta): ClientMeta,
) -> Result<(StatusCode, Json<StartView>), Problem> {
    let view = service::start(&state, &principal, &meta).await?;
    let status = if view.resumed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(view)))
}

/// 交卷
///
/// 每题选中的 key 与正确答案完全一致才得分（多选不给部分分），达到及格线即通过。
/// 首次通过时发放丁尼与经验奖励。超时交卷记一次失败并返回 409。
#[utoipa::path(
    post,
    path = "/v1/exam/attempts/{id}/submit",
    tag = "exam",
    params(("id" = String, Path, description = "考试场次 id")),
    request_body = SubmitRequest,
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = SubmitResult),
        (status = 400, description = "VALIDATION_FAILED：作答引用了不在本场考试中的题目或选项", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "ACCOUNT_BLOCKED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "EXAM_ATTEMPT_ALREADY_SUBMITTED / EXAM_ATTEMPT_EXPIRED", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn submit(
    State(state): State<ExamState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id<ExamAttempt>>,
    Json(request): Json<SubmitRequest>,
) -> Result<Json<SubmitResult>, Problem> {
    Ok(Json(
        service::submit(&state, &principal, id.get(), request).await?,
    ))
}

/// 回顾已交卷的考试
///
/// 逐题给出自己的作答、对错与解析；不下发正确答案。
#[utoipa::path(
    get,
    path = "/v1/exam/review",
    tag = "exam",
    params(ReviewQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = ReviewView),
        (status = 404, description = "没有已交卷的考试", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn review(
    State(state): State<ExamState>,
    CurrentUser(principal): CurrentUser,
    Query(query): Query<ReviewQuery>,
) -> Result<Json<ReviewView>, Problem> {
    Ok(Json(service::review(&state, &principal, query).await?))
}
