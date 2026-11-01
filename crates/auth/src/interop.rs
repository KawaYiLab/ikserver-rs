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
    /// 站点是否开启入站考试（后台「系统设置」）。
    pub exam_required: bool,
    pub level: i16,
}

impl LockedUser {
    /// 免考试门控、等级不锁 0 的账号：通过考试、管理员（含版主）、AI 账号（EXP-002）；
    /// 站点关闭入站考试时所有账号都免。
    pub fn exam_exempt(&self) -> bool {
        !self.exam_required
            || self.exam_passed_at.is_some()
            || self.role.is_staff()
            || self.is_ai_agent
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
            exam_required: row.exam_required,
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
    /// 会员号（展示编号）。
    pub member_no: i64,
    pub username: String,
    pub level: i16,
    pub is_ai_agent: bool,
    /// 已注销或已合并。
    pub gone: bool,
    pub created_at: DateTime<Utc>,
    /// 自定义头像（媒体 id）。
    pub avatar_media_id: Option<i64>,
    /// 装备的系统头像、论坛名片（装扮目录的 id）。
    pub equipped_avatar_id: Option<i64>,
    pub equipped_card_id: Option<i64>,
}

pub async fn profile_user(
    conn: &mut PgConnection,
    user_id: i64,
) -> Result<Option<ProfileUser>, Problem> {
    Ok(repo::profile_user(conn, user_id)
        .await?
        .map(|row| ProfileUser {
            id: row.id,
            member_no: row.member_no,
            gone: matches!(row.status.as_str(), "deleted" | "merged"),
            username: row.username,
            level: row.level,
            is_ai_agent: row.is_ai_agent,
            created_at: row.created_at,
            avatar_media_id: row.avatar_media_id,
            equipped_avatar_id: row.equipped_avatar_id,
            equipped_card_id: row.equipped_card_id,
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
    /// 装备的论坛名片。
    pub equipped_card_id: Option<i64>,
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
            equipped_card_id: row.equipped_card_id,
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

fn hit(found: bool) -> Result<(), Problem> {
    if found {
        Ok(())
    } else {
        Err(crate::service::account_blocked())
    }
}

/// 设置 / 清除自定义头像。设置时同时卸下系统头像（COSMETIC-006），清除时不动装备。
/// 调用方负责校验媒体归属、扣费，并已锁住用户行。账号不可用时返回 403 `ACCOUNT_BLOCKED`。
pub async fn set_avatar(
    conn: &mut PgConnection,
    user_id: i64,
    media_id: Option<i64>,
) -> Result<(), Problem> {
    hit(match media_id {
        Some(media_id) => repo::set_avatar(conn, user_id, media_id).await?,
        None => repo::clear_avatar(conn, user_id).await?,
    })
}

/// 装备系统头像 `(目录 id, 图片媒体 id)`，`None` 卸下。调用方负责确认目录项存在。
/// 账号不可用时返回 403 `ACCOUNT_BLOCKED`。
pub async fn equip_avatar(
    conn: &mut PgConnection,
    user_id: i64,
    avatar: Option<(i64, i64)>,
) -> Result<(), Problem> {
    hit(repo::equip_avatar(conn, user_id, avatar).await?)
}

/// 装备论坛名片，`None` 卸下。调用方负责确认目录项存在。账号不可用时返回 403 `ACCOUNT_BLOCKED`。
pub async fn equip_card(
    conn: &mut PgConnection,
    user_id: i64,
    card_id: Option<i64>,
) -> Result<(), Problem> {
    hit(repo::equip_card(conn, user_id, card_id).await?)
}

/// 系统头像换了图：同步所有装备着它的账号上冗余的图片 id。返回更新的账号数。
pub async fn refresh_equipped_avatar(
    conn: &mut PgConnection,
    avatar_id: i64,
    media_id: i64,
) -> Result<u64, Problem> {
    Ok(repo::refresh_equipped_avatar(conn, avatar_id, media_id).await?)
}

/// 系统头像被删除：所有装备着它的账号卸下（回到自定义头像或默认头像）。返回卸下的账号数。
pub async fn unequip_system_avatar(
    conn: &mut PgConnection,
    avatar_id: i64,
) -> Result<u64, Problem> {
    Ok(repo::unequip_system_avatar(conn, avatar_id).await?)
}

/// 名片被删除：所有装备着它的账号卸下。返回卸下的账号数。
pub async fn unequip_card(conn: &mut PgConnection, card_id: i64) -> Result<u64, Problem> {
    Ok(repo::unequip_card(conn, card_id).await?)
}

/// 建一个没有邮箱、没有密码的账号（米游社首登，ACC-013、MHY-010）。用户名已被占用时返回 false，
/// 调用方换一个名字重试。注册开关由调用方检查。
pub async fn insert_user_without_email(
    conn: &mut PgConnection,
    user_id: i64,
    username: &str,
    now: DateTime<Utc>,
) -> Result<bool, Problem> {
    Ok(repo::insert_user_without_email(conn, user_id, username, now).await?)
}

/// 账号是否有可用邮箱（没有的账号只能扫码登录，不能解绑米游社，MHY-014）。
pub async fn has_email(conn: &mut PgConnection, user_id: i64) -> Result<bool, Problem> {
    Ok(repo::has_email(conn, user_id).await?)
}

/// 在注销事务里消费注销验证码（DEL-005）：邮箱在核对之后变了 → 409 `ACCOUNT_CHANGED`；
/// 验证码被并发消费或过期 → 400 `CODE_INVALID`。调用方已锁住用户行。
pub async fn consume_delete_code(
    conn: &mut PgConnection,
    user_id: i64,
    code: &crate::service::DeleteCode,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    let email = repo::me(conn, user_id)
        .await?
        .ok_or_else(crate::service::account_blocked)?
        .email;
    if email.as_deref() != Some(code.email.as_str()) {
        return Err(crate::service::account_changed());
    }
    if repo::consume_code(conn, code.code_id, now).await? {
        Ok(())
    } else {
        Err(crate::code::code_invalid())
    }
}

/// 注销账号的 auth 部分（DEL-006）：脱敏（用户名改为「已注销用户_<id>」、清邮箱、清头像与装扮、删密码）、
/// 状态改为 deleted、吊销全部会话。调用方已锁住用户行，提交后发布
/// [`crate::service::sessions_changed_event`]。帖子、评论等内容保留（DEL-010）。
pub async fn anonymize_deleted(
    conn: &mut PgConnection,
    user_id: i64,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    repo::anonymize(conn, user_id, &format!("已注销用户_{user_id}"), now).await?;
    repo::revoke_user_sessions(conn, user_id, None, now, "account_deleted").await?;
    Ok(())
}

/// 在调用方事务中锁住账号状态，防止授权检查与业务写入之间发生注销/封禁。
pub async fn lock_user_status(
    conn: &mut PgConnection,
    user_id: i64,
) -> Result<Option<String>, Problem> {
    Ok(
        sqlx::query_scalar("SELECT status FROM users WHERE id=$1 FOR SHARE")
            .bind(user_id)
            .fetch_optional(conn)
            .await?,
    )
}

/// 绑定命令已持有旧身份锁时，只允许立即取得新账号锁；与注销竞争时回滚重试。
pub async fn try_lock_user_status(
    conn: &mut PgConnection,
    user_id: i64,
) -> Result<Option<String>, Problem> {
    Ok(
        sqlx::query_scalar("SELECT status FROM users WHERE id=$1 FOR SHARE NOWAIT")
            .bind(user_id)
            .fetch_optional(conn)
            .await?,
    )
}

// ---------- 审核（moderation） ----------

/// 账号状态（审核用）。
#[derive(Debug, Clone)]
pub struct UserStatus {
    pub id: i64,
    pub username: String,
    /// active / blocked / deleted / merged。
    pub status: String,
    pub role: Role,
}

impl UserStatus {
    /// 已注销或已合并。
    pub fn gone(&self) -> bool {
        matches!(self.status.as_str(), "deleted" | "merged")
    }
}

impl From<repo::UserStatusRow> for UserStatus {
    fn from(row: repo::UserStatusRow) -> Self {
        Self {
            id: row.id,
            username: row.username,
            status: row.status,
            role: row.role,
        }
    }
}

pub async fn user_statuses(
    conn: &mut PgConnection,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, UserStatus>, Problem> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    Ok(repo::user_statuses(conn, ids)
        .await?
        .into_iter()
        .map(|row| (row.id, row.into()))
        .collect())
}

/// 封禁的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOutcome {
    Blocked,
    /// 原本就是封禁状态。
    AlreadyBlocked,
    /// 已注销、已合并或不存在：没有可封禁的账号。
    Gone,
    /// 工作人员（版主、管理员）不能被封禁。
    Staff,
}

/// 封禁账号（SANCTION-001）：状态改为 blocked 并吊销全部会话（D-059 方案 A）。调用方在提交后发布
/// [`crate::service::sessions_changed_event`]，实时连接随之断开。已发布的内容不隐藏（D-058 方案 A）。
pub async fn block_user(
    conn: &mut PgConnection,
    user_id: i64,
    now: DateTime<Utc>,
) -> Result<BlockOutcome, Problem> {
    let Some(user) = repo::lock_user_status(conn, user_id)
        .await?
        .map(UserStatus::from)
    else {
        return Ok(BlockOutcome::Gone);
    };
    if user.gone() {
        return Ok(BlockOutcome::Gone);
    }
    if user.role.is_staff() {
        return Ok(BlockOutcome::Staff);
    }
    if user.status == "blocked" {
        return Ok(BlockOutcome::AlreadyBlocked);
    }
    repo::set_status(conn, user_id, "blocked", now).await?;
    repo::revoke_user_sessions(conn, user_id, None, now, "blocked").await?;
    Ok(BlockOutcome::Blocked)
}

/// 解封（D-057）。只有封禁中的账号会改回 active；返回是否改了。
pub async fn unblock_user(
    conn: &mut PgConnection,
    user_id: i64,
    now: DateTime<Utc>,
) -> Result<bool, Problem> {
    let Some(user) = repo::lock_user_status(conn, user_id).await? else {
        return Ok(false);
    };
    if user.status != "blocked" {
        return Ok(false);
    }
    repo::set_status(conn, user_id, "active", now).await?;
    Ok(true)
}

/// 用户名包含 `needle` 的账号 id（不区分大小写，最多 `limit` 个）。
pub async fn ids_by_username_fragment(
    conn: &mut PgConnection,
    needle: &str,
    limit: i64,
) -> Result<Vec<i64>, Problem> {
    Ok(repo::ids_by_username_fragment(conn, needle, limit).await?)
}

/// 把用户名为 `username` 的活跃账号标为 AI 账号（AI-012，AI 角色卡导入用），返回其 id；
/// 账号不存在或非活跃时返回 None。
pub async fn mark_ai_agent(
    conn: &mut PgConnection,
    username: &str,
    now: DateTime<Utc>,
) -> Result<Option<i64>, Problem> {
    Ok(repo::mark_ai_agent(conn, username, now).await?)
}
