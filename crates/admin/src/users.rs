//! 账号管理（media-ops.md ADMINCRUD「用户」：设管理员 / AI 账号、手动通过考试、调整丁尼 / 经验；
//! 封禁与解封在 ik-moderation）。
//!
//! 列表与详情：版主或管理员（邮箱只有管理员看到全文）；修改：只有管理员。每个修改与审计同一事务。

use chrono::{DateTime, Utc};
use http::StatusCode;
use ik_auth::Operator;
use ik_auth::admin::{self, AdminUser, Audit};
use ik_auth::dto::SessionView;
use ik_economy::dto::{DennyLogView, ExpLogView};
use ik_economy::service::{Dedup, DennyAction, ExpAction};
use ik_kernel::bus::{Audience, Effects, Event, internal};
use ik_kernel::cursor::Paged;
use ik_kernel::{Problem, Role};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::AdminState;

/// 备注最多这么多字。
const REASON_MAX_CHARS: usize = 200;
/// 一次调整丁尼的上限（余额本身封顶 9999，D-002）。
const DENNY_STEP_MAX: i32 = 9999;
/// 一次发放经验的上限。
const EXP_STEP_MAX: i32 = 100_000;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminUserDetail {
    pub user: AdminUser,
    /// 丁尼余额。
    pub denny: i32,
    /// 累计经验。
    pub exp: i64,
    /// 已发布、公开、实名的帖子数。
    pub post_count: i64,
    pub total_views: i64,
    pub total_likes: i64,
    /// 这些帖子收到的评论数。
    pub total_comments: i64,
    /// 签名。
    pub bio: String,
    /// 自定义头像的地址；没有时为 null（显示默认头像或系统头像）。
    pub avatar_url: Option<String>,
    /// 最近一次活跃（全部会话里最新的）。
    pub last_seen_at: Option<DateTime<Utc>>,
    /// 累计投出的丁尼。
    pub denny_given: i32,
    pub check_in_streak: i32,
    pub check_in_total: i32,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct LedgerQuery {
    pub page: Option<u32>,
    /// 默认 20，最大 100。
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevokeResult {
    /// 吊销的会话数。
    pub revoked: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsernameUpdate {
    /// 新用户名（与用户自己改名的规则一致，不收费）。
    pub username: String,
    /// 原因（写进审计；通知用户时附上）。
    #[serde(default)]
    pub note: Option<String>,
    /// 给用户发系统通知。
    #[serde(default)]
    pub notify: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileReset {
    /// 清空签名。
    #[serde(default)]
    pub bio: bool,
    /// 清除自定义头像（系统头像不动）。
    #[serde(default)]
    pub avatar: bool,
    #[serde(default)]
    pub note: Option<String>,
    /// 给用户发系统通知。
    #[serde(default)]
    pub notify: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileResetResult {
    /// 确实清掉了签名（原本就是空的为 false）。
    pub bio_cleared: bool,
    pub avatar_cleared: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleUpdate {
    pub role: Role,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiAgentUpdate {
    pub is_ai_agent: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DennyAdjustment {
    /// 正数发放、负数扣除，不能为 0；单次绝对值不超过 9999。余额封顶 9999、不能扣成负数。
    pub amount: i32,
    /// 原因（写进流水与审计，1–200 字）。
    pub reason: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpGrant {
    /// 1–100000，不占每日上限。
    pub amount: i32,
    pub reason: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EconomyResult {
    pub denny: i32,
    pub exp: i64,
    pub level: i16,
}

fn reason(raw: &str) -> Result<String, Problem> {
    let text: String = raw.trim().chars().take(REASON_MAX_CHARS).collect();
    if text.is_empty() {
        return Err(Problem::field("reason", "REQUIRED", "请填写原因"));
    }
    Ok(text)
}

fn account_unavailable() -> Problem {
    Problem::new(
        StatusCode::CONFLICT,
        "ACCOUNT_UNAVAILABLE",
        "该账号已封禁、注销或不存在，不能调整",
    )
}

pub async fn list(
    state: &AdminState,
    op: &Operator,
    query: admin::UserQuery,
) -> Result<ik_kernel::cursor::Paged<AdminUser>, Problem> {
    let mut conn = state.db.acquire().await?;
    admin::list_users(&mut conn, &op.principal, query).await
}

pub async fn detail(
    state: &AdminState,
    op: &Operator,
    id: i64,
) -> Result<AdminUserDetail, Problem> {
    let mut conn = state.db.acquire().await?;
    let user = admin::user(&mut conn, &op.principal, id).await?;
    let economy = ik_economy::service::summary_of(&mut conn, id).await?;
    let bio = ik_profile::interop::bio_of(&mut conn, id).await?;
    let last_seen_at = admin::last_seen(&mut conn, id).await?;
    drop(conn);
    let stats = ik_content::service::author_stats(&state.content, id).await?;
    let authors = ik_auth::service::authors(&state.db, &[id]).await?;
    let avatar_url = state
        .content
        .media
        .avatar_urls(authors.values())
        .await?
        .remove(&id);
    Ok(AdminUserDetail {
        user,
        denny: economy.denny,
        exp: economy.exp,
        post_count: stats.post_count,
        total_views: stats.total_views,
        total_likes: stats.total_likes,
        total_comments: stats.total_comments,
        bio,
        avatar_url,
        last_seen_at,
        denny_given: economy.denny_given,
        check_in_streak: economy.check_in_streak,
        check_in_total: economy.check_in_total,
    })
}

pub async fn set_role(
    state: &AdminState,
    op: &Operator,
    id: i64,
    update: RoleUpdate,
) -> Result<AdminUserDetail, Problem> {
    let mut tx = state.db.begin().await?;
    admin::set_role(&mut tx, &state.ids, op, id, update.role).await?;
    tx.commit().await?;
    detail(state, op, id).await
}

pub async fn set_ai_agent(
    state: &AdminState,
    op: &Operator,
    id: i64,
    update: AiAgentUpdate,
) -> Result<AdminUserDetail, Problem> {
    let mut tx = state.db.begin().await?;
    admin::set_ai_agent(&mut tx, &state.ids, op, id, update.is_ai_agent).await?;
    tx.commit().await?;
    detail(state, op, id).await
}

async fn economy_result(state: &AdminState, id: i64) -> Result<EconomyResult, Problem> {
    let mut conn = state.db.acquire().await?;
    let denny = ik_economy::service::balance_of(&mut conn, id).await?;
    let exp = ik_economy::service::exp_of(&mut conn, id).await?;
    let level = ik_auth::interop::profile_user(&mut conn, id)
        .await?
        .map_or(0, |u| u.level);
    Ok(EconomyResult { denny, exp, level })
}

/// 手动通过入站考试（不发首次通过奖励）：记通过时间并按经验重算等级。已通过的不改。
pub async fn pass_exam(
    state: &AdminState,
    op: &Operator,
    id: i64,
) -> Result<AdminUserDetail, Problem> {
    let now: DateTime<Utc> = Utc::now();
    let mut tx = state.db.begin().await?;
    if ik_auth::interop::lock_active_user(&mut tx, id)
        .await?
        .is_none()
    {
        return Err(account_unavailable());
    }
    let first = ik_auth::interop::mark_exam_passed(&mut tx, id, now).await?;
    ik_economy::service::refresh_level(&mut tx, id).await?;
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "user.exam.pass",
            target_type: "user",
            target_id: Some(id.to_string()),
            details: serde_json::json!({ "firstPass": first }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    detail(state, op, id).await
}

/// 调整丁尼：正数发放（`admin_grant`），负数扣除（`admin_deduct`）。
pub async fn adjust_denny(
    state: &AdminState,
    op: &Operator,
    id: i64,
    request: DennyAdjustment,
) -> Result<EconomyResult, Problem> {
    let reason = reason(&request.reason)?;
    if request.amount == 0 || request.amount.unsigned_abs() > DENNY_STEP_MAX.unsigned_abs() {
        return Err(Problem::field(
            "amount",
            "INVALID",
            "数量不能为 0，单次绝对值不超过 9999",
        ));
    }
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    if request.amount > 0 {
        ik_economy::service::add_denny(
            &mut tx,
            &state.ids,
            id,
            DennyAction::AdminGrant,
            request.amount,
            None,
            &reason,
            now,
        )
        .await?
        .ok_or_else(account_unavailable)?;
    } else {
        ik_economy::service::spend_denny(
            &mut tx,
            &state.ids,
            id,
            DennyAction::AdminDeduct,
            request.amount.saturating_neg(),
            &reason,
            now,
        )
        .await?;
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "user.denny.adjust",
            target_type: "user",
            target_id: Some(id.to_string()),
            details: serde_json::json!({ "amount": request.amount, "reason": reason }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    economy_result(state, id).await
}

/// 发放经验（`admin_grant`，不占每日上限），按新经验重算等级。
pub async fn grant_exp(
    state: &AdminState,
    op: &Operator,
    id: i64,
    request: ExpGrant,
) -> Result<EconomyResult, Problem> {
    let reason = reason(&request.reason)?;
    if !(1..=EXP_STEP_MAX).contains(&request.amount) {
        return Err(Problem::field("amount", "INVALID", "数量需为 1–100000"));
    }
    let now = Utc::now();
    let grant_id = state.ids.next_raw();
    let mut tx = state.db.begin().await?;
    ik_economy::service::grant_exp(
        &mut tx,
        &state.ids,
        id,
        ExpAction::AdminGrant,
        request.amount,
        Dedup::Ref(format!("admin:{grant_id}")),
        now,
    )
    .await?
    .ok_or_else(account_unavailable)?;
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "user.exp.grant",
            target_type: "user",
            target_id: Some(id.to_string()),
            details: serde_json::json!({ "amount": request.amount, "reason": reason }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    economy_result(state, id).await
}

// ---------- 账号详情的其他标签页 ----------

pub async fn denny_logs(
    state: &AdminState,
    id: i64,
    query: LedgerQuery,
) -> Result<Paged<DennyLogView>, Problem> {
    let mut conn = state.db.acquire().await?;
    ik_economy::service::denny_log_list(&mut conn, id, query.page, query.limit).await
}

pub async fn exp_logs(
    state: &AdminState,
    id: i64,
    query: LedgerQuery,
) -> Result<Paged<ExpLogView>, Problem> {
    let mut conn = state.db.acquire().await?;
    ik_economy::service::exp_log_list(&mut conn, id, query.page, query.limit).await
}

pub async fn sessions(state: &AdminState, id: i64) -> Result<Vec<SessionView>, Problem> {
    let mut conn = state.db.acquire().await?;
    admin::user_sessions(&mut conn, id).await
}

fn clean_note(note: Option<String>) -> Option<String> {
    note.map(|n| n.trim().chars().take(REASON_MAX_CHARS).collect::<String>())
        .filter(|n| !n.is_empty())
}

fn with_note(message: &str, note: Option<&str>) -> String {
    match note {
        Some(note) => format!("{message}\n说明：{note}"),
        None => message.to_owned(),
    }
}

/// 强制下线：吊销全部会话，在线的连接随即断开。
pub async fn revoke_sessions(
    state: &AdminState,
    op: &Operator,
    id: i64,
) -> Result<RevokeResult, Problem> {
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let revoked = admin::revoke_sessions(&mut tx, &state.ids, op, id, now).await?;
    if revoked > 0 {
        effects.publish(ik_auth::service::sessions_changed_event(id)?);
    }
    tx.commit().await?;
    effects.dispatch(state.content.bus.as_ref());
    Ok(RevokeResult { revoked })
}

/// 强制改名（不收费，封禁中的账号也能改）。提交后通知搜索索引更新此人的作者名。
pub async fn force_rename(
    state: &AdminState,
    op: &Operator,
    id: i64,
    update: UsernameUpdate,
) -> Result<AdminUserDetail, Problem> {
    let username = ik_profile::service::validate_username(&update.username)?;
    let note = clean_note(update.note);
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let renamed =
        admin::force_rename(&mut tx, &state.ids, op, id, &username, note.as_deref(), now).await?;
    if let Some(from) = renamed {
        effects.publish(Event::new(
            internal::USER_RENAMED,
            Audience::Internal,
            &serde_json::json!({ "userId": id }),
        )?);
        if update.notify {
            let message =
                format!("你的用户名「{from}」不符合绳网社区规范，已被管理员改为「{username}」。");
            ik_notify::interop::system(
                &mut tx,
                &state.ids,
                &[id],
                &with_note(&message, note.as_deref()),
                None,
                now,
                &mut effects,
            )
            .await?;
        }
    }
    tx.commit().await?;
    effects.dispatch(state.content.bus.as_ref());
    detail(state, op, id).await
}

/// 清理违规资料：清空签名、清除自定义头像（不收费，封禁中的账号也能清）。版主不能清理版主、管理员的资料。
pub async fn reset_profile(
    state: &AdminState,
    op: &Operator,
    id: i64,
    reset: ProfileReset,
) -> Result<ProfileResetResult, Problem> {
    if !reset.bio && !reset.avatar {
        return Err(Problem::field("bio", "REQUIRED", "请选择要清理的资料"));
    }
    let note = clean_note(reset.note);
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    admin::lock_for_profile_reset(&mut tx, op, id).await?;
    let bio_cleared = reset.bio && ik_profile::interop::clear_bio(&mut tx, id).await?;
    let avatar_cleared = reset.avatar && admin::clear_avatar(&mut tx, id).await?;
    if bio_cleared || avatar_cleared {
        admin::record(
            &mut tx,
            &state.ids,
            op,
            Audit {
                action: "user.profile.reset",
                target_type: "user",
                target_id: Some(id.to_string()),
                details: serde_json::json!({
                    "bio": bio_cleared, "avatar": avatar_cleared, "note": note,
                }),
            },
            now,
        )
        .await?;
        if reset.notify {
            let what = match (bio_cleared, avatar_cleared) {
                (true, true) => "签名和头像",
                (true, false) => "签名",
                _ => "头像",
            };
            let message = format!("你的{what}不符合绳网社区规范，已被管理员清除。");
            ik_notify::interop::system(
                &mut tx,
                &state.ids,
                &[id],
                &with_note(&message, note.as_deref()),
                None,
                now,
                &mut effects,
            )
            .await?;
        }
    }
    tx.commit().await?;
    effects.dispatch(state.content.bus.as_ref());
    Ok(ProfileResetResult {
        bio_cleared,
        avatar_cleared,
    })
}
