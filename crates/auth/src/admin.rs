//! 管理后台的操作者与审计日志（api-guidelines.md「管理后台」）。
//!
//! - [`StaffOperator`]：版主或管理员（看后台、处理举报、封禁）；
//! - [`AdminOperator`]：只有管理员（改配置：题库、AI、分区、装扮、角色……）。
//!
//! 写操作在业务事务里调用 [`record`] 记一行审计（操作者、动作、目标、参数、IP、UA）。

use axum::extract::{FromRef, FromRequestParts};
use chrono::{DateTime, Utc};
use http::StatusCode;
use http::request::Parts;
use ik_kernel::cursor::{Paged, page_window};
use ik_kernel::{Id, Problem, Snowflake};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use utoipa::{IntoParams, ToSchema};

use crate::extract::{ClientMeta, CurrentUser, Principal};
use crate::service::RequestMeta;
use crate::{AuthState, repo};

/// 发起后台操作的人与请求信息。
#[derive(Debug, Clone)]
pub struct Operator {
    pub principal: Principal,
    pub meta: RequestMeta,
}

impl Operator {
    pub fn user_id(&self) -> i64 {
        self.principal.user_id.get()
    }
}

fn staff_required() -> Problem {
    Problem::new(StatusCode::FORBIDDEN, "FORBIDDEN", "需要版主或管理员权限")
}

fn admin_required() -> Problem {
    Problem::new(StatusCode::FORBIDDEN, "FORBIDDEN", "需要管理员权限")
}

async fn operator<S>(parts: &mut Parts, state: &S) -> Result<Operator, Problem>
where
    S: Send + Sync,
    AuthState: FromRef<S>,
{
    let CurrentUser(principal) = CurrentUser::from_request_parts(parts, state).await?;
    let ClientMeta(meta) = ClientMeta::from_request_parts(parts, state).await?;
    Ok(Operator { principal, meta })
}

/// 版主或管理员。
#[derive(Debug, Clone)]
pub struct StaffOperator(pub Operator);

impl<S> FromRequestParts<S> for StaffOperator
where
    S: Send + Sync,
    AuthState: FromRef<S>,
{
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let op = operator(parts, state).await?;
        if !op.principal.role.is_staff() {
            return Err(staff_required());
        }
        Ok(Self(op))
    }
}

/// 只有管理员。
#[derive(Debug, Clone)]
pub struct AdminOperator(pub Operator);

impl<S> FromRequestParts<S> for AdminOperator
where
    S: Send + Sync,
    AuthState: FromRef<S>,
{
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let op = operator(parts, state).await?;
        if !op.principal.role.is_admin() {
            return Err(admin_required());
        }
        Ok(Self(op))
    }
}

/// 一条审计。
#[derive(Debug, Clone)]
pub struct Audit<'a> {
    /// 形如 `exam.question.create`。
    pub action: &'a str,
    pub target_type: &'a str,
    pub target_id: Option<String>,
    pub details: serde_json::Value,
}

/// 在调用方的事务里记一行审计。
pub async fn record(
    conn: &mut PgConnection,
    ids: &Snowflake,
    op: &Operator,
    audit: Audit<'_>,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    repo::insert_audit(
        conn,
        &repo::NewAudit {
            id: ids.next_raw(),
            actor_id: op.user_id(),
            action: audit.action,
            target_type: audit.target_type,
            target_id: audit.target_id.as_deref(),
            details: &audit.details,
            ip: op.meta.ip.to_string(),
            user_agent: op.meta.user_agent.as_deref(),
            now,
        },
    )
    .await?;
    Ok(())
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct AuditQuery {
    /// 操作者用户 id。
    pub actor_id: Option<String>,
    /// 动作前缀，如 `exam.`、`ai.model`。
    pub action: Option<String>,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub page: Option<u32>,
    /// 默认 50，最大 100。
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    #[schema(value_type = String)]
    pub id: Id,
    #[schema(value_type = Option<String>)]
    pub actor_id: Option<Id>,
    /// 操作者用户名（账号已删除时为空）。
    pub actor_name: Option<String>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    #[schema(value_type = Object)]
    pub details: serde_json::Value,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 审计日志，新的在前。
pub async fn list(
    conn: &mut PgConnection,
    query: AuditQuery,
) -> Result<Paged<AuditEntry>, Problem> {
    let (page, limit, offset) = page_window(query.page, query.limit, 50);
    let actor_id = match query
        .actor_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(raw) => Some(
            raw.parse::<i64>()
                .map_err(|_| Problem::field("actorId", "INVALID", "不是合法的用户 id"))?,
        ),
    };
    let clean = |v: Option<String>| v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let filter = repo::AuditFilter {
        actor_id,
        action_prefix: clean(query.action),
        target_type: clean(query.target_type),
        target_id: clean(query.target_id),
    };
    let total = repo::count_audit(conn, &filter).await?;
    let rows = repo::list_audit(conn, &filter, i64::from(limit), offset).await?;
    Ok(Paged {
        items: rows
            .into_iter()
            .map(|r| AuditEntry {
                id: Id::new(r.id),
                actor_id: r.actor_id.map(Id::new),
                actor_name: r.actor_name,
                action: r.action,
                target_type: r.target_type,
                target_id: r.target_id,
                details: r.details,
                ip: r.ip,
                user_agent: r.user_agent,
                created_at: r.created_at,
            })
            .collect(),
        total,
        page,
        limit,
    })
}

// ---------- 账号 ----------

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct UserQuery {
    /// 用户名或邮箱包含；是数字时也按 id 精确匹配。
    pub q: Option<String>,
    /// `user` | `moderator` | `admin`。
    pub role: Option<String>,
    /// `active` | `blocked` | `deleted` | `merged`。
    pub status: Option<String>,
    /// 只看（或排除）AI 账号。
    pub ai: Option<bool>,
    pub page: Option<u32>,
    /// 默认 20，最大 100。
    pub limit: Option<u32>,
}

/// 后台看到的账号。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminUser {
    #[schema(value_type = String)]
    pub id: Id,
    pub username: String,
    /// 只有管理员看得到；版主看到的是打码后的形式。
    pub email: Option<String>,
    pub status: String,
    pub role: ik_kernel::Role,
    pub is_ai_agent: bool,
    pub exam_passed_at: Option<DateTime<Utc>>,
    pub level: i16,
    pub created_at: DateTime<Utc>,
}

/// 邮箱打码：保留首字符与域名。
fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) => {
            let first: String = local.chars().take(1).collect();
            format!("{first}***@{domain}")
        }
        None => "***".to_owned(),
    }
}

fn user_view(row: repo::AdminUserRow, full_email: bool) -> AdminUser {
    AdminUser {
        id: Id::new(row.id),
        username: row.username,
        email: row
            .email
            .map(|e| if full_email { e } else { mask_email(&e) }),
        status: row.status,
        role: row.role,
        is_ai_agent: row.is_ai_agent,
        exam_passed_at: row.exam_passed_at,
        level: row.level,
        created_at: row.created_at,
    }
}

fn pattern(q: &str) -> String {
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

/// 账号列表，新注册的在前。
pub async fn list_users(
    conn: &mut PgConnection,
    viewer: &Principal,
    query: UserQuery,
) -> Result<Paged<AdminUser>, Problem> {
    let (page, limit, offset) = page_window(query.page, query.limit, 20);
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let clean = |v: Option<String>| v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let filter = repo::UserFilter {
        pattern: q.map(pattern),
        id: q.and_then(|q| q.parse::<i64>().ok()),
        role: clean(query.role),
        status: clean(query.status),
        ai: query.ai,
    };
    let total = repo::count_admin_users(conn, &filter).await?;
    let rows = repo::list_admin_users(conn, &filter, i64::from(limit), offset).await?;
    let full = viewer.role.is_admin();
    Ok(Paged {
        items: rows.into_iter().map(|r| user_view(r, full)).collect(),
        total,
        page,
        limit,
    })
}

pub async fn user(
    conn: &mut PgConnection,
    viewer: &Principal,
    id: i64,
) -> Result<AdminUser, Problem> {
    repo::admin_user(conn, id)
        .await?
        .map(|r| user_view(r, viewer.role.is_admin()))
        .ok_or_else(Problem::not_found)
}

/// 改角色（只有管理员；不能改自己，避免把最后一个管理员降级）。
pub async fn set_role(
    conn: &mut PgConnection,
    ids: &Snowflake,
    op: &Operator,
    id: i64,
    role: ik_kernel::Role,
) -> Result<(), Problem> {
    if id == op.user_id() {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "CANNOT_CHANGE_SELF",
            "不能修改自己的角色",
        ));
    }
    let now = Utc::now();
    if !repo::set_role(conn, id, role, now).await? {
        return Err(Problem::not_found());
    }
    record(
        conn,
        ids,
        op,
        Audit {
            action: "user.role.set",
            target_type: "user",
            target_id: Some(id.to_string()),
            details: serde_json::json!({ "role": role.as_str() }),
        },
        now,
    )
    .await
}

/// 标记 / 取消 AI 账号（ADMINCRUD：设 AI 账号）。
pub async fn set_ai_agent(
    conn: &mut PgConnection,
    ids: &Snowflake,
    op: &Operator,
    id: i64,
    on: bool,
) -> Result<(), Problem> {
    let now = Utc::now();
    if !repo::set_ai_agent(conn, id, on, now).await? {
        return Err(Problem::not_found());
    }
    record(
        conn,
        ids,
        op,
        Audit {
            action: "user.ai_agent.set",
            target_type: "user",
            target_id: Some(id.to_string()),
            details: serde_json::json!({ "isAiAgent": on }),
        },
        now,
    )
    .await
}

/// 账号数（仪表盘）：现存账号、封禁中、`since` 之后注册的。
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserCounts {
    pub total: i64,
    pub blocked: i64,
    pub new_today: i64,
}

pub async fn user_counts(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
) -> Result<UserCounts, Problem> {
    let row = repo::user_counts(conn, since).await?;
    Ok(UserCounts {
        total: row.total,
        blocked: row.blocked,
        new_today: row.new_since,
    })
}
