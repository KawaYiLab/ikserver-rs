//! 后台「系统状态」：版本与运行时长、数据库、搜索、存储与发信的配置情况、后台任务队列，
//! 以及处理死信任务、发测试邮件。只读部署配置里不含密钥的部分（只说配没配）。

use std::sync::LazyLock;
use std::time::Instant;

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Utc};
use http::StatusCode;
use ik_auth::admin::{AdminOperator, Audit};
use ik_kernel::Problem;
use ik_kernel::config::MailProvider;
use ik_kernel::mail::OutgoingMail;
use serde::{Deserialize, Serialize};
use sqlx::Connection;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::{AppState, MIGRATOR};

static STARTED_AT: LazyLock<DateTime<Utc>> = LazyLock::new(Utc::now);

pub fn router() -> OpenApiRouter<AppState> {
    LazyLock::force(&STARTED_AT);
    OpenApiRouter::new()
        .routes(routes!(system_status))
        .routes(routes!(list_system_jobs))
        .routes(routes!(retry_system_job, delete_system_job))
        .routes(routes!(send_test_mail))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    pub version: &'static str,
    /// 本实例的启动时间。
    pub started_at: DateTime<Utc>,
    pub node_id: u16,
    /// 是否多实例部署（事件经数据库转发给其他实例）。
    pub cluster: bool,
    pub database: SystemDatabase,
    pub search: SystemSearch,
    pub storage: SystemStorage,
    pub mail: SystemMail,
    pub jobs: SystemJobs,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemDatabase {
    pub ok: bool,
    /// 一次 ping 的往返耗时。
    pub latency_ms: Option<f64>,
    pub server_version: Option<String>,
    pub size_bytes: Option<i64>,
    pub pool_size: u32,
    pub pool_idle: u32,
    /// 已应用的最新迁移版本。
    pub migration: Option<i64>,
    /// 二进制里有、数据库还没应用的迁移数。
    pub pending_migrations: usize,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemSearch {
    /// 是否配置了搜索引擎；没配时搜索走数据库。
    pub configured: bool,
    pub ok: bool,
    pub posts: Option<usize>,
    pub comments: Option<usize>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemStorage {
    /// 配置了存储桶；没配时上传接口返回 503。
    pub configured: bool,
    /// 对外访问地址前缀。
    pub public_base_url: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemMail {
    /// `dev_log`：只写日志不发信；`dual`：国内邮箱走 SMTP，其余走 Brevo。
    pub provider: String,
    pub smtp: bool,
    pub brevo: bool,
    pub fallback: bool,
}

#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemJobs {
    /// 到点等待执行的任务。
    pub ready: i64,
    /// 还没到执行时间（含失败后等待重试）的任务。
    pub scheduled: i64,
    pub running: i64,
    /// 用完重试次数、不再执行的任务（死信）。
    pub failed: i64,
    /// 按任务类型分开的计数，只列有任务的类型。
    pub tasks: Vec<SystemTaskCount>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemTaskCount {
    pub task: String,
    pub ready: i64,
    pub scheduled: i64,
    pub running: i64,
    pub failed: i64,
}

/// 系统状态
#[utoipa::path(
    get,
    path = "/v1/admin/system",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = SystemStatus))
)]
async fn system_status(
    State(state): State<AppState>,
    AdminOperator(_): AdminOperator,
) -> Result<Json<SystemStatus>, Problem> {
    let (database, jobs) = database_and_jobs(&state).await;
    let search = search_status(&state).await;
    let config = &state.config;
    Ok(Json(SystemStatus {
        version: env!("CARGO_PKG_VERSION"),
        started_at: *STARTED_AT,
        node_id: config.node_id,
        cluster: config.bus.cluster,
        database,
        search,
        storage: SystemStorage {
            configured: !config.storage.bucket.is_empty(),
            public_base_url: config.storage.public_base_url.clone(),
        },
        mail: SystemMail {
            provider: match config.mail.provider {
                MailProvider::DevLog => "dev_log",
                MailProvider::Dual => "dual",
            }
            .to_owned(),
            smtp: config.mail.smtp.enabled(),
            brevo: config.mail.brevo.enabled(),
            fallback: config.mail.fallback,
        },
        jobs,
    }))
}

async fn database_and_jobs(state: &AppState) -> (SystemDatabase, SystemJobs) {
    let mut status = SystemDatabase {
        ok: false,
        latency_ms: None,
        server_version: None,
        size_bytes: None,
        pool_size: state.db.size(),
        pool_idle: u32::try_from(state.db.num_idle()).unwrap_or(u32::MAX),
        migration: None,
        pending_migrations: 0,
    };
    let mut conn = match state.db.acquire().await {
        Ok(conn) => conn,
        Err(error) => {
            tracing::warn!(%error, "系统状态：获取数据库连接失败");
            return (status, SystemJobs::default());
        }
    };
    let started = Instant::now();
    if conn.ping().await.is_err() {
        return (status, SystemJobs::default());
    }
    status.ok = true;
    status.latency_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
    if let Ok(row) = sqlx::query!(
        r#"SELECT current_setting('server_version') AS "version!",
                  pg_database_size(current_database()) AS "size!""#
    )
    .fetch_one(&mut *conn)
    .await
    {
        status.server_version = Some(row.version);
        status.size_bytes = Some(row.size);
    }
    if let Ok(applied) =
        sqlx::query_scalar!("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
            .fetch_all(&mut *conn)
            .await
    {
        status.migration = applied.last().copied();
        status.pending_migrations = MIGRATOR
            .iter()
            .filter(|m| !m.migration_type.is_down_migration() && !applied.contains(&m.version))
            .count();
    }
    let tasks = sqlx::query_as!(
        SystemTaskCount,
        r#"SELECT t.identifier AS task,
                  count(*) FILTER (WHERE j.is_available AND j.run_at <= now()) AS "ready!",
                  count(*) FILTER (WHERE j.is_available AND j.run_at > now()) AS "scheduled!",
                  count(*) FILTER (WHERE j.locked_at IS NOT NULL) AS "running!",
                  count(*) FILTER (WHERE j.locked_at IS NULL AND j.attempts >= j.max_attempts) AS "failed!"
             FROM graphile_worker._private_jobs j
             JOIN graphile_worker._private_tasks t ON t.id = j.task_id
            GROUP BY t.identifier
            ORDER BY t.identifier"#
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap_or_else(|error| {
        tracing::warn!(%error, "系统状态：读取任务队列失败");
        Vec::new()
    });
    let jobs = SystemJobs {
        ready: tasks.iter().map(|t| t.ready).sum(),
        scheduled: tasks.iter().map(|t| t.scheduled).sum(),
        running: tasks.iter().map(|t| t.running).sum(),
        failed: tasks.iter().map(|t| t.failed).sum(),
        tasks,
    };
    (status, jobs)
}

async fn search_status(state: &AppState) -> SystemSearch {
    let configured = state.search.engine_configured();
    let counts = if configured {
        state.search.document_counts().await
    } else {
        Ok(None)
    };
    match counts {
        Ok(counts) => SystemSearch {
            configured,
            ok: true,
            posts: counts.map(|c| c.0),
            comments: counts.map(|c| c.1),
        },
        Err(error) => {
            tracing::warn!(%error, "系统状态：搜索引擎不可用");
            SystemSearch {
                configured,
                ok: false,
                posts: None,
                comments: None,
            }
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct JobListQuery {
    /// `failed`（死信，默认）、`scheduled`（等待重试或定时）、`running`。
    pub state: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueuedJob {
    pub id: String,
    pub task: String,
    pub attempts: i16,
    pub max_attempts: i16,
    pub last_error: Option<String>,
    pub run_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub locked_at: Option<DateTime<Utc>>,
}

/// 后台任务列表
///
/// 最多 100 条，最近更新的在前。不含 payload（可能有用户内容）。
#[utoipa::path(
    get,
    path = "/v1/admin/system/jobs",
    tag = "admin",
    params(JobListQuery),
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = Vec<QueuedJob>))
)]
async fn list_system_jobs(
    State(state): State<AppState>,
    AdminOperator(_): AdminOperator,
    Query(query): Query<JobListQuery>,
) -> Result<Json<Vec<QueuedJob>>, Problem> {
    let filter = query.state.as_deref().unwrap_or("failed");
    if !matches!(filter, "failed" | "scheduled" | "running") {
        return Err(Problem::field(
            "state",
            "INVALID",
            "只能是 failed、scheduled 或 running",
        ));
    }
    let jobs = sqlx::query_as!(
        QueuedJob,
        r#"SELECT j.id::text AS "id!", t.identifier AS task, j.attempts, j.max_attempts,
                  left(j.last_error, 2000) AS last_error, j.run_at, j.created_at, j.updated_at, j.locked_at
             FROM graphile_worker._private_jobs j
             JOIN graphile_worker._private_tasks t ON t.id = j.task_id
            WHERE ($1 = 'failed' AND j.locked_at IS NULL AND j.attempts >= j.max_attempts)
               OR ($1 = 'scheduled' AND j.is_available AND j.run_at > now())
               OR ($1 = 'running' AND j.locked_at IS NOT NULL)
            ORDER BY j.updated_at DESC
            LIMIT 100"#,
        filter
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(jobs))
}

fn job_id(raw: &str) -> Result<i64, Problem> {
    raw.parse().map_err(|_| job_not_found())
}

fn job_not_found() -> Problem {
    Problem::new(
        StatusCode::NOT_FOUND,
        "JOB_NOT_FOUND",
        "任务不存在或正在执行",
    )
}

/// 立即重试任务
///
/// 重置尝试次数并马上执行；正在执行的任务不能重试。
#[utoipa::path(
    post,
    path = "/v1/admin/system/jobs/{id}/retry",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = QueuedJob))
)]
async fn retry_system_job(
    State(state): State<AppState>,
    AdminOperator(op): AdminOperator,
    Path(raw): Path<String>,
) -> Result<Json<QueuedJob>, Problem> {
    let id = job_id(&raw)?;
    let mut tx = state.db.begin().await?;
    let task = locked_task(&mut tx, id).await?;
    sqlx::query!(
        "SELECT id FROM graphile_worker.reschedule_jobs(ARRAY[$1::bigint], run_at => now(), attempts => 0)",
        id
    )
    .fetch_all(&mut *tx)
    .await?;
    let job = sqlx::query_as!(
        QueuedJob,
        r#"SELECT j.id::text AS "id!", t.identifier AS task, j.attempts, j.max_attempts,
                  left(j.last_error, 2000) AS last_error, j.run_at, j.created_at, j.updated_at, j.locked_at
             FROM graphile_worker._private_jobs j
             JOIN graphile_worker._private_tasks t ON t.id = j.task_id
            WHERE j.id = $1"#,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    ik_auth::admin::record(
        &mut tx,
        &state.ids,
        &op,
        Audit {
            action: "system.job.retry",
            target_type: "job",
            target_id: Some(raw),
            details: serde_json::json!({ "task": task }),
        },
        Utc::now(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(job))
}

/// 删除任务
///
/// 从队列里移除，不再执行；正在执行的任务不能删除。
#[utoipa::path(
    delete,
    path = "/v1/admin/system/jobs/{id}",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 204))
)]
async fn delete_system_job(
    State(state): State<AppState>,
    AdminOperator(op): AdminOperator,
    Path(raw): Path<String>,
) -> Result<StatusCode, Problem> {
    let id = job_id(&raw)?;
    let mut tx = state.db.begin().await?;
    let task = locked_task(&mut tx, id).await?;
    sqlx::query!(
        "SELECT id FROM graphile_worker.complete_jobs(ARRAY[$1::bigint])",
        id
    )
    .fetch_all(&mut *tx)
    .await?;
    ik_auth::admin::record(
        &mut tx,
        &state.ids,
        &op,
        Audit {
            action: "system.job.delete",
            target_type: "job",
            target_id: Some(raw),
            details: serde_json::json!({ "task": task }),
        },
        Utc::now(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 锁住一个没在执行的任务，返回它的类型。
async fn locked_task(conn: &mut sqlx::PgConnection, id: i64) -> Result<String, Problem> {
    sqlx::query_scalar!(
        "SELECT t.identifier
           FROM graphile_worker._private_jobs j
           JOIN graphile_worker._private_tasks t ON t.id = j.task_id
          WHERE j.id = $1 AND j.locked_at IS NULL
            FOR UPDATE OF j",
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(job_not_found)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TestMailInput {
    pub to: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestMailResult {
    /// 实际发往的地址（已规范化）。
    pub to: String,
    /// 发信器是只写日志的开发模式时为 true：没有真的发出去。
    pub dev_log: bool,
    pub ok: bool,
    /// 发送失败的原因（发信通道返回的错误）。
    pub error: Option<String>,
}

/// 发一封测试邮件
///
/// 按正式发信的路由选通道（国内邮箱走 SMTP，其余走 Brevo），用来确认发信配置可用。
/// 发送失败也返回 200，原因在 `error` 里。
#[utoipa::path(
    post,
    path = "/v1/admin/system/test-mail",
    tag = "admin",
    request_body = TestMailInput,
    security(("session_cookie" = []), ("bearer" = [])),
    responses((status = 200, body = TestMailResult))
)]
async fn send_test_mail(
    State(state): State<AppState>,
    AdminOperator(op): AdminOperator,
    Json(input): Json<TestMailInput>,
) -> Result<Json<TestMailResult>, Problem> {
    let to = ik_auth::validate::normalize_email(&input.to)
        .map_err(|_| Problem::field("to", "INVALID", "邮箱格式不正确"))?;
    let now = Utc::now();
    let sent = state
        .auth
        .mailer
        .send(OutgoingMail {
            to: to.clone(),
            subject: "绳网测试邮件".to_owned(),
            text: format!(
                "这是一封来自绳网管理后台的测试邮件，由管理员（用户 id {}）在 {} 发出。收到说明发信配置可用。",
                op.user_id(),
                now.format("%Y-%m-%d %H:%M:%S UTC"),
            ),
            html: None,
        })
        .await;
    let mut conn = state.db.acquire().await?;
    ik_auth::admin::record(
        &mut conn,
        &state.ids,
        &op,
        Audit {
            action: "system.mail.test",
            target_type: "mail",
            target_id: None,
            details: serde_json::json!({
                "to": ik_kernel::mail::mask_email(&to),
                "ok": sent.is_ok(),
            }),
        },
        now,
    )
    .await?;
    Ok(Json(TestMailResult {
        to,
        dev_log: state.config.mail.provider == MailProvider::DevLog,
        ok: sent.is_ok(),
        error: sent.err().map(|error| error.0),
    }))
}
