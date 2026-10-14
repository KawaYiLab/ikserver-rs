//! 经济业务逻辑。业务规则：`docs/business-rules/social-economy.md` 第 6–9 节。
//!
//! 对其他域公开的写入函数（[`grant_exp`]、[`add_denny`]、[`spend_denny`]、[`gift_post`]）
//! 都接收调用方的连接/事务，与对方的业务写入在同一个事务里提交（AGENTS.md 规则 6）。
//! 每个函数都先锁住用户行（auth 的 `lock_active_user`），同一用户的经济写入因此串行，
//! 每日上限不会被并发击穿（EXP-006）。锁顺序：调用方自己的行（如帖子）→ 用户行（多人时按 id 升序）
//! → 经济行。

use chrono::{DateTime, Duration, NaiveDate, Utc};
use http::StatusCode;
use ik_auth::Principal;
use ik_auth::interop::{self, LockedUser};
use ik_kernel::cursor::{self, Page};
use ik_kernel::{Problem, Snowflake, time};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::dto::{
    BenefitsView, CheckInResult, CheckInStatusView, DennyLogView, ExpSource, ExpSources, ExpView,
    PageQuery, WalletView,
};
use crate::repo::{self, EconomyRow, NewCheckIn, NewDennyLog, NewExpLog};
use crate::{EconomyState, benefits, level};

/// 余额上限（D-002 方案 A：所有入账统一按 9999 封顶，超出部分丢弃）。
pub const MAX_BALANCE: i32 = 9999;
/// 新用户初始余额（DENNY-001）。与 `user_economy.denny_balance` 的列默认值一致。
pub const INITIAL_BALANCE: i32 = 30;
/// 签到奖励丁尼（DENNY-018）。
pub const CHECK_IN_DENNY: i32 = 10;
/// 投币固定 1 丁尼（DENNY-008）。
pub const GIFT_AMOUNT: i32 = 1;
const GIFT_MESSAGE_MAX_CHARS: usize = 100;
const DEFAULT_PAGE: u32 = 20;
const MAX_PAGE: u32 = 50;

// ---------- 错误 ----------

pub fn insufficient_denny() -> Problem {
    Problem::new(
        StatusCode::PAYMENT_REQUIRED,
        "INSUFFICIENT_DENNY",
        "丁尼余额不足",
    )
}

fn already_checked_in() -> Problem {
    Problem::conflict(
        "CHECK_IN_ALREADY_TODAY",
        "今天已经签到过了，明天 4 点后再来吧",
    )
}

fn already_gifted() -> Problem {
    Problem::conflict("DENNY_ALREADY_GIVEN", "你已经给这篇委托投过丁尼了")
}

pub fn self_gift() -> Problem {
    Problem::new(
        StatusCode::FORBIDDEN,
        "DENNY_SELF_GIVE",
        "不能给自己的委托投丁尼",
    )
}

// ---------- 经验 ----------

/// 经验动作（EXP 动作总表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpAction {
    CheckIn,
    CreatePost,
    CreateComment,
    LikeGive,
    CommentAccepted,
    DennyReceived,
    ExamPassed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Active,
    Passive,
    Exempt,
}

impl Bucket {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Passive => "passive",
            Self::Exempt => "exempt",
        }
    }

    const fn cap(self) -> Option<i32> {
        match self {
            Self::Active => Some(level::ACTIVE_DAILY_CAP),
            Self::Passive => Some(level::PASSIVE_DAILY_CAP),
            Self::Exempt => None,
        }
    }
}

impl ExpAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CheckIn => "check_in",
            Self::CreatePost => "create_post",
            Self::CreateComment => "create_comment",
            Self::LikeGive => "like_give",
            Self::CommentAccepted => "comment_accepted",
            Self::DennyReceived => "denny_received",
            Self::ExamPassed => "exam_passed",
        }
    }

    /// 每日上限桶（EXP-009）：被投币属于被动桶；考试奖励豁免；其余（含被采纳，D-050 方案 A）属于主动桶。
    const fn bucket(self) -> Bucket {
        match self {
            Self::DennyReceived => Bucket::Passive,
            Self::ExamPassed => Bucket::Exempt,
            Self::CheckIn
            | Self::CreatePost
            | Self::CreateComment
            | Self::LikeGive
            | Self::CommentAccepted => Bucket::Active,
        }
    }
}

/// 去重方式（EXP-008）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dedup {
    /// 每个业务日只有第一次发放。
    Daily,
    /// 同一实体终身只发一次，例如 `comment:<id>`。
    Ref(String),
}

impl Dedup {
    fn key(&self, day: NaiveDate) -> String {
        match self {
            Self::Daily => format!("day:{day}"),
            Self::Ref(key) => format!("ref:{key}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpOutcome {
    /// 实际发放量（去重命中或额度用完时为 0）。
    pub added: i32,
    /// 去重命中：之前已经发过（D-064 方案 A：调用方应视为成功）。
    pub duplicate: bool,
    pub exp: i64,
    pub level: i16,
}

/// 锁住用户行与经济行。账号不存在或非活跃时返回 None。
async fn lock_account(
    conn: &mut PgConnection,
    user_id: i64,
) -> Result<Option<(LockedUser, EconomyRow)>, Problem> {
    let Some(user) = interop::lock_active_user(conn, user_id).await? else {
        return Ok(None);
    };
    repo::ensure_row(conn, user_id).await?;
    let row = repo::lock_row(conn, user_id).await?;
    Ok(Some((user, row)))
}

/// 按当前经验与考试状态写回等级，返回新等级。
async fn sync_level(conn: &mut PgConnection, user: &LockedUser, exp: i64) -> Result<i16, Problem> {
    let level = level::effective_level(exp, user.exam_exempt());
    if level != user.level {
        interop::set_level(conn, user.id, level).await?;
    }
    Ok(level)
}

/// 发放经验。账号不可用时返回 None（经验是附带奖励，不阻断主操作）。
///
/// 发放量 = min(amount, 当日该桶剩余额度)；发放量为 0 时不写流水，所以按日去重的动作
/// 被上限挡住时不会占掉「今日首次」的名额（EXP-010）。
pub async fn grant_exp(
    conn: &mut PgConnection,
    ids: &Snowflake,
    user_id: i64,
    action: ExpAction,
    amount: i32,
    dedup: Dedup,
    now: DateTime<Utc>,
) -> Result<Option<ExpOutcome>, Problem> {
    let Some((user, row)) = lock_account(conn, user_id).await? else {
        return Ok(None);
    };
    let day = time::biz_day(now);
    let dedup_key = dedup.key(day);
    let unchanged = |duplicate| ExpOutcome {
        added: 0,
        duplicate,
        exp: row.exp,
        level: user.level,
    };
    if repo::exp_log_exists(conn, user_id, action.as_str(), &dedup_key).await? {
        return Ok(Some(unchanged(true)));
    }
    let bucket = action.bucket();
    let requested = amount.max(0);
    let granted = match bucket.cap() {
        Some(cap) => {
            let used = repo::exp_gained_in_bucket(conn, user_id, day, bucket.as_str()).await?;
            let remaining = (i64::from(cap) - used).max(0);
            i32::try_from(remaining.min(i64::from(requested))).unwrap_or(0)
        }
        None => requested,
    };
    if granted == 0 {
        return Ok(Some(unchanged(false)));
    }
    repo::insert_exp_log(
        conn,
        &NewExpLog {
            id: ids.next_raw(),
            user_id,
            action: action.as_str(),
            amount: granted,
            requested,
            bucket: bucket.as_str(),
            biz_day: day,
            dedup_key: &dedup_key,
            now,
        },
    )
    .await?;
    let exp = row.exp.saturating_add(i64::from(granted));
    repo::set_exp(conn, user_id, exp, now).await?;
    let level = sync_level(conn, &user, exp).await?;
    Ok(Some(ExpOutcome {
        added: granted,
        duplicate: false,
        exp,
        level,
    }))
}

/// 按当前经验与考试状态重算并写回等级（例如刚通过考试，EXP-003）。账号不可用时返回 None。
pub async fn refresh_level(conn: &mut PgConnection, user_id: i64) -> Result<Option<i16>, Problem> {
    let Some((user, row)) = lock_account(conn, user_id).await? else {
        return Ok(None);
    };
    Ok(Some(sync_level(conn, &user, row.exp).await?))
}

// ---------- 丁尼 ----------

/// 丁尼动作（DENNY 动作总表；删帖扣费已取消，D-007）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DennyAction {
    CheckIn,
    GiftSent,
    GiftReceived,
    SpendRename,
    SpendAvatar,
    ExamPassed,
    AdminGrant,
}

impl DennyAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CheckIn => "check_in",
            Self::GiftSent => "gift_sent",
            Self::GiftReceived => "gift_received",
            Self::SpendRename => "spend_rename",
            Self::SpendAvatar => "spend_avatar",
            Self::ExamPassed => "exam_passed",
            Self::AdminGrant => "admin_grant",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DennyOutcome {
    /// 实际变动量（入账可能因封顶小于请求量；出账为负数）。
    pub delta: i32,
    /// 幂等键命中：之前已经入账过。
    pub duplicate: bool,
    pub balance: i32,
}

fn clip_description(text: &str) -> String {
    text.chars().take(255).collect()
}

/// 入账。按 `(用户, 动作, ref_key)` 幂等（DENNY-005）；封顶 9999，实际增加量为 0 时不写流水（DENNY-006）。
/// 账号不可用时返回 None。
#[allow(clippy::too_many_arguments)]
pub async fn add_denny(
    conn: &mut PgConnection,
    ids: &Snowflake,
    user_id: i64,
    action: DennyAction,
    amount: i32,
    ref_key: Option<&str>,
    description: &str,
    now: DateTime<Utc>,
) -> Result<Option<DennyOutcome>, Problem> {
    let Some((_, row)) = lock_account(conn, user_id).await? else {
        return Ok(None);
    };
    credit(
        conn,
        ids,
        user_id,
        &row,
        action,
        amount,
        ref_key,
        description,
        now,
    )
    .await
    .map(Some)
}

/// 对已锁住的经济行入账。
#[allow(clippy::too_many_arguments)]
async fn credit(
    conn: &mut PgConnection,
    ids: &Snowflake,
    user_id: i64,
    row: &EconomyRow,
    action: DennyAction,
    amount: i32,
    ref_key: Option<&str>,
    description: &str,
    now: DateTime<Utc>,
) -> Result<DennyOutcome, Problem> {
    let unchanged = |duplicate| DennyOutcome {
        delta: 0,
        duplicate,
        balance: row.denny_balance,
    };
    if let Some(key) = ref_key
        && repo::denny_log_exists(conn, user_id, action.as_str(), key).await?
    {
        return Ok(unchanged(true));
    }
    let current = row.denny_balance.min(MAX_BALANCE);
    let balance = current.saturating_add(amount.max(0)).min(MAX_BALANCE);
    let delta = balance - current;
    if delta == 0 {
        return Ok(unchanged(false));
    }
    repo::insert_denny_log(
        conn,
        &NewDennyLog {
            id: ids.next_raw(),
            user_id,
            action: action.as_str(),
            amount: delta,
            balance,
            ref_key,
            description: &clip_description(description),
            now,
        },
    )
    .await?;
    repo::set_balance(conn, user_id, balance, 0, now).await?;
    Ok(DennyOutcome {
        delta,
        duplicate: false,
        balance,
    })
}

/// 扣费（改名、自定义头像等）。余额不足 → 402 `INSUFFICIENT_DENNY`；账号不可用 → 403 `ACCOUNT_BLOCKED`。
pub async fn spend_denny(
    conn: &mut PgConnection,
    ids: &Snowflake,
    user_id: i64,
    action: DennyAction,
    amount: i32,
    description: &str,
    now: DateTime<Utc>,
) -> Result<DennyOutcome, Problem> {
    let (_, row) = lock_account(conn, user_id)
        .await?
        .ok_or_else(ik_auth::service::account_blocked)?;
    debit(
        conn,
        ids,
        user_id,
        &row,
        action,
        amount,
        0,
        None,
        description,
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn debit(
    conn: &mut PgConnection,
    ids: &Snowflake,
    user_id: i64,
    row: &EconomyRow,
    action: DennyAction,
    amount: i32,
    given_delta: i32,
    ref_key: Option<&str>,
    description: &str,
    now: DateTime<Utc>,
) -> Result<DennyOutcome, Problem> {
    let amount = amount.max(1);
    if row.denny_balance < amount {
        return Err(insufficient_denny());
    }
    let balance = row.denny_balance - amount;
    repo::insert_denny_log(
        conn,
        &NewDennyLog {
            id: ids.next_raw(),
            user_id,
            action: action.as_str(),
            amount: -amount,
            balance,
            ref_key,
            description: &clip_description(description),
            now,
        },
    )
    .await?;
    repo::set_balance(conn, user_id, balance, given_delta, now).await?;
    Ok(DennyOutcome {
        delta: -amount,
        duplicate: false,
        balance,
    })
}

// ---------- 投币 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GiftOutcome {
    /// 投币者投币后的余额。
    pub balance: i32,
}

/// 给帖子投 1 丁尼（DENNY-008/009）。调用方（content 域）负责：锁住帖子行、确认帖子公开、
/// 非匿名、不是自己的帖子，并在同一事务里把帖子的 `denny_count` +1。
///
/// 在同一事务内：按 id 升序锁双方 → 写投币记录（每人每帖一次）→ 投币者 -1、累计投出 +1
/// → 作者 +1（封顶 9999）→ 作者获得 1 点被动经验（DENNY-012）。
pub async fn gift_post(
    conn: &mut PgConnection,
    ids: &Snowflake,
    giver_id: i64,
    author_id: i64,
    post_id: i64,
    message: &str,
    now: DateTime<Utc>,
) -> Result<GiftOutcome, Problem> {
    if giver_id == author_id {
        return Err(self_gift());
    }
    let message = message.trim();
    if message.chars().count() > GIFT_MESSAGE_MAX_CHARS {
        return Err(Problem::field(
            "message",
            "TOO_LONG",
            format!("留言最多 {GIFT_MESSAGE_MAX_CHARS} 个字"),
        ));
    }
    // 双方按 id 升序加锁，避免两人互相投币时死锁。
    let (first, second) = if giver_id < author_id {
        (giver_id, author_id)
    } else {
        (author_id, giver_id)
    };
    let a = lock_account(conn, first).await?;
    let b = lock_account(conn, second).await?;
    let (giver, author) = if giver_id < author_id { (a, b) } else { (b, a) };
    let (_, giver_row) = giver.ok_or_else(ik_auth::service::account_blocked)?;
    // 作者已注销或被封禁：不再收币。
    let (_, author_row) = author.ok_or_else(Problem::not_found)?;

    if giver_row.denny_balance < GIFT_AMOUNT {
        return Err(insufficient_denny());
    }
    if !repo::insert_gift(conn, post_id, giver_id, message, now).await? {
        return Err(already_gifted());
    }
    let sent = debit(
        conn,
        ids,
        giver_id,
        &giver_row,
        DennyAction::GiftSent,
        GIFT_AMOUNT,
        GIFT_AMOUNT,
        Some(&format!("post:{post_id}")),
        &format!("投币给委托 {post_id}"),
        now,
    )
    .await?;
    credit(
        conn,
        ids,
        author_id,
        &author_row,
        DennyAction::GiftReceived,
        GIFT_AMOUNT,
        Some(&format!("post:{post_id}:from:{giver_id}")),
        &format!("收到委托投币 {post_id}"),
        now,
    )
    .await?;
    grant_exp(
        conn,
        ids,
        author_id,
        ExpAction::DennyReceived,
        1,
        Dedup::Ref(format!("gift:{giver_id}:post:{post_id}")),
        now,
    )
    .await?;
    Ok(GiftOutcome {
        balance: sent.balance,
    })
}

/// 当前用户投过币的帖子（批量查询，用于详情页按钮状态）。
pub async fn gifted_posts(
    conn: &mut PgConnection,
    giver_id: i64,
    post_ids: &[i64],
) -> Result<Vec<i64>, Problem> {
    if post_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(repo::gifted_post_ids(conn, giver_id, post_ids).await?)
}

// ---------- 签到 ----------

/// 现在签到会得到的连签天数：上次签到正好是前一个签到日则 +1，否则从 1 开始（CHECKIN-003）。
fn next_streak(last: Option<NaiveDate>, stored: i32, today: NaiveDate) -> i32 {
    match last {
        Some(day) if today.pred_opt() == Some(day) => stored.saturating_add(1),
        _ => 1,
    }
}

/// 签到：每个业务日一次（北京时间 04:00 切日，D-001 方案 A）。
/// 签到状态、排名、经验、丁尼在同一事务内完成（CHECKIN-004）。
pub async fn check_in(
    state: &EconomyState,
    principal: &Principal,
) -> Result<CheckInResult, Problem> {
    principal.require_write()?;
    let user_id = principal.user_id.get();
    let now = Utc::now();
    let today = time::biz_day(now);
    let next_eligible_at = time::biz_day_start(today) + Duration::days(1);

    let mut tx = state.db.begin().await?;
    // 账号在事务里被封禁时返回 403（D-052 方案 A）。
    let (_, row) = lock_account(&mut tx, user_id)
        .await?
        .ok_or_else(ik_auth::service::account_blocked)?;
    if row.last_check_in_day == Some(today) {
        return Err(already_checked_in());
    }
    let streak = next_streak(row.last_check_in_day, row.check_in_streak, today);
    repo::record_check_in_state(&mut tx, user_id, today, streak, now).await?;
    let rank = repo::next_check_in_rank(&mut tx, today).await?;

    let exp = grant_exp(
        &mut tx,
        &state.ids,
        user_id,
        ExpAction::CheckIn,
        level::check_in_exp(streak),
        Dedup::Daily,
        now,
    )
    .await?
    .ok_or_else(ik_auth::service::account_blocked)?;
    let denny = add_denny(
        &mut tx,
        &state.ids,
        user_id,
        DennyAction::CheckIn,
        CHECK_IN_DENNY,
        Some(&format!("day:{today}")),
        &format!("每日签到奖励 - 第 {streak} 天"),
        now,
    )
    .await?
    .ok_or_else(ik_auth::service::account_blocked)?;
    repo::insert_check_in(
        &mut tx,
        &NewCheckIn {
            user_id,
            day: today,
            rank,
            streak,
            exp_granted: exp.added,
            denny_granted: denny.delta,
            now,
        },
    )
    .await?;
    tx.commit().await?;

    Ok(CheckInResult {
        day: today,
        streak,
        total_days: row.check_in_total.saturating_add(1),
        rank,
        exp_gained: exp.added,
        denny_gained: denny.delta,
        exp: exp.exp,
        level: exp.level,
        balance: denny.balance,
        next_eligible_at,
    })
}

pub async fn check_in_status(
    state: &EconomyState,
    principal: &Principal,
) -> Result<CheckInStatusView, Problem> {
    let user_id = principal.user_id.get();
    let now = Utc::now();
    let today = time::biz_day(now);
    let mut conn = state.db.acquire().await?;
    let row = repo::get_row(&mut conn, user_id).await?;
    let (last, stored, total, balance) = row.map_or((None, 0, 0, INITIAL_BALANCE), |r| {
        (
            r.last_check_in_day,
            r.check_in_streak,
            r.check_in_total,
            r.denny_balance,
        )
    });
    let done = last == Some(today);
    let rank = if done {
        repo::check_in_rank(&mut conn, user_id, today).await?
    } else {
        None
    };
    let next = if done {
        stored
    } else {
        next_streak(last, stored, today)
    };
    Ok(CheckInStatusView {
        can_check_in: !done,
        day: today,
        total_days: total,
        streak: stored,
        rank,
        next_streak: next,
        next_exp: level::check_in_exp(next),
        next_denny: CHECK_IN_DENNY,
        next_eligible_at: done.then(|| time::biz_day_start(today) + Duration::days(1)),
        balance,
    })
}

// ---------- 查询 ----------

pub async fn wallet(state: &EconomyState, principal: &Principal) -> Result<WalletView, Problem> {
    let mut conn = state.db.acquire().await?;
    let row = repo::get_row(&mut conn, principal.user_id.get()).await?;
    Ok(row.map_or(
        WalletView {
            balance: INITIAL_BALANCE,
            given: 0,
        },
        |r| WalletView {
            balance: r.denny_balance,
            given: r.denny_given,
        },
    ))
}

#[derive(Serialize, Deserialize)]
struct LogCursor {
    c: DateTime<Utc>,
    i: i64,
}

pub async fn denny_logs(
    state: &EconomyState,
    principal: &Principal,
    query: PageQuery,
) -> Result<Page<DennyLogView>, Problem> {
    let limit = i64::from(query.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE));
    let after = query
        .cursor
        .as_deref()
        .map(cursor::decode::<LogCursor>)
        .transpose()?
        .map(|c| (c.c, c.i));
    let mut conn = state.db.acquire().await?;
    let rows = repo::denny_logs_page(&mut conn, principal.user_id.get(), after, limit + 1).await?;
    let has_more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    let next_cursor = match (has_more, rows.get(usize::try_from(limit - 1).unwrap_or(0))) {
        (true, Some(last)) => Some(cursor::encode(&LogCursor {
            c: last.created_at,
            i: last.id,
        })?),
        _ => None,
    };
    Ok(Page {
        items: rows
            .into_iter()
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .map(|r| DennyLogView {
                action: r.action,
                amount: r.amount,
                balance: r.balance,
                description: r.description,
                created_at: r.created_at,
            })
            .collect(),
        next_cursor,
    })
}

pub async fn exp(state: &EconomyState, principal: &Principal) -> Result<ExpView, Problem> {
    let user_id = principal.user_id.get();
    let today = time::biz_day(Utc::now());
    let mut conn = state.db.acquire().await?;
    let total = repo::get_row(&mut conn, user_id)
        .await?
        .map_or(0, |r| r.exp);
    let by_action = repo::exp_by_action(&mut conn, user_id, today).await?;
    let source = |action: ExpAction| {
        by_action
            .iter()
            .find(|s| s.action == action.as_str())
            .map_or(ExpSource::default(), |s| ExpSource {
                done: s.amount > 0,
                exp: s.amount,
            })
    };
    let gained = |bucket: Bucket| -> i64 {
        by_action
            .iter()
            .filter(|s| bucket_of(&s.action) == Some(bucket))
            .map(|s| s.amount)
            .sum()
    };
    let level = principal.level;
    Ok(ExpView {
        exp: total,
        level,
        next_level_exp: if level == 0 {
            None
        } else {
            level::next_level_exp(level)
        },
        today,
        today_active_gained: gained(Bucket::Active),
        today_active_cap: level::ACTIVE_DAILY_CAP,
        today_passive_gained: gained(Bucket::Passive),
        today_passive_cap: level::PASSIVE_DAILY_CAP,
        sources: ExpSources {
            check_in: source(ExpAction::CheckIn),
            create_post: source(ExpAction::CreatePost),
            create_comment: source(ExpAction::CreateComment),
            like_give: source(ExpAction::LikeGive),
            comment_accepted: source(ExpAction::CommentAccepted),
        },
    })
}

/// 流水里的动作属于哪个桶（含迁移来的历史被动动作）。
fn bucket_of(action: &str) -> Option<Bucket> {
    match action {
        "denny_received" | "like_received" | "comment_received" | "favorite_received" => {
            Some(Bucket::Passive)
        }
        "exam_passed" => Some(Bucket::Exempt),
        "check_in" | "create_post" | "create_comment" | "like_give" | "comment_accepted" => {
            Some(Bucket::Active)
        }
        _ => None,
    }
}

/// 等级权益（BENEFIT-004），未登录按 0 级。
pub fn benefits_for(viewer: Option<&Principal>) -> BenefitsView {
    let level = viewer.map_or(0, Principal::benefit_level);
    let next = (level < benefits::MAX_BENEFIT_LEVEL).then_some(level + 1);
    BenefitsView {
        level: i16::try_from(level).unwrap_or(0),
        max_level: level::MAX_LEVEL,
        benefits: benefits::for_level(level),
        next_level: next.and_then(|n| i16::try_from(n).ok()),
        next_benefits: next.map(benefits::for_level),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn streak_continues_only_from_yesterday() {
        let today = d(2026, 10, 14);
        assert_eq!(next_streak(Some(d(2026, 10, 13)), 4, today), 5);
        assert_eq!(next_streak(Some(d(2026, 10, 12)), 4, today), 1);
        assert_eq!(next_streak(None, 0, today), 1);
    }

    #[test]
    fn dedup_keys_are_namespaced() {
        let day = d(2026, 10, 14);
        assert_eq!(Dedup::Daily.key(day), "day:2026-10-14");
        assert_eq!(Dedup::Ref("comment:9".into()).key(day), "ref:comment:9");
    }

    #[test]
    fn buckets_follow_rules() {
        assert_eq!(ExpAction::CommentAccepted.bucket(), Bucket::Active);
        assert_eq!(ExpAction::DennyReceived.bucket(), Bucket::Passive);
        assert_eq!(ExpAction::ExamPassed.bucket(), Bucket::Exempt);
        for action in [
            ExpAction::CheckIn,
            ExpAction::CreatePost,
            ExpAction::CreateComment,
            ExpAction::LikeGive,
            ExpAction::CommentAccepted,
            ExpAction::DennyReceived,
            ExpAction::ExamPassed,
        ] {
            assert_eq!(bucket_of(action.as_str()), Some(action.bucket()));
        }
    }
}
