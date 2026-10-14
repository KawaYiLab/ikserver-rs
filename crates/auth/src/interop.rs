//! 给其他业务域（经验、考试……）用的账号接口。
//!
//! 函数都接收调用方的连接/事务，这样对方的写入与用户行上的状态变更在同一个事务里提交
//! （AGENTS.md 规则 6），同时不需要直接读写 `users` 表（规则 5）。

use chrono::{DateTime, Utc};
use ik_kernel::{Problem, Role};
use sqlx::PgConnection;

use crate::repo;

/// 锁住后的账号状态。
#[derive(Debug, Clone)]
pub struct LockedUser {
    pub id: i64,
    pub role: Role,
    pub is_ai_agent: bool,
    pub exam_passed_at: Option<DateTime<Utc>>,
    pub level: i16,
}

impl LockedUser {
    /// 免考试门控、等级不锁 0 的账号：通过考试、管理员（含版主）、AI 账号（EXP-002）。
    pub fn exam_exempt(&self) -> bool {
        self.exam_passed_at.is_some() || self.role.is_staff() || self.is_ai_agent
    }
}

/// 锁住用户行（`FOR NO KEY UPDATE`），串行化同一用户的经验、丁尼、签到等写入（COMMON-009）。
/// 与外键检查加的 KEY SHARE 锁兼容，调用方可以先插入引用该用户的行再调用。
/// 账号不存在或非活跃（封禁、注销、合并）时返回 None。
pub async fn lock_active_user(
    conn: &mut PgConnection,
    user_id: i64,
) -> Result<Option<LockedUser>, Problem> {
    Ok(repo::lock_active_user_state(conn, user_id)
        .await?
        .map(|row| LockedUser {
            id: row.id,
            role: row.role,
            is_ai_agent: row.is_ai_agent,
            exam_passed_at: row.exam_passed_at,
            level: row.level,
        }))
}

/// 作为关注、拉黑等关系目标的用户。
#[derive(Debug, Clone)]
pub struct UserRef {
    pub id: i64,
    /// 已注销或已合并：不能再被关注、拉黑。
    pub gone: bool,
    pub is_ai_agent: bool,
}

pub async fn user_ref(conn: &mut PgConnection, user_id: i64) -> Result<Option<UserRef>, Problem> {
    Ok(repo::user_ref(conn, user_id).await?.map(|row| UserRef {
        id: row.id,
        gone: matches!(row.status.as_str(), "deleted" | "merged"),
        is_ai_agent: row.is_ai_agent,
    }))
}

/// 入站考试通过时间（不加锁）。用户不存在时返回 None，未通过时返回 `Some(None)`。
pub async fn exam_passed_at(
    conn: &mut PgConnection,
    user_id: i64,
) -> Result<Option<Option<DateTime<Utc>>>, Problem> {
    Ok(repo::exam_passed_at(conn, user_id).await?)
}

/// 写入等级（0–7）。调用方必须已锁住该用户行。
pub async fn set_level(conn: &mut PgConnection, user_id: i64, level: i16) -> Result<(), Problem> {
    repo::set_level(conn, user_id, level.clamp(0, 7), Utc::now()).await?;
    Ok(())
}

/// 记录通过入站考试。已经通过的不会改写通过时间；返回这次是否是首次通过。
pub async fn mark_exam_passed(
    conn: &mut PgConnection,
    user_id: i64,
    at: DateTime<Utc>,
) -> Result<bool, Problem> {
    Ok(repo::mark_exam_passed(conn, user_id, at).await?)
}
