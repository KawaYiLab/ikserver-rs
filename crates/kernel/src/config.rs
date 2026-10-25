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
    pub mihoyo: Mihoyo,
    pub video: Video,
    pub moderation: Moderation,
    pub ai: Ai,
    pub story: Story,
    pub qqbot: Qqbot,
}

/// QQ 官方机器人：凭据只在部署配置里，角色与使用限制在管理后台维护。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Qqbot {
    pub enabled: bool,
    pub sandbox: bool,
    pub apps: std::collections::BTreeMap<String, QqbotApp>,
    pub token_url: String,
    /// 未设置时按 sandbox 选择腾讯的正式或沙箱地址。
    pub api_base: Option<String>,
    /// 仅在配置回调地址时临时打开；校验请求仍严格限制签名内容。
    pub allow_validation: bool,
    pub webhook_rate_per_second: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QqbotApp {
    pub app_id: String,
    pub secret: Secret,
}

impl Default for Qqbot {
    fn default() -> Self {
        Self {
            enabled: false,
            sandbox: false,
            apps: std::collections::BTreeMap::new(),
            token_url: "https://bots.qq.com/app/getAppAccessToken".to_owned(),
            api_base: None,
            allow_validation: false,
            webhook_rate_per_second: 50,
        }
    }
}

impl Qqbot {
    pub fn api_base(&self) -> &str {
        self.api_base.as_deref().unwrap_or(if self.sandbox {
            "https://sandbox.api.sgroup.qq.com"
        } else {
            "https://api.sgroup.qq.com"
        })
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut ids = std::collections::BTreeSet::new();
        if self.enabled && self.apps.is_empty() {
            return Err(ConfigError::Missing("IK_QQBOT__APPS（启用时至少配置一个机器人）"));
        }
        for app in self.apps.values() {
            if app.app_id.is_empty()
                || app.app_id.len() > 32
                || !app.app_id.bytes().all(|b| b.is_ascii_digit())
                || app.secret.expose().trim().is_empty()
                || !ids.insert(&app.app_id)
            {
                return Err(ConfigError::Invalid(
                    "qqbot.apps 的 app_id 必须是唯一的数字字符串（≤32 位），secret 不能为空".to_owned(),
                ));
            }
        }
        let url_ok = |url: &str| url.starts_with("https://") || url.starts_with("http://");
        if !(1..=10000).contains(&self.webhook_rate_per_second)
            || !url_ok(&self.token_url)
            || !url_ok(self.api_base())
        {
            return Err(ConfigError::Invalid(
                "qqbot 的 token_url/api_base 须为 http(s)://，webhook_rate_per_second 必须在 1–10000 之间".to_owned(),
            ));
        }
        Ok(())
    }
}

/// AI 助手（ai-assistant.md）。模型、角色卡、额度上限在数据库里（后台维护）；这里是部署相关的参数与
/// 上游提供方的地址、密钥（不进数据库）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Ai {
    /// OpenAI 兼容的上游，按名字引用（`ai_models.provider`）。环境变量形如
    /// `IK_AI__PROVIDERS__DEEPSEEK__BASE_URL`、`IK_AI__PROVIDERS__DEEPSEEK__API_KEY`。
    /// 也可以在管理后台维护（`ai_providers`）；同名时以这里为准。
    pub providers: std::collections::BTreeMap<String, AiProvider>,
    /// 预占的存活时长，覆盖一次最坏耗时的生成（QUOTA-010，60–7200 秒）。
    pub hold_ttl_secs: u64,
    /// 单条回复的总墙钟预算（GEN-020，10–1800 秒）。
    pub reply_timeout_secs: u64,
    /// 首帧看门狗（GEN-021，0 关闭，否则不超过总预算）。
    pub first_delta_timeout_secs: u64,
    /// 正文增量至少增长这么多字符才推送一次（GEN-004，1–1000）。
    pub stream_min_chars: usize,
    /// 私信历史最多带多少条消息（D-086，1–200）。
    pub max_history_messages: usize,
    /// 一次回复最多几轮工具调用（D-086，不设上限，受总预算约束）；超出后要求模型基于已有结果作答。
    pub max_tool_rounds: u32,
    /// 预占估算与无用量时的估算用的上下文基数（QUOTA-007、GEN-081，0–100000）。
    pub estimated_context_tokens: i64,
    /// 单个任务最多尝试几次（TRIG-033，1–10）。
    pub max_attempts: i16,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AiProvider {
    /// 例如 `https://api.deepseek.com/v1`。
    pub base_url: String,
    pub api_key: Secret,
}

impl Default for Ai {
    fn default() -> Self {
        Self {
            providers: std::collections::BTreeMap::new(),
            hold_ttl_secs: 900,
            reply_timeout_secs: 300,
            first_delta_timeout_secs: 90,
            stream_min_chars: 16,
            max_history_messages: 40,
            max_tool_rounds: 8,
            estimated_context_tokens: 4000,
            max_attempts: 3,
        }
    }
}

/// 举报与审核。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Moderation {
    /// 同一内容收到这么多个不同的人的待处理举报时自动隐藏（RPT-052，1–100）。
    pub auto_hide_threshold: i64,
}

impl Default for Moderation {
    fn default() -> Self {
        Self {
            auto_hide_threshold: 5,
        }
    }
}

/// 米游社扫码登录与绑定（MHY）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Mihoyo {
    /// 关闭时扫码接口返回 503 `MIHOYO_DISABLED`；已有的绑定照常展示与解绑。
    pub enabled: bool,
    pub passport_base_url: String,
    pub takumi_base_url: String,
    /// 上游请求超时（1–60 秒）。
    pub timeout_secs: u64,
}

impl Default for Mihoyo {
    fn default() -> Self {
        Self {
            enabled: true,
            passport_base_url: "https://passport-api.miyoushe.com".to_owned(),
            takumi_base_url: "https://api-takumi.mihoyo.com".to_owned(),
            timeout_secs: 10,
        }
    }
}

/// 外部视频（B 站）信息查询：发帖时取标题、封面、时长。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    pub bilibili_base_url: String,
    /// 上游请求超时（1–30 秒）。保存帖子时会等它，所以不宜太长。
    pub timeout_secs: u64,
}

impl Default for Video {
    fn default() -> Self {
        Self {
            bilibili_base_url: "https://api.bilibili.com".to_owned(),
            timeout_secs: 5,
        }
    }
}

/// 绝区零剧情库（ik-story），以 MCP 提供在 `POST /v1/story/mcp`。
/// 语料由管理员上传到数据库（`PUT /v1/admin/story/corpus`），这里只管访问控制与本地开发。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Story {
    /// 本地语料（开发用）：zzz-story-extractor 的输出目录或语料包文件。设置后启动时用它，不读数据库；
    /// 生产环境留空。
    pub source: String,
    /// MCP 客户端须带 `Authorization: Bearer <access_key>`。
    pub access_key: Secret,
    /// 允许不带密钥访问。默认关闭：既没有密钥也没有公开时，接口拒绝所有请求（游戏文本默认不公开）。
    pub public: bool,
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
            mihoyo: Mihoyo::default(),
            video: Video::default(),
            moderation: Moderation::default(),
            ai: Ai::default(),
            story: Story::default(),
            qqbot: Qqbot::default(),
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
    /// 双通道（MAIL-001～003）：国内邮箱走 SMTP（国内邮件服务商），其余走 Brevo，主通道失败改走另一个。
    /// 至少配置一个通道；只配一个时所有邮件都走它。
    Dual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Mail {
    pub provider: MailProvider,
    /// 主通道失败时是否改走另一个通道（MAIL-003）。
    pub fallback: bool,
    /// 追加的国内邮箱域名（小写，MAIL-001 内置名单之外）。
    pub domestic_domains: Vec<String>,
    pub smtp: Smtp,
    pub brevo: Brevo,
}

impl Default for Mail {
    fn default() -> Self {
        Self {
            provider: MailProvider::DevLog,
            fallback: true,
            domestic_domains: Vec::new(),
            smtp: Smtp::default(),
            brevo: Brevo::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpTls {
    /// 连接即 TLS（通常是 465 端口）。
    #[default]
    Implicit,
    /// 明文连接后必须升级为 TLS（通常是 587 端口），服务器不支持时拒绝发送。
    Starttls,
}

/// SMTP 通道（国内邮箱的主通道，MAIL-006）。`host` 为空表示不启用。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Smtp {
    pub host: String,
    pub port: u16,
    pub tls: SmtpTls,
    pub username: String,
    pub password: Secret,
    /// 发件人，`绳网 <noreply@example.com>` 或纯地址。
    pub from: String,
    /// 回复地址，空为不设置。
    pub reply_to: String,
    /// 连接、握手与每条命令的超时（1–60 秒）。
    pub timeout_secs: u64,
    /// 连接池上限（1–16）。
    pub pool_size: u32,
}

impl Default for Smtp {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 465,
            tls: SmtpTls::Implicit,
            username: String::new(),
            password: Secret::default(),
            from: String::new(),
            reply_to: String::new(),
            timeout_secs: 15,
            pool_size: 3,
        }
    }
}

impl Smtp {
    pub fn enabled(&self) -> bool {
        !self.host.is_empty()
    }
}

/// Brevo HTTP API 通道（国外邮箱的主通道，MAIL-007）。`api_key` 为空表示不启用。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Brevo {
    pub api_key: Secret,
    pub api_url: String,
    /// 发件人，`绳网 <noreply@example.com>` 或纯地址（须是 Brevo 里验证过的发件人）。
    pub from: String,
    /// 回复地址，空为不设置。
    pub reply_to: String,
    /// 请求超时（1–60 秒）。
    pub timeout_secs: u64,
}

impl Default for Brevo {
    fn default() -> Self {
        Self {
            api_key: Secret::default(),
            api_url: "https://api.brevo.com/v3".to_owned(),
            from: String::new(),
            reply_to: String::new(),
            timeout_secs: 10,
        }
    }
}

impl Brevo {
    pub fn enabled(&self) -> bool {
        !self.api_key.is_empty()
    }
}

impl Mail {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.provider != MailProvider::Dual {
            return Ok(());
        }
        let invalid = |msg: &str| Err(ConfigError::Invalid(format!("mail 配置不合法：{msg}")));
        if !self.smtp.enabled() && !self.brevo.enabled() {
            return invalid("dual 至少要配置 smtp.host 或 brevo.api_key 之一");
        }
        if self.smtp.enabled() {
            if self.smtp.from.is_empty() {
                return invalid("缺少 smtp.from");
            }
            if !(1..=60).contains(&self.smtp.timeout_secs)
                || !(1..=16).contains(&self.smtp.pool_size)
            {
                return invalid("smtp.timeout_secs 1–60，smtp.pool_size 1–16");
            }
        }
        if self.brevo.enabled() {
            if self.brevo.from.is_empty() {
                return invalid("缺少 brevo.from");
            }
            if !(1..=60).contains(&self.brevo.timeout_secs) {
                return invalid("brevo.timeout_secs 1–60");
            }
            if !(self.brevo.api_url.starts_with("https://")
                || self.brevo.api_url.starts_with("http://"))
            {
                return invalid("brevo.api_url 须为 http(s)://");
            }
        }
        if self
            .domestic_domains
            .iter()
            .any(|d| d.is_empty() || d.contains('@') || *d != d.trim().to_lowercase())
        {
            return invalid("domestic_domains 须为小写域名，例如 [\"corp.cn\"]");
        }
        Ok(())
    }
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
    /// 网页与接口不在同一个站点时打开（主站 interk.net 直接跨站调用 ik.tiwat.cn）：
    /// Cookie 改为 `SameSite=None; Partitioned`，只在主站页面里随请求发出（按顶层站点分区）。
    /// 跨站写请求仍由 CSRF 白名单（`http.allowed_origins`）把关。要求 `cookie_secure`。
    pub cookie_cross_site: bool,
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
            cookie_cross_site: false,
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
    /// 米游社扫码登录的确定性会话密钥（base64，解码后至少 32 字节，D-070 方案 A）。
    pub qr_session_key: Secret,
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

/// `IK_MAIL__SMTP__PASSWORD` → `mail.smtp.password`。
fn env_path(name: &str) -> String {
    name.to_lowercase().replace("__", ".")
}

/// 值是自由文本、不能按数字或布尔解析的配置项（密码、密钥、账号名、发件人）。
fn is_free_text(path: &str) -> bool {
    let last = path.rsplit('.').next().unwrap_or(path);
    matches!(last, "password" | "username" | "from" | "reply_to" | "secret" | "app_id")
        || last.ends_with("key")
}

impl Config {
    /// 从环境变量加载配置并校验。调用前应先加载 `.env`（由二进制负责）。
    pub fn load() -> Result<Self, ConfigError> {
        let mut figment = Figment::new()
            .merge(Serialized::defaults(Self::default()))
            .merge(Env::prefixed("IK_").split("__"));
        // figment 会猜环境变量的类型：纯数字的密码 `000123` 会变成整数 123（报错且丢了前导零）。
        // 凭据、发件人这类自由文本再按原样读一遍覆盖上去。
        for (name, value) in std::env::vars() {
            if let Some(path) = name.strip_prefix("IK_").map(env_path)
                && is_free_text(&path)
            {
                figment = figment.merge(Serialized::default(&path, value));
            }
        }
        Self::from_figment(figment)
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
        if self.bus.cluster && self.secrets.qr_session_key.is_empty() {
            return Err(ConfigError::Missing(
                "IK_SECRETS__QR_SESSION_KEY（bus.cluster 打开时必须配置，且各实例相同）",
            ));
        }
        let ai = &self.ai;
        if !(60..=7200).contains(&ai.hold_ttl_secs)
            || !(10..=1800).contains(&ai.reply_timeout_secs)
            || ai.first_delta_timeout_secs > ai.reply_timeout_secs
            || !(1..=1000).contains(&ai.stream_min_chars)
            || !(1..=200).contains(&ai.max_history_messages)
            || !(0..=100_000).contains(&ai.estimated_context_tokens)
            || !(1..=10).contains(&ai.max_attempts)
            || ai.providers.iter().any(|(name, p)| {
                name.is_empty()
                    || !(p.base_url.starts_with("https://") || p.base_url.starts_with("http://"))
            })
        {
            return Err(ConfigError::Invalid(
                "ai 配置不合法：hold_ttl_secs 60–7200，reply_timeout_secs 10–1800，first_delta_timeout_secs 不超过总预算，\
                 stream_min_chars 1–1000，max_history_messages 1–200，\
                 estimated_context_tokens 0–100000，max_attempts 1–10，providers 的 base_url 须为 http(s)://"
                    .to_owned(),
            ));
        }
        if !(1..=100).contains(&self.moderation.auto_hide_threshold) {
            return Err(ConfigError::Invalid(
                "moderation.auto_hide_threshold 必须在 1–100 之间".to_owned(),
            ));
        }
        let mihoyo = &self.mihoyo;
        let base_ok = |url: &str| url.starts_with("https://") || url.starts_with("http://");
        if !(1..=60).contains(&mihoyo.timeout_secs)
            || !base_ok(&mihoyo.passport_base_url)
            || !base_ok(&mihoyo.takumi_base_url)
        {
            return Err(ConfigError::Invalid(
                "mihoyo 配置不合法：timeout_secs 1–60，上游地址须为 http(s)://".to_owned(),
            ));
        }
        if !(1..=30).contains(&self.video.timeout_secs) || !base_ok(&self.video.bilibili_base_url) {
            return Err(ConfigError::Invalid(
                "video 配置不合法：timeout_secs 1–30，bilibili_base_url 须为 http(s)://".to_owned(),
            ));
        }
        self.mail.validate()?;
        self.qqbot.validate()?;
        if self.session.cookie_cross_site && !self.session.cookie_secure {
            return Err(ConfigError::Invalid(
                "session.cookie_cross_site 需要同时开启 session.cookie_secure（浏览器只接受 Secure 的跨站 Cookie）"
                    .to_owned(),
            ));
        }
        if self.env.is_prod() {
            if self.secrets.code_hmac_key.is_empty() {
                return Err(ConfigError::Missing("IK_SECRETS__CODE_HMAC_KEY"));
            }
            if self.secrets.qr_session_key.is_empty() {
                return Err(ConfigError::Missing("IK_SECRETS__QR_SESSION_KEY"));
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
    fn qqbot_config_requires_unique_apps_and_secrets() {
        let mut config = Qqbot::default();
        assert!(config.validate().is_ok());
        assert!(!config.allow_validation);
        assert_eq!(config.webhook_rate_per_second, 50);
        config.enabled = true;
        assert!(config.validate().is_err());
        let app = QqbotApp { app_id: "00123".to_owned(), secret: Secret::new("secret") };
        config.apps.insert("main".to_owned(), app.clone());
        assert!(config.validate().is_ok());
        assert!(!format!("{config:?}").contains("\"secret\""));
        config.apps.insert("duplicate".to_owned(), app);
        assert!(config.validate().is_err());
        config.apps.remove("duplicate");
        config.apps.get_mut("main").unwrap().secret = Secret::new(" ");
        assert!(config.validate().is_err());
        config.sandbox = true;
        assert_eq!(config.api_base(), "https://sandbox.api.sgroup.qq.com");
        assert!(is_free_text("qqbot.apps.main.app_id"));
        assert!(is_free_text("qqbot.apps.main.secret"));
    }

    #[test]
    fn dual_mail_needs_a_complete_channel() {
        let base = [("database.url", "postgres://x"), ("mail.provider", "dual")];
        assert!(Config::from_figment(with(&base)).is_err());
        let smtp_without_from =
            [base.as_slice(), &[("mail.smtp.host", "smtp.example.cn")]].concat();
        assert!(Config::from_figment(with(&smtp_without_from)).is_err());
        let ok = [
            base.as_slice(),
            &[
                ("mail.smtp.host", "smtp.example.cn"),
                ("mail.smtp.from", "a@example.cn"),
            ],
        ]
        .concat();
        let config = Config::from_figment(with(&ok)).unwrap();
        assert!(config.mail.smtp.enabled() && !config.mail.brevo.enabled());
        assert!(config.mail.fallback);
        let domains = |list: &[&str]| {
            Config::from_figment(
                with(&ok).merge(Serialized::default("mail.domestic_domains", list)),
            )
        };
        assert!(domains(&["corp.cn"]).is_ok());
        assert!(domains(&["Corp.CN"]).is_err());
    }

    #[test]
    #[allow(clippy::result_large_err)] // figment::Jail 的闭包签名是固定的
    fn mail_settings_from_env_keep_strings() {
        // 纯数字的密码、带空格和尖括号的发件人从环境变量读进来仍是原样的字符串。
        figment::Jail::expect_with(|jail| {
            jail.set_env("IK_DATABASE__URL", "postgres://x");
            jail.set_env("IK_MAIL__PROVIDER", "dual");
            jail.set_env("IK_MAIL__SMTP__HOST", "smtp.qiye.aliyun.com");
            jail.set_env("IK_MAIL__SMTP__TLS", "starttls");
            jail.set_env("IK_MAIL__SMTP__PORT", "587");
            jail.set_env("IK_MAIL__SMTP__USERNAME", "123456");
            jail.set_env("IK_MAIL__SMTP__PASSWORD", "000123");
            jail.set_env("IK_MAIL__SMTP__FROM", "绳网 <noreply@example.cn>");
            jail.set_env("IK_MAIL__DOMESTIC_DOMAINS", "[\"corp.cn\"]");
            let config = Config::load().unwrap();
            assert_eq!(config.mail.smtp.tls, SmtpTls::Starttls);
            assert_eq!(config.mail.smtp.port, 587);
            assert_eq!(config.mail.smtp.username, "123456");
            assert_eq!(config.mail.smtp.password.expose(), "000123");
            assert_eq!(config.mail.smtp.from, "绳网 <noreply@example.cn>");
            assert_eq!(config.mail.domestic_domains, ["corp.cn"]);
            Ok(())
        });
    }

    #[test]
    fn prod_rejects_dev_log_mailer() {
        let err = Config::from_figment(with(&[
            ("database.url", "postgres://x"),
            ("env", "prod"),
            ("secrets.code_hmac_key", "a2V5"),
            ("secrets.qr_session_key", "a2V5"),
        ]))
        .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn cluster_requires_shared_keys() {
        let cluster = Serialized::default("bus.cluster", true);
        let err =
            Config::from_figment(with(&[("database.url", "postgres://x")]).merge(cluster.clone()))
                .unwrap_err();
        assert!(matches!(err, ConfigError::Missing(_)));
        let err = Config::from_figment(
            with(&[
                ("database.url", "postgres://x"),
                ("secrets.code_hmac_key", "a2V5"),
            ])
            .merge(cluster.clone()),
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Missing(key) if key.contains("QR_SESSION_KEY")));
        let config = Config::from_figment(
            with(&[
                ("database.url", "postgres://x"),
                ("secrets.code_hmac_key", "a2V5"),
                ("secrets.qr_session_key", "a2V5"),
            ])
            .merge(cluster),
        )
        .unwrap();
        assert!(config.bus.cluster);
    }

    #[test]
    fn ai_providers_come_from_nested_keys() {
        let config = Config::from_figment(with(&[
            ("database.url", "postgres://x"),
            (
                "ai.providers.deepseek.base_url",
                "https://api.deepseek.com/v1",
            ),
            ("ai.providers.deepseek.api_key", "sk-test"),
        ]))
        .unwrap();
        let provider = &config.ai.providers["deepseek"];
        assert_eq!(provider.base_url, "https://api.deepseek.com/v1");
        assert_eq!(provider.api_key.expose(), "sk-test");
        let err = Config::from_figment(with(&[
            ("database.url", "postgres://x"),
            ("ai.providers.bad.base_url", "ftp://x"),
        ]))
        .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn cross_site_cookie_requires_secure() {
        let cross_site = |secure: bool| {
            with(&[("database.url", "postgres://x")])
                .merge(Serialized::default("session.cookie_secure", secure))
                .merge(Serialized::default("session.cookie_cross_site", true))
        };
        let err = Config::from_figment(cross_site(false)).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
        let config = Config::from_figment(cross_site(true)).unwrap();
        assert!(config.session.cookie_cross_site);
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
