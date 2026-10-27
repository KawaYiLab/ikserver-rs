//! 站点参数里的内容与账号规则（后台「内容与安全」）：屏蔽词、新账号限制。各业务域在写入前调用。
//! 版主、管理员与 AI 账号不受这些限制。

use chrono::{DateTime, Duration, Utc};
use http::StatusCode;
use ik_kernel::Problem;
use ik_kernel::site::{Screened, SiteValues};
use sqlx::PgConnection;

use crate::{Principal, repo};

/// 不受内容规则限制的账号。
pub fn exempt(principal: &Principal) -> bool {
    principal.role.is_staff() || principal.is_ai_agent
}

/// 按屏蔽词检查一个字段：命中拦截词时报字段错误 `BLOCKED_WORD`，命中替换词时返回替换后的文字。
pub fn screen(values: &SiteValues, field: &str, text: &str) -> Result<String, Problem> {
    match values.screen(text) {
        Screened::Clean(text) => Ok(text),
        Screened::Blocked(word) => Err(Problem::field(
            field,
            "BLOCKED_WORD",
            format!("包含不允许的内容：「{word}」"),
        )),
    }
}

/// 账号还在新账号限制期内时，返回统计「24 小时内」发了多少的起点；不受限制时返回 `None`。
pub async fn new_user_window(
    conn: &mut PgConnection,
    values: &SiteValues,
    principal: &Principal,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, Problem> {
    let hours = values.new_user.restrict_hours;
    if hours == 0 || exempt(principal) {
        return Ok(None);
    }
    let Some(registered) = repo::registered_at(conn, principal.user_id.get()).await? else {
        return Ok(None);
    };
    Ok((now - registered < Duration::hours(i64::from(hours))).then(|| now - Duration::hours(24)))
}

/// 新账号不能贴外部链接。
pub fn links_not_allowed(field: &str) -> Problem {
    Problem::field(field, "LINKS_NOT_ALLOWED", "新注册的账号暂时不能发外部链接")
}

/// 新账号 24 小时内的发帖 / 评论数到了上限。
pub fn new_user_limit(what: &str, limit: u32) -> Problem {
    Problem::new(
        StatusCode::TOO_MANY_REQUESTS,
        "NEW_USER_LIMIT",
        format!("新注册的账号 24 小时内最多{what} {limit} 次，请稍后再来"),
    )
    .with_retry_after(3600)
}
