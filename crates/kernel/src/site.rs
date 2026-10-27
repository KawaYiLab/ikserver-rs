//! 后台「系统设置」里可以在线修改的站点参数（不含任何密钥与部署相关的项）。
//!
//! 缺省值来自部署配置（`IK_*` 环境变量，见 [`crate::config`]）；后台改过的项存在
//! `site_settings.overrides`（只存与缺省值不同的叶子）。进程内缓存 60 秒，本实例写入后立即失效，
//! 多实例部署时其他实例最长 60 秒后生效。
//!
//! 读取失败或库里的值不合法时沿用上一份（没有就用缺省值），不会因为设置表出问题而放开门控。

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{PgConnection, PgPool};
use utoipa::ToSchema;

use crate::FieldError;
use crate::config::{self, Config};

pub const CACHE_TTL: Duration = Duration::from_secs(60);

/// 全部可在线修改的参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SiteValues {
    pub announcement: Announcement,
    pub read_only: ReadOnly,
    pub registration: Registration,
    pub new_user: NewUserLimits,
    pub filter: ContentFilter,
    pub ip_bans: Vec<IpBan>,
    pub exam: ExamParams,
    pub hot: HotParams,
    pub moderation: ModerationParams,
    pub rate_limit: RateLimitParams,
}

/// 全站公告横幅（主站顶部）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Announcement {
    pub enabled: bool,
    /// 纯文本，最多 300 字。
    pub text: String,
    /// 点击后打开的地址（`http(s)://`），空为不跳转。
    pub link: String,
    pub tone: AnnouncementTone,
    /// 只在这段时间里显示；都不设时一直显示。
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnouncementTone {
    #[default]
    Info,
    Warning,
}

impl Announcement {
    /// 现在是否该显示。
    pub fn active_at(&self, now: DateTime<Utc>) -> bool {
        self.enabled
            && !self.text.trim().is_empty()
            && self.starts_at.is_none_or(|at| at <= now)
            && self.ends_at.is_none_or(|at| now < at)
    }
}

/// 只读维护模式：除登录、登出外的写请求返回 503 `READ_ONLY`；版主与管理员不受影响。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadOnly {
    pub enabled: bool,
    /// 给用户看的说明，最多 200 字；空时用默认提示。
    pub message: String,
}

/// 注册与登录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    /// 注册总开关（REG-002）。关闭时邮箱注册与米游社首登建号都被拒绝，已有账号不受影响。
    pub open: bool,
    /// 米游社扫码登录与绑定。关闭时扫码接口返回 503 `MIHOYO_DISABLED`。
    pub mihoyo_login: bool,
    /// 不允许注册的邮箱域名（小写，也拦它的子域名），例如 `mailinator.com`。
    pub email_domain_blocklist: Vec<String>,
    /// 用户不能改成的用户名（不区分大小写，整名匹配），例如 `admin`、`官方`。后台改名不受限制。
    pub reserved_usernames: Vec<String>,
}

/// 新注册账号在头几个小时里的限制（参考 Discourse 的信任等级 0）。版主、管理员与 AI 账号不受限制。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewUserLimits {
    /// 注册后多少小时内算新账号；0 关闭全部限制。
    pub restrict_hours: u32,
    /// 新账号 24 小时内最多发布几篇帖子；0 不限。
    pub posts_per_day: u32,
    /// 新账号 24 小时内最多发几条评论；0 不限。
    pub comments_per_day: u32,
    /// 新账号能否在帖子、评论里贴外部链接。
    pub allow_links: bool,
}

/// 屏蔽词（参考 Discourse 的 watched words）：作用于帖子标题与正文、评论、改名。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContentFilter {
    pub words: Vec<WatchedWord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WatchedWord {
    /// 按字面匹配，英文字母不区分大小写。
    pub word: String,
    pub action: WordAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WordAction {
    /// 拒绝提交，提示命中的词。
    Block,
    /// 替换成等长的 `*` 后照常保存。
    Censor,
}

/// IP 封禁：命中的地址不能注册、登录与执行写操作（版主、管理员登录后不受影响）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpBan {
    /// 单个地址或网段，例如 `203.0.113.7`、`203.0.113.0/24`、`2001:db8::/32`。
    pub cidr: String,
    pub note: String,
    /// 到期后自动失效；空为永久。
    pub expires_at: Option<DateTime<Utc>>,
}

/// 屏蔽词检查的结果。
#[derive(Debug, PartialEq, Eq)]
pub enum Screened {
    /// 可以保存（命中替换词时已替换）。
    Clean(String),
    /// 命中拦截词。
    Blocked(String),
}

/// 入站考试参数（EXAM-004）。改动只影响之后开始的考试。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExamParams {
    pub question_count: u32,
    pub pass_score_percent: u32,
    pub time_limit_secs: i64,
    pub max_fails_before_cooldown: u32,
    pub fail_cooldown_secs: i64,
    pub reward_denny: i32,
    pub reward_exp: i32,
}

/// 热榜参数（HOT-001）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HotParams {
    pub enabled: bool,
    pub count: usize,
    pub feed_size: usize,
    pub refresh_secs: u64,
    pub gravity: f64,
    pub recent_days: u32,
    pub candidates: i64,
    pub max_per_author: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModerationParams {
    /// 同一内容收到这么多个不同的人的待处理举报时自动隐藏（RPT-052）。
    pub auto_hide_threshold: i64,
}

/// 全站默认限流（每分钟，RL-002）。改动后计数从零开始。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitParams {
    pub enabled: bool,
    pub anon_read_per_min: u32,
    pub anon_write_per_min: u32,
    pub user_read_per_min: u32,
    pub user_write_per_min: u32,
}

impl SiteValues {
    /// 部署配置里的值，作为缺省值。
    pub fn from_config(config: &Config) -> Self {
        Self {
            announcement: Announcement::default(),
            read_only: ReadOnly::default(),
            registration: Registration {
                open: config.auth.registration_open,
                mihoyo_login: config.mihoyo.enabled,
                email_domain_blocklist: Vec::new(),
                reserved_usernames: Vec::new(),
            },
            new_user: NewUserLimits {
                restrict_hours: 0,
                posts_per_day: 3,
                comments_per_day: 20,
                allow_links: true,
            },
            filter: ContentFilter::default(),
            ip_bans: Vec::new(),
            exam: ExamParams::from(&config.exam),
            hot: HotParams::from(&config.hot),
            moderation: ModerationParams {
                auto_hide_threshold: config.moderation.auto_hide_threshold,
            },
            rate_limit: RateLimitParams::from(&config.rate_limit),
        }
    }

    /// 取值范围与部署配置的启动校验一致；返回全部不合法的字段（字段名形如 `hot.count`）。
    pub fn validate(&self) -> Vec<FieldError> {
        let mut errors = Vec::new();
        let mut check = |ok: bool, field: &str, detail: &str| {
            if !ok {
                errors.push(FieldError {
                    field: field.to_owned(),
                    code: "OUT_OF_RANGE".to_owned(),
                    detail: detail.to_owned(),
                });
            }
        };
        let a = &self.announcement;
        check(
            a.text.chars().count() <= 300,
            "announcement.text",
            "公告最多 300 字",
        );
        check(
            !a.enabled || !a.text.trim().is_empty(),
            "announcement.text",
            "开启公告时要填写内容",
        );
        check(
            a.link.is_empty()
                || ((a.link.starts_with("https://") || a.link.starts_with("http://"))
                    && a.link.len() <= 500),
            "announcement.link",
            "链接须以 http:// 或 https:// 开头，最长 500 字符",
        );
        check(
            !matches!((a.starts_at, a.ends_at), (Some(start), Some(end)) if start >= end),
            "announcement.endsAt",
            "结束时间要晚于开始时间",
        );
        check(
            self.read_only.message.chars().count() <= 200,
            "readOnly.message",
            "说明最多 200 字",
        );
        let reg = &self.registration;
        check(
            reg.email_domain_blocklist.len() <= 1000,
            "registration.emailDomainBlocklist",
            "最多 1000 个域名",
        );
        check(
            reg.email_domain_blocklist.iter().all(|d| {
                !d.is_empty()
                    && d.len() <= 253
                    && !d.contains('@')
                    && d.contains('.')
                    && *d == d.trim().to_lowercase()
            }),
            "registration.emailDomainBlocklist",
            "每行一个小写域名，例如 mailinator.com",
        );
        check(
            reg.reserved_usernames.len() <= 1000,
            "registration.reservedUsernames",
            "最多 1000 个",
        );
        check(
            reg.reserved_usernames
                .iter()
                .all(|n| !n.trim().is_empty() && n.chars().count() <= 20),
            "registration.reservedUsernames",
            "每个用户名 1–20 字",
        );
        let nu = &self.new_user;
        check(
            nu.restrict_hours <= 720,
            "newUser.restrictHours",
            "0–720 小时",
        );
        check(nu.posts_per_day <= 1000, "newUser.postsPerDay", "0–1000 篇");
        check(
            nu.comments_per_day <= 10_000,
            "newUser.commentsPerDay",
            "0–10000 条",
        );
        let words = &self.filter.words;
        check(words.len() <= 2000, "filter.words", "最多 2000 个屏蔽词");
        check(
            words
                .iter()
                .all(|w| !w.word.trim().is_empty() && w.word.chars().count() <= 50),
            "filter.words",
            "每个屏蔽词 1–50 字",
        );
        check(self.ip_bans.len() <= 1000, "ipBans", "最多 1000 条");
        check(
            self.ip_bans
                .iter()
                .all(|b| parse_cidr(&b.cidr).is_some() && b.note.chars().count() <= 100),
            "ipBans",
            "地址格式不对（例如 203.0.113.7 或 203.0.113.0/24），备注最多 100 字",
        );
        let exam = &self.exam;
        check(
            (1..=500).contains(&exam.question_count),
            "exam.questionCount",
            "每场题数 1–500",
        );
        check(
            (1..=100).contains(&exam.pass_score_percent),
            "exam.passScorePercent",
            "及格线 1–100",
        );
        check(
            (60..=86_400).contains(&exam.time_limit_secs),
            "exam.timeLimitSecs",
            "时限 60–86400 秒",
        );
        check(
            (1..=100).contains(&exam.max_fails_before_cooldown),
            "exam.maxFailsBeforeCooldown",
            "1–100 次",
        );
        check(
            (0..=604_800).contains(&exam.fail_cooldown_secs),
            "exam.failCooldownSecs",
            "冷却 0–604800 秒",
        );
        check(
            (0..=100_000).contains(&exam.reward_denny),
            "exam.rewardDenny",
            "奖励 0–100000",
        );
        check(
            (0..=100_000).contains(&exam.reward_exp),
            "exam.rewardExp",
            "奖励 0–100000",
        );
        let hot = &self.hot;
        check((1..=100).contains(&hot.count), "hot.count", "展示数 1–100");
        check(
            (1..=2000).contains(&hot.feed_size),
            "hot.feedSize",
            "条数 1–2000",
        );
        check(
            (1..=3600).contains(&hot.refresh_secs),
            "hot.refreshSecs",
            "刷新间隔 1–3600 秒",
        );
        check(
            hot.gravity.is_finite() && hot.gravity > 0.0 && hot.gravity <= 10.0,
            "hot.gravity",
            "衰减指数大于 0、不超过 10",
        );
        check(
            (1..=90).contains(&hot.recent_days),
            "hot.recentDays",
            "统计天数 1–90",
        );
        check(
            (1..=10_000).contains(&hot.candidates),
            "hot.candidates",
            "候选数 1–10000",
        );
        check(
            hot.max_per_author <= 100,
            "hot.maxPerAuthor",
            "0–100（0 表示不限）",
        );
        check(
            (1..=100).contains(&self.moderation.auto_hide_threshold),
            "moderation.autoHideThreshold",
            "阈值 1–100",
        );
        let rl = &self.rate_limit;
        for (value, field) in [
            (rl.anon_read_per_min, "rateLimit.anonReadPerMin"),
            (rl.anon_write_per_min, "rateLimit.anonWritePerMin"),
            (rl.user_read_per_min, "rateLimit.userReadPerMin"),
            (rl.user_write_per_min, "rateLimit.userWritePerMin"),
        ] {
            check((1..=100_000).contains(&value), field, "每分钟 1–100000 次");
        }
        errors
    }

    /// 按屏蔽词检查一段文字。
    pub fn screen(&self, text: &str) -> Screened {
        let folded = text.to_ascii_lowercase();
        let mut censored: Vec<(usize, usize)> = Vec::new();
        for watched in &self.filter.words {
            let needle = watched.word.trim().to_ascii_lowercase();
            if needle.is_empty() {
                continue;
            }
            let hits: Vec<usize> = folded.match_indices(&needle).map(|(at, _)| at).collect();
            if hits.is_empty() {
                continue;
            }
            match watched.action {
                WordAction::Block => return Screened::Blocked(watched.word.trim().to_owned()),
                WordAction::Censor => {
                    censored.extend(hits.into_iter().map(|at| (at, at + needle.len())))
                }
            }
        }
        if censored.is_empty() {
            return Screened::Clean(text.to_owned());
        }
        // ASCII 小写不改变字节位置，所以命中区间可以直接用在原文上。
        let mut out = String::with_capacity(text.len());
        for (at, ch) in text.char_indices() {
            if censored
                .iter()
                .any(|(start, end)| (*start..*end).contains(&at))
            {
                out.push('*');
            } else {
                out.push(ch);
            }
        }
        Screened::Clean(out)
    }

    /// 邮箱域名（或它的上级域名）在注册黑名单里。
    pub fn email_domain_blocked(&self, email: &str) -> bool {
        let Some((_, domain)) = email.rsplit_once('@') else {
            return false;
        };
        let domain = domain.to_lowercase();
        self.registration
            .email_domain_blocklist
            .iter()
            .any(|blocked| domain == *blocked || domain.ends_with(&format!(".{blocked}")))
    }

    /// 用户名是保留名（不区分大小写）。
    pub fn username_reserved(&self, name: &str) -> bool {
        let name = name.trim().to_lowercase();
        self.registration
            .reserved_usernames
            .iter()
            .any(|reserved| reserved.trim().to_lowercase() == name)
    }

    /// 地址命中未到期的 IP 封禁。
    pub fn ip_banned(&self, ip: IpAddr, now: DateTime<Utc>) -> bool {
        self.ip_bans.iter().any(|ban| {
            ban.expires_at.is_none_or(|at| now < at)
                && parse_cidr(&ban.cidr).is_some_and(|net| net.contains(ip))
        })
    }

    /// 与 `defaults` 不同的叶子（数组整体算一个叶子），形如 `{"hot":{"count":6}}`。
    pub fn overrides_against(&self, defaults: &Self) -> Value {
        diff(&to_value(self), &to_value(defaults)).unwrap_or_else(|| Value::Object(Map::new()))
    }

    /// 在缺省值上叠加覆盖项。覆盖项结构不对（字段改过名等）时返回错误，调用方决定怎么兜底。
    pub fn with_overrides(defaults: &Self, overrides: &Value) -> Result<Self, serde_json::Error> {
        let mut merged = to_value(defaults);
        merge(&mut merged, overrides);
        serde_json::from_value(merged)
    }
}

/// 覆盖项里所有叶子的路径（`hot.count`），给后台标「已改动」与写审计用。
pub fn override_paths(overrides: &Value) -> Vec<String> {
    fn walk(prefix: &str, value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    walk(&path, child, out);
                }
            }
            _ if !prefix.is_empty() => out.push(prefix.to_owned()),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk("", overrides, &mut out);
    out
}

/// 文字里有没有外部链接（新账号限制用）。
pub fn has_link(text: &str) -> bool {
    let folded = text.to_ascii_lowercase();
    folded.contains("http://") || folded.contains("https://") || folded.contains("www.")
}

/// 地址或网段。IPv4 映射的 IPv6 地址按 IPv4 比较。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpNet {
    addr: IpAddr,
    prefix: u8,
}

impl IpNet {
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            v4 @ IpAddr::V4(_) => v4,
        };
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

pub fn parse_cidr(raw: &str) -> Option<IpNet> {
    let raw = raw.trim();
    let (addr, prefix) = match raw.split_once('/') {
        Some((addr, prefix)) => (
            addr.parse::<IpAddr>().ok()?,
            Some(prefix.parse::<u8>().ok()?),
        ),
        None => (raw.parse::<IpAddr>().ok()?, None),
    };
    let max = if addr.is_ipv4() { 32 } else { 128 };
    let prefix = prefix.unwrap_or(max);
    // 太宽的网段多半是手误（会把大量正常用户挡在外面）。
    let min = if addr.is_ipv4() { 8 } else { 16 };
    (min..=max)
        .contains(&prefix)
        .then_some(IpNet { addr, prefix })
}

fn to_value(values: &SiteValues) -> Value {
    serde_json::to_value(values).unwrap_or(Value::Null)
}

fn diff(value: &Value, default: &Value) -> Option<Value> {
    match (value, default) {
        (Value::Object(value), Value::Object(default)) => {
            let map: Map<String, Value> = value
                .iter()
                .filter_map(|(key, child)| {
                    let changed = match default.get(key) {
                        Some(base) => diff(child, base)?,
                        None => child.clone(),
                    };
                    Some((key.clone(), changed))
                })
                .collect();
            (!map.is_empty()).then_some(Value::Object(map))
        }
        _ if value == default => None,
        _ => Some(value.clone()),
    }
}

fn merge(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                // 缺省值里没有的键是旧版本留下的，忽略。
                if let Some(slot) = base.get_mut(key) {
                    merge(slot, value);
                }
            }
        }
        (slot, value) => *slot = value.clone(),
    }
}

impl From<&config::Exam> for ExamParams {
    fn from(c: &config::Exam) -> Self {
        Self {
            question_count: c.question_count,
            pass_score_percent: c.pass_score_percent,
            time_limit_secs: c.time_limit_secs,
            max_fails_before_cooldown: c.max_fails_before_cooldown,
            fail_cooldown_secs: c.fail_cooldown_secs,
            reward_denny: c.reward_denny,
            reward_exp: c.reward_exp,
        }
    }
}

impl From<&ExamParams> for config::Exam {
    fn from(p: &ExamParams) -> Self {
        Self {
            question_count: p.question_count,
            pass_score_percent: p.pass_score_percent,
            time_limit_secs: p.time_limit_secs,
            max_fails_before_cooldown: p.max_fails_before_cooldown,
            fail_cooldown_secs: p.fail_cooldown_secs,
            reward_denny: p.reward_denny,
            reward_exp: p.reward_exp,
        }
    }
}

impl From<&config::Hot> for HotParams {
    fn from(c: &config::Hot) -> Self {
        Self {
            enabled: c.enabled,
            count: c.count,
            feed_size: c.feed_size,
            refresh_secs: c.refresh_secs,
            gravity: c.gravity,
            recent_days: c.recent_days,
            candidates: c.candidates,
            max_per_author: c.max_per_author,
        }
    }
}

impl From<&HotParams> for config::Hot {
    fn from(p: &HotParams) -> Self {
        Self {
            enabled: p.enabled,
            count: p.count,
            feed_size: p.feed_size,
            refresh_secs: p.refresh_secs,
            gravity: p.gravity,
            recent_days: p.recent_days,
            candidates: p.candidates,
            max_per_author: p.max_per_author,
        }
    }
}

impl From<&config::RateLimit> for RateLimitParams {
    fn from(c: &config::RateLimit) -> Self {
        Self {
            enabled: c.enabled,
            anon_read_per_min: c.anon_read_per_min,
            anon_write_per_min: c.anon_write_per_min,
            user_read_per_min: c.user_read_per_min,
            user_write_per_min: c.user_write_per_min,
        }
    }
}

/// 站点参数的进程内快照。克隆开销很小（共享同一份缓存）。
#[derive(Clone)]
pub struct Site {
    inner: Arc<Inner>,
}

struct Inner {
    db: PgPool,
    defaults: Arc<SiteValues>,
    cache: Mutex<Option<(Instant, Arc<SiteValues>)>>,
    /// 每次失效加一：查库期间失效了，查到的就是旧值，不能放进缓存。
    generation: AtomicU64,
    /// 同一时间只让一个请求去查库，其余的等它查完。
    loading: tokio::sync::Mutex<()>,
}

impl Site {
    pub fn new(db: PgPool, defaults: SiteValues) -> Self {
        Self {
            inner: Arc::new(Inner {
                db,
                defaults: Arc::new(defaults),
                cache: Mutex::new(None),
                generation: AtomicU64::new(0),
                loading: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// 部署配置里的缺省值。
    pub fn defaults(&self) -> &SiteValues {
        &self.inner.defaults
    }

    /// 当前生效的参数（最长 60 秒前的）。
    pub async fn get(&self) -> Arc<SiteValues> {
        if let Some(values) = self.fresh() {
            return values;
        }
        let _guard = self.inner.loading.lock().await;
        if let Some(values) = self.fresh() {
            return values;
        }
        let generation = self.inner.generation.load(Ordering::Acquire);
        let values = match self.load().await {
            Ok(values) => Arc::new(values),
            Err(error) => {
                tracing::warn!(%error, "读取站点设置失败，沿用上一份");
                self.last()
            }
        };
        let mut cache = self.inner.cache.lock();
        if self.inner.generation.load(Ordering::Acquire) == generation {
            *cache = Some((Instant::now(), values.clone()));
        }
        values
    }

    /// 本实例的缓存立即失效（后台保存后调用）。
    pub fn invalidate(&self) {
        let mut cache = self.inner.cache.lock();
        self.inner.generation.fetch_add(1, Ordering::AcqRel);
        *cache = None;
    }

    fn fresh(&self) -> Option<Arc<SiteValues>> {
        let cache = self.inner.cache.lock();
        cache
            .as_ref()
            .filter(|(at, _)| at.elapsed() < CACHE_TTL)
            .map(|(_, values)| values.clone())
    }

    fn last(&self) -> Arc<SiteValues> {
        self.inner
            .cache
            .lock()
            .as_ref()
            .map_or_else(|| self.inner.defaults.clone(), |(_, values)| values.clone())
    }

    async fn load(&self) -> anyhow::Result<SiteValues> {
        let mut conn = self.inner.db.acquire().await?;
        let overrides = read_overrides(&mut conn).await?;
        let values = SiteValues::with_overrides(&self.inner.defaults, &overrides)?;
        let errors = values.validate();
        anyhow::ensure!(errors.is_empty(), "站点设置不合法：{errors:?}");
        Ok(values)
    }
}

/// 库里保存的覆盖项（缺行时为空对象）。
pub async fn read_overrides(conn: &mut PgConnection) -> sqlx::Result<Value> {
    let row = sqlx::query_scalar!("SELECT overrides FROM site_settings WHERE id = 1")
        .fetch_optional(conn)
        .await?;
    Ok(row.unwrap_or_else(|| Value::Object(Map::new())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn defaults() -> SiteValues {
        SiteValues::from_config(&Config::default())
    }

    #[test]
    fn defaults_are_valid() {
        assert!(defaults().validate().is_empty());
    }

    #[test]
    fn overrides_keep_only_changed_leaves_and_round_trip() {
        let base = defaults();
        let mut changed = base.clone();
        changed.hot.count = 9;
        changed.registration.open = !base.registration.open;
        let overrides = changed.overrides_against(&base);
        assert_eq!(
            overrides,
            json!({ "hot": { "count": 9 }, "registration": { "open": !base.registration.open } })
        );
        let mut paths = override_paths(&overrides);
        paths.sort();
        assert_eq!(paths, ["hot.count", "registration.open"]);
        assert_eq!(
            SiteValues::with_overrides(&base, &overrides).unwrap(),
            changed
        );
        assert_eq!(base.overrides_against(&base), json!({}));
    }

    #[test]
    fn unknown_override_keys_are_ignored() {
        let base = defaults();
        let merged =
            SiteValues::with_overrides(&base, &json!({ "gone": 1, "hot": { "old": 2 } })).unwrap();
        assert_eq!(merged, base);
    }

    #[test]
    fn screen_blocks_and_censors_case_insensitively() {
        let mut values = defaults();
        values.filter.words = vec![
            WatchedWord {
                word: "Spam".to_owned(),
                action: WordAction::Censor,
            },
            WatchedWord {
                word: "代练".to_owned(),
                action: WordAction::Censor,
            },
        ];
        assert_eq!(
            values.screen("买 SPAM 找代练"),
            Screened::Clean("买 **** 找**".to_owned())
        );
        values.filter.words.push(WatchedWord {
            word: "外挂".to_owned(),
            action: WordAction::Block,
        });
        assert_eq!(
            values.screen("出售外挂"),
            Screened::Blocked("外挂".to_owned())
        );
        assert_eq!(
            values.screen("正常内容"),
            Screened::Clean("正常内容".to_owned())
        );
    }

    #[test]
    fn registration_and_ip_rules() {
        let mut values = defaults();
        values.registration.email_domain_blocklist = vec!["spam.com".to_owned()];
        values.registration.reserved_usernames = vec!["Admin".to_owned()];
        assert!(values.email_domain_blocked("a@spam.com"));
        assert!(values.email_domain_blocked("a@mx.SPAM.com"));
        assert!(!values.email_domain_blocked("a@notspam.com"));
        assert!(values.username_reserved(" admin "));
        assert!(!values.username_reserved("admin2"));

        let now = Utc::now();
        values.ip_bans = vec![
            IpBan {
                cidr: "203.0.113.0/24".to_owned(),
                note: String::new(),
                expires_at: None,
            },
            IpBan {
                cidr: "198.51.100.7".to_owned(),
                note: String::new(),
                expires_at: Some(now - chrono::Duration::hours(1)),
            },
            IpBan {
                cidr: "2001:db8::/32".to_owned(),
                note: String::new(),
                expires_at: None,
            },
        ];
        assert!(values.validate().is_empty());
        assert!(values.ip_banned("203.0.113.9".parse().unwrap(), now));
        assert!(values.ip_banned("::ffff:203.0.113.9".parse().unwrap(), now));
        assert!(!values.ip_banned("203.0.114.1".parse().unwrap(), now));
        assert!(
            !values.ip_banned("198.51.100.7".parse().unwrap(), now),
            "已过期"
        );
        assert!(values.ip_banned("2001:db8:1::1".parse().unwrap(), now));
        assert!(parse_cidr("10.0.0.0/4").is_none(), "太宽");
        assert!(parse_cidr("not-an-ip").is_none());
        assert!(has_link("看 https://x.cn") && !has_link("没有链接"));
    }

    #[test]
    fn validate_names_bad_fields() {
        let mut values = defaults();
        values.hot.count = 0;
        values.rate_limit.user_write_per_min = 0;
        let fields: Vec<_> = values.validate().into_iter().map(|e| e.field).collect();
        assert_eq!(fields, ["hot.count", "rateLimit.userWritePerMin"]);
    }
}
