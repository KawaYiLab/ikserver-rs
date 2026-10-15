//! 运行时配置。
//!
//! 来源优先级（后者覆盖前者）：代码默认值 → `IK_` 前缀环境变量（`__` 分层）。
//! 例如 `IK_HTTP__ADDR=0.0.0.0:8080` 对应 `http.addr`。
//! `database.url` 未设置时回退读取 `DATABASE_URL`（与 sqlx 宏共用）。

use std::fmt;
use std::net::SocketAddr;

use figment::Figment;
use figment::providers::{Env, Serialized};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvKind {
    #[default]
    Dev,
    Prod,
}

impl EnvKind {
    pub fn is_prod(self) -> bool {
        matches!(self, Self::Prod)
    }
}

/// 不会出现在 Debug 输出里的敏感字符串。
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub env: EnvKind,
    /// 雪花 ID 的节点号（0–1023），多实例部署时每个实例必须不同。
    pub node_id: u16,
    pub http: Http,
    pub database: Database,
    pub session: Session,
    pub auth: Auth,
    pub mail: Mail,
    pub secrets: Secrets,
    pub log: Log,
    pub exam: Exam,
    pub hot: Hot,
    pub search: Search,
    pub storage: Storage,
    pub jobs: Jobs,
    pub rate_limit: RateLimit,
    pub bus: Bus,
}

/// 事件总线。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Bus {
    /// 多实例部署时打开：实时事件经 PostgreSQL LISTEN/NOTIFY 转给其他实例。单实例不需要。
    pub cluster: bool,
}

/// 全站默认限流（每分钟，RL-002）。读 = GET / HEAD，其余为写。登录用户按用户计，游客按 IP 计。
/// 进程内计数：多实例时每个实例各自计。具体接口另有更严的限流时两者都生效。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimit {
    pub enabled: bool,
    pub anon_read_per_min: u32,
    pub anon_write_per_min: u32,
    pub user_read_per_min: u32,
    pub user_write_per_min: u32,
}

impl Default for RateLimit {
    fn default() -> Self {
        Self {
            enabled: true,
            anon_read_per_min: 300,
            anon_write_per_min: 60,
            user_read_per_min: 600,
            user_write_per_min: 120,
        }
    }
}

/// 后台任务（graphile_worker）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Jobs {
    /// 本实例是否执行任务。多实例时可以只让部分实例执行；入队不受影响。
    pub run_worker: bool,
    /// 同时执行的任务数（1–64）。
    pub concurrency: usize,
}

impl Default for Jobs {
    fn default() -> Self {
        Self {
            run_worker: true,
            concurrency: 4,
        }
    }
}

/// 对象存储（S3 兼容：R2 / 七牛）与直传上传。`bucket` 为空时上传接口返回 503。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Storage {
    /// 例如 `https://<account>.r2.cloudflarestorage.com`。
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub access_key: Secret,
    pub secret_key: Secret,
    /// 对外地址前缀（CDN，例如 `https://im.tiwat.cn`）。为空时用 `{endpoint}/{bucket}`（UPL-009）。
    pub public_base_url: String,
    /// 所有对象 key 的前缀（可空，例如 `uploads`）。
    pub root_prefix: String,
    /// 七牛等需要显式 ACL 的存储设为 true（写入时带 `public-read`）；R2 不支持 ACL，保持 false。
    pub public_read_acl: bool,
    /// 预签名 PUT 与完成令牌的有效期（UPL-011）。
    pub sign_expires_secs: u64,
    /// 同时校验的上传数（每个最多占 30 MiB 内存，D-003 方案 A）。
    pub verify_slots: usize,
    /// 排队等待校验槽位的上限时间（UPL-041）。
    pub queue_timeout_secs: u64,
    /// 单个校验任务的总时限（UPL-041）。
    pub task_timeout_secs: u64,
    /// 单个 S3 请求的超时（UPL-042）。
    pub request_timeout_secs: u64,
    /// 暂存对象（签名后没完成的上传）保留多久后由后台任务清理（UPL-060）。
    pub staging_ttl_hours: u64,
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            region: "auto".to_owned(),
            bucket: String::new(),
            access_key: Secret::default(),
            secret_key: Secret::default(),
            public_base_url: String::new(),
            root_prefix: String::new(),
            public_read_acl: false,
            sign_expires_secs: 600,
            verify_slots: 2,
            queue_timeout_secs: 60,
            task_timeout_secs: 90,
            request_timeout_secs: 15,
            staging_ttl_hours: 24,
        }
    }
}

/// 搜索（Meilisearch）。`url` 为空时不连接搜索引擎，搜索一律走数据库兜底（SRCH-014）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Search {
    /// 例如 `http://127.0.0.1:7700`。
    pub url: Option<String>,
    pub api_key: Secret,
    /// 索引名前缀（区分环境），只允许小写字母、数字、`_`、`-`，最长 32（SRCH-006）。
    pub index_prefix: String,
    /// 单次搜索请求超时（SRCH-012：3000ms）。
    pub timeout_ms: u64,
    /// 增量同步间隔：扫描最近改动的帖子与新评论。
    pub sweep_secs: u64,
    /// 全量对账间隔（启动时也跑一次）：重建全部文档并删除残留。
    pub reconcile_hours: u64,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            url: None,
            api_key: Secret::default(),
            index_prefix: String::new(),
            timeout_ms: 3000,
            sweep_secs: 30,
            reconcile_hours: 24,
        }
    }
}

/// 热榜参数（HOT-001）。启动时校验，不接受 0 或非法值（EDGE-017）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Hot {
    pub enabled: bool,
    /// 首页注入的热门帖数（展示位）。
    pub count: usize,
    /// 热门浏览流（/v1/feed/hot）的最大条数。
    pub feed_size: usize,
    pub refresh_secs: u64,
    /// 时间衰减指数。
    pub gravity: f64,
    /// 统计近期活动的天数窗口。
    pub recent_days: u32,
    /// 按最近活跃取的候选帖数。
    pub candidates: i64,
    /// 展示位里同一作者最多几篇；0 表示不限。匿名帖不参与限流。
    pub max_per_author: usize,
}

impl Default for Hot {
    fn default() -> Self {
        Self {
            enabled: true,
            count: 4,
            feed_size: 200,
            refresh_secs: 600,
            gravity: 1.2,
            recent_days: 7,
            candidates: 1000,
            max_per_author: 1,
        }
    }
}

/// 入站考试参数（EXAM-004）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Exam {
    /// 每场抽题数（题库不足时取全部）。
    pub question_count: u32,
    /// 及格线（百分比，含边界）。
    pub pass_score_percent: u32,
    pub time_limit_secs: i64,
    /// 最近这么多次全部未通过就进入冷却。
    pub max_fails_before_cooldown: u32,
    pub fail_cooldown_secs: i64,
    pub reward_denny: i32,
    pub reward_exp: i32,
}

impl Default for Exam {
    fn default() -> Self {
        Self {
            question_count: 100,
            pass_score_percent: 60,
            time_limit_secs: 3600,
            max_fails_before_cooldown: 3,
            fail_cooldown_secs: 1800,
            reward_denny: 50,
            reward_exp: 50,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            env: EnvKind::Dev,
            node_id: 1,
            http: Http::default(),
            database: Database::default(),
            session: Session::default(),
            auth: Auth::default(),
            mail: Mail::default(),
            secrets: Secrets::default(),
            log: Log::default(),
            exam: Exam::default(),
            hot: Hot::default(),
            search: Search::default(),
            storage: Storage::default(),
            jobs: Jobs::default(),
            rate_limit: RateLimit::default(),
            bus: Bus::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Auth {
    /// 注册总开关（REG-002）。关闭时邮箱注册与米游社首登建号都被拒绝，已有账号不受影响。
    pub registration_open: bool,
}

impl Default for Auth {
    fn default() -> Self {
        Self {
            registration_open: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailProvider {
    /// 只写日志不发信，仅允许在开发环境使用。
    #[default]
    DevLog,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Mail {
    pub provider: MailProvider,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Http {
    pub addr: SocketAddr,
    /// 可信反向代理层数；客户端 IP 从 X-Forwarded-For 右侧数这么多跳取得。
    pub trusted_proxy_hops: u32,
    /// 允许携带凭据的跨域来源（CORS + CSRF 白名单）。
    pub allowed_origins: Vec<String>,
    pub request_timeout_secs: u64,
    pub body_limit_bytes: usize,
}

impl Default for Http {
    fn default() -> Self {
        Self {
            addr: SocketAddr::from(([127, 0, 0, 1], 8080)),
            trusted_proxy_hops: 0,
            allowed_origins: Vec::new(),
            request_timeout_secs: 30,
            body_limit_bytes: 2 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Database {
    pub url: Secret,
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_secs: u64,
}

impl Default for Database {
    fn default() -> Self {
        Self {
            url: Secret::default(),
            max_connections: 20,
            min_connections: 1,
            acquire_timeout_secs: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub cookie_name: String,
    pub cookie_secure: bool,
    /// 滑动有效期：活跃时自动续期到 now + ttl。
    pub ttl_days: i64,
    /// 绝对上限：自创建起最多存活这么久，到期必须重新登录。
    pub max_lifetime_days: i64,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            cookie_name: "ik_session".to_owned(),
            cookie_secure: true,
            ttl_days: 30,
            max_lifetime_days: 180,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Secrets {
    /// 验证码哈希用的 HMAC 密钥（base64，解码后至少 32 字节）。
    pub code_hmac_key: Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Json,
    Pretty,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Log {
    pub format: LogFormat,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("配置解析失败：{0}")]
    Parse(#[from] Box<figment::Error>),
    #[error("缺少必需配置：{0}")]
    Missing(&'static str),
    #[error("配置值不合法：{0}")]
    Invalid(String),
}

impl Config {
    /// 从环境变量加载配置并校验。调用前应先加载 `.env`（由二进制负责）。
    pub fn load() -> Result<Self, ConfigError> {
        Self::from_figment(
            Figment::new()
                .merge(Serialized::defaults(Self::default()))
                .merge(Env::prefixed("IK_").split("__")),
        )
    }

    pub fn from_figment(figment: Figment) -> Result<Self, ConfigError> {
        let mut config: Self = figment.extract().map_err(Box::new)?;
        if config.database.url.is_empty()
            && let Ok(url) = std::env::var("DATABASE_URL")
        {
            config.database.url = Secret::new(url);
        }
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.database.url.is_empty() {
            return Err(ConfigError::Missing("IK_DATABASE__URL 或 DATABASE_URL"));
        }
        if self.node_id > crate::id::MAX_NODE_ID {
            return Err(ConfigError::Invalid(format!(
                "node_id 必须在 0–{} 之间",
                crate::id::MAX_NODE_ID
            )));
        }
        if self.session.ttl_days <= 0 || self.session.max_lifetime_days < self.session.ttl_days {
            return Err(ConfigError::Invalid(
                "session.ttl_days 必须为正，且不大于 session.max_lifetime_days".to_owned(),
            ));
        }
        let exam = &self.exam;
        if exam.question_count == 0
            || exam.pass_score_percent == 0
            || exam.pass_score_percent > 100
            || exam.time_limit_secs <= 0
            || exam.max_fails_before_cooldown == 0
            || exam.fail_cooldown_secs < 0
            || exam.reward_denny < 0
            || exam.reward_exp < 0
        {
            return Err(ConfigError::Invalid(
                "exam 配置不合法：题数、及格线（1–100）、时限、冷却次数必须为正，冷却时长与奖励不能为负"
                    .to_owned(),
            ));
        }
        if !(1..=64).contains(&self.jobs.concurrency) {
            return Err(ConfigError::Invalid(
                "jobs.concurrency 必须在 1–64 之间".to_owned(),
            ));
        }
        let rl = &self.rate_limit;
        if [
            rl.anon_read_per_min,
            rl.anon_write_per_min,
            rl.user_read_per_min,
            rl.user_write_per_min,
        ]
        .iter()
        .any(|n| !(1..=100_000).contains(n))
        {
            return Err(ConfigError::Invalid(
                "rate_limit 的每分钟次数必须在 1–100000 之间".to_owned(),
            ));
        }
        let hot = &self.hot;
        if !(1..=100).contains(&hot.count)
            || !(1..=2000).contains(&hot.feed_size)
            || !(1..=3600).contains(&hot.refresh_secs)
            || !(hot.gravity.is_finite() && hot.gravity > 0.0)
            || !(1..=90).contains(&hot.recent_days)
            || !(1..=10_000).contains(&hot.candidates)
        {
            return Err(ConfigError::Invalid(
                "hot 配置不合法：count 1–100、feed_size 1–2000、refresh_secs 1–3600、gravity > 0、recent_days 1–90、candidates 1–10000"
                    .to_owned(),
            ));
        }
        let search = &self.search;
        let prefix_ok = search.index_prefix.len() <= 32
            && search
                .index_prefix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        if !prefix_ok
            || !(100..=30_000).contains(&search.timeout_ms)
            || !(1..=3600).contains(&search.sweep_secs)
            || !(1..=168).contains(&search.reconcile_hours)
            || search
                .url
                .as_deref()
                .is_some_and(|u| !(u.starts_with("http://") || u.starts_with("https://")))
        {
            return Err(ConfigError::Invalid(
                "search 配置不合法：url 须为 http(s)://，index_prefix 只能含小写字母、数字、_、-（≤32），timeout_ms 100–30000，sweep_secs 1–3600，reconcile_hours 1–168"
                    .to_owned(),
            ));
        }
        let storage = &self.storage;
        let prefix_ok = storage.root_prefix.split('/').all(|seg| {
            seg.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        }) && !storage.root_prefix.starts_with('/')
            && !storage.root_prefix.ends_with('/')
            && !storage.root_prefix.contains("//");
        if !prefix_ok
            || !(60..=3600).contains(&storage.sign_expires_secs)
            || !(1..=32).contains(&storage.verify_slots)
            || !(1..=600).contains(&storage.queue_timeout_secs)
            || !(1..=600).contains(&storage.task_timeout_secs)
            || !(1..=300).contains(&storage.request_timeout_secs)
            || !(1..=720).contains(&storage.staging_ttl_hours)
            || (!storage.bucket.is_empty()
                && !(storage.endpoint.starts_with("http://")
                    || storage.endpoint.starts_with("https://")))
        {
            return Err(ConfigError::Invalid(
                "storage 配置不合法：设置 bucket 时 endpoint 须为 http(s)://；root_prefix 只能是由字母、数字、_、- 组成的路径段；sign_expires_secs 60–3600，verify_slots 1–32，queue/task_timeout_secs 1–600，request_timeout_secs 1–300，staging_ttl_hours 1–720"
                    .to_owned(),
            ));
        }
        // 多实例时各实例的进程内随机密钥不同，一个实例签发的上传凭证在另一个实例校验不过。
        if self.bus.cluster && self.secrets.code_hmac_key.is_empty() {
            return Err(ConfigError::Missing(
                "IK_SECRETS__CODE_HMAC_KEY（bus.cluster 打开时必须配置，且各实例相同）",
            ));
        }
        if self.env.is_prod() {
            if self.secrets.code_hmac_key.is_empty() {
                return Err(ConfigError::Missing("IK_SECRETS__CODE_HMAC_KEY"));
            }
            if !self.session.cookie_secure {
                return Err(ConfigError::Invalid(
                    "生产环境必须开启 session.cookie_secure".to_owned(),
                ));
            }
            if self.mail.provider == MailProvider::DevLog {
                return Err(ConfigError::Invalid(
                    "生产环境不能使用 dev_log 邮件通道（会把验证码写进日志）".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(pairs: &[(&str, &str)]) -> Figment {
        let mut figment = Figment::new().merge(Serialized::defaults(Config::default()));
        for (key, value) in pairs {
            figment = figment.merge(Serialized::default(key, value));
        }
        figment
    }

    #[test]
    fn defaults_apply_when_only_url_given() {
        let config = Config::from_figment(with(&[("database.url", "postgres://x")])).unwrap();
        assert_eq!(config.http.addr.port(), 8080);
        assert_eq!(config.session.cookie_name, "ik_session");
        assert_eq!(config.database.url.expose(), "postgres://x");
    }

    #[test]
    fn prod_rejects_dev_log_mailer() {
        let err = Config::from_figment(with(&[
            ("database.url", "postgres://x"),
            ("env", "prod"),
            ("secrets.code_hmac_key", "a2V5"),
        ]))
        .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn cluster_requires_a_shared_hmac_key() {
        let cluster = Serialized::default("bus.cluster", true);
        let err =
            Config::from_figment(with(&[("database.url", "postgres://x")]).merge(cluster.clone()))
                .unwrap_err();
        assert!(matches!(err, ConfigError::Missing(_)));
        let config = Config::from_figment(
            with(&[
                ("database.url", "postgres://x"),
                ("secrets.code_hmac_key", "a2V5"),
            ])
            .merge(cluster),
        )
        .unwrap();
        assert!(config.bus.cluster);
    }

    #[test]
    fn prod_requires_hmac_key() {
        let err = Config::from_figment(with(&[("database.url", "postgres://x"), ("env", "prod")]))
            .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Missing("IK_SECRETS__CODE_HMAC_KEY")
        ));
    }

    #[test]
    fn storage_config_is_validated_at_startup() {
        let base = || with(&[("database.url", "postgres://x")]);
        assert!(
            Config::from_figment(base())
                .unwrap()
                .storage
                .bucket
                .is_empty()
        );
        for (key, value) in [
            ("storage.root_prefix", "/abs"),
            ("storage.root_prefix", "a/../b"),
            ("storage.bucket", "media"),
        ] {
            let err =
                Config::from_figment(base().merge(Serialized::default(key, value))).unwrap_err();
            assert!(matches!(err, ConfigError::Invalid(_)), "{key}={value}");
        }
        let ok = Config::from_figment(
            base()
                .merge(Serialized::default("storage.bucket", "media"))
                .merge(Serialized::default(
                    "storage.endpoint",
                    "http://127.0.0.1:9000",
                ))
                .merge(Serialized::default("storage.root_prefix", "uploads/v2")),
        )
        .unwrap();
        assert_eq!(ok.storage.verify_slots, 2);
    }

    #[test]
    fn search_config_is_validated_at_startup() {
        let base = || with(&[("database.url", "postgres://x")]);
        assert!(Config::from_figment(base()).unwrap().search.url.is_none());
        for (key, value) in [
            ("search.index_prefix", "Prod/"),
            ("search.url", "localhost:7700"),
        ] {
            let err =
                Config::from_figment(base().merge(Serialized::default(key, value))).unwrap_err();
            assert!(matches!(err, ConfigError::Invalid(_)), "{key}={value}");
        }
        let err = Config::from_figment(base().merge(Serialized::default("search.timeout_ms", 0)))
            .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
        let ok = Config::from_figment(
            base()
                .merge(Serialized::default("search.url", "http://127.0.0.1:7700"))
                .merge(Serialized::default("search.index_prefix", "staging_")),
        )
        .unwrap();
        assert_eq!(ok.search.index_prefix, "staging_");
    }

    #[test]
    fn hot_config_is_validated_at_startup() {
        // 旧系统里 HOT_COUNT=0 会悄悄回落默认值（EDGE-017）；这里直接拒绝启动。
        let base = || with(&[("database.url", "postgres://x")]);
        for (key, value) in [
            ("hot.count", 0_i64),
            ("hot.feed_size", 0),
            ("hot.refresh_secs", 0),
            ("hot.recent_days", 91),
            ("hot.candidates", 0),
        ] {
            let err =
                Config::from_figment(base().merge(Serialized::default(key, value))).unwrap_err();
            assert!(matches!(err, ConfigError::Invalid(_)), "{key}={value}");
        }
        let err = Config::from_figment(base().merge(Serialized::default("hot.gravity", 0.0)))
            .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)), "gravity=0");
        let config =
            Config::from_figment(base().merge(Serialized::default("hot.max_per_author", 0)))
                .unwrap();
        assert_eq!(config.hot.max_per_author, 0, "0 表示不限，是合法值");
    }

    #[test]
    fn secret_is_redacted_in_debug() {
        let secret = Secret::new("hunter2");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
    }
}
