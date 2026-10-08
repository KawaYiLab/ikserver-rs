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
    fn prod_requires_hmac_key() {
        let err = Config::from_figment(with(&[("database.url", "postgres://x"), ("env", "prod")]))
            .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Missing("IK_SECRETS__CODE_HMAC_KEY")
        ));
    }

    #[test]
    fn secret_is_redacted_in_debug() {
        let secret = Secret::new("hunter2");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
    }
}
