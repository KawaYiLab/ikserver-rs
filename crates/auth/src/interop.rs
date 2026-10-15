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

/// 状态正常（未封禁、未注销）的账号。
#[derive(Debug, Clone)]
pub struct ActiveUser {
    pub id: i64,
    pub username: String,
    pub is_ai_agent: bool,
}

/// `ids` 中状态正常的账号（顺序不保证）。用于通知收件人过滤、通知文案里的名字。
pub async fn active_users(
    conn: &mut PgConnection,
    ids: &[i64],
) -> Result<Vec<ActiveUser>, Problem> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(repo::active_users(conn, ids)
        .await?
        .into_iter()
        .map(|row| ActiveUser {
            id: row.id,
            username: row.username,
            is_ai_agent: row.is_ai_agent,
        })
        .collect())
}

/// 作者主页需要的账号信息。
#[derive(Debug, Clone)]
pub struct ProfileUser {
    pub id: i64,
    pub username: String,
    pub level: i16,
    pub is_ai_agent: bool,
    /// 已注销或已合并。
    pub gone: bool,
    pub created_at: DateTime<Utc>,
    /// 自定义头像（媒体 id）。
    pub avatar_media_id: Option<i64>,
}

pub async fn profile_user(
    conn: &mut PgConnection,
    user_id: i64,
) -> Result<Option<ProfileUser>, Problem> {
    Ok(repo::profile_user(conn, user_id)
        .await?
        .map(|row| ProfileUser {
            id: row.id,
            gone: matches!(row.status.as_str(), "deleted" | "merged"),
            username: row.username,
            level: row.level,
            is_ai_agent: row.is_ai_agent,
            created_at: row.created_at,
            avatar_media_id: row.avatar_media_id,
        }))
}

/// 改用户名。调用方负责校验格式、扣费，并已锁住用户行。
/// 名字已被占用 → 409 `USERNAME_TAKEN`（调用方的事务随之回滚，扣费一并撤销）。
pub async fn rename(
    conn: &mut PgConnection,
    user_id: i64,
    username: &str,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    match repo::rename(conn, user_id, username, now).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(crate::service::account_blocked()),
        Err(error) if ik_kernel::db::is_unique_violation(&error, "users_username_uq") => {
            Err(Problem::conflict("USERNAME_TAKEN", "该用户名已被占用"))
        }
        Err(error) => Err(error.into()),
    }
}

/// 用户名搜索的一条结果。
#[derive(Debug, Clone)]
pub struct FoundUser {
    pub id: i64,
    pub username: String,
    pub level: i16,
}

/// 按用户名子串搜索状态正常的账号（不区分大小写）。完全匹配排最前，其次前缀匹配，同档按用户名排序
/// （PROFILE-024）。隐藏主页、拉黑过滤由调用方完成。
pub async fn search_users(
    conn: &mut PgConnection,
    query: &str,
    limit: i64,
) -> Result<Vec<FoundUser>, Problem> {
    let escaped = like_escape(query);
    Ok(repo::search_users(conn, query, &escaped, limit)
        .await?
        .into_iter()
        .map(|row| FoundUser {
            id: row.id,
            username: row.username,
            level: row.level,
        })
        .collect())
}

fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
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

/// 设置 / 清除自定义头像。调用方负责校验媒体归属、扣费，并已锁住用户行。
/// 账号不可用时返回 403 `ACCOUNT_BLOCKED`。
pub async fn set_avatar(
    conn: &mut PgConnection,
    user_id: i64,
    media_id: Option<i64>,
) -> Result<(), Problem> {
    if repo::set_avatar(conn, user_id, media_id).await? {
        Ok(())
    } else {
        Err(crate::service::account_blocked())
    }
}
