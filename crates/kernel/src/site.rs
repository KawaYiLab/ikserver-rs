//! 后台「系统设置」里可以在线修改的站点参数（不含任何密钥与部署相关的项）。
//!
//! 缺省值来自部署配置（`IK_*` 环境变量，见 [`crate::config`]）；后台改过的项存在
//! `site_settings.overrides`（只存与缺省值不同的叶子）。进程内缓存 60 秒，本实例写入后立即失效，
//! 多实例部署时其他实例最长 60 秒后生效。
//!
//! 读取失败或库里的值不合法时沿用上一份（没有就用缺省值），不会因为设置表出问题而放开门控。

use std::sync::Arc;
use std::time::{Duration, Instant};

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
    pub registration: Registration,
    pub exam: ExamParams,
    pub hot: HotParams,
    pub moderation: ModerationParams,
    pub rate_limit: RateLimitParams,
}

/// 注册与登录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    /// 注册总开关（REG-002）。关闭时邮箱注册与米游社首登建号都被拒绝，已有账号不受影响。
    pub open: bool,
    /// 米游社扫码登录与绑定。关闭时扫码接口返回 503 `MIHOYO_DISABLED`。
    pub mihoyo_login: bool,
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
            registration: Registration {
                open: config.auth.registration_open,
                mihoyo_login: config.mihoyo.enabled,
            },
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
    /// 同一时间只让一个请求去查库，其余的用上一份。
    loading: tokio::sync::Mutex<()>,
}

impl Site {
    pub fn new(db: PgPool, defaults: SiteValues) -> Self {
        Self {
            inner: Arc::new(Inner {
                db,
                defaults: Arc::new(defaults),
                cache: Mutex::new(None),
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
        let Ok(_guard) = self.inner.loading.try_lock() else {
            // 别的请求正在查库：先用上一份。
            return self.last();
        };
        if let Some(values) = self.fresh() {
            return values;
        }
        let values = match self.load().await {
            Ok(values) => Arc::new(values),
            Err(error) => {
                tracing::warn!(%error, "读取站点设置失败，沿用上一份");
                self.last()
            }
        };
        *self.inner.cache.lock() = Some((Instant::now(), values.clone()));
        values
    }

    /// 本实例的缓存立即失效（后台保存后调用）。
    pub fn invalidate(&self) {
        *self.inner.cache.lock() = None;
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
    fn validate_names_bad_fields() {
        let mut values = defaults();
        values.hot.count = 0;
        values.rate_limit.user_write_per_min = 0;
        let fields: Vec<_> = values.validate().into_iter().map(|e| e.field).collect();
        assert_eq!(fields, ["hot.count", "rateLimit.userWritePerMin"]);
    }
}
