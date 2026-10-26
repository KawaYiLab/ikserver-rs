use std::num::NonZeroU32;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use ik_kernel::Snowflake;
use ik_kernel::bus::EventBus;
use ik_kernel::config::Config;
use ik_kernel::mail::Mailer;
use sqlx::PgPool;

/// 认证接口每 IP 每条路由每分钟 20 次（RL-001）。
const AUTH_REQUESTS_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(20) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};

#[derive(Clone)]
pub struct AuthState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub mailer: Arc<dyn Mailer>,
    pub settings: Arc<AuthSettings>,
    /// 会话被吊销时发布内部事件（实时网关据此断开连接）。
    pub bus: Arc<dyn EventBus>,
    pub lifecycle: crate::lifecycle::LifecycleHooks,
    pub(crate) limiter: Arc<DefaultKeyedRateLimiter<String>>,
}

impl AuthState {
    pub fn new(
        db: PgPool,
        ids: Arc<Snowflake>,
        mailer: Arc<dyn Mailer>,
        settings: AuthSettings,
        bus: Arc<dyn EventBus>,
    ) -> Self {
        Self {
            db,
            ids,
            mailer,
            settings: Arc::new(settings),
            bus,
            lifecycle: crate::lifecycle::LifecycleHooks::default(),
            limiter: Arc::new(RateLimiter::keyed(Quota::per_minute(
                AUTH_REQUESTS_PER_MINUTE,
            ))),
        }
    }
}

#[derive(Debug)]
pub struct AuthSettings {
    pub cookie_name: String,
    pub cookie_secure: bool,
    /// 跨站 Cookie（`SameSite=None; Partitioned`），见 `ik_kernel::config::Session::cookie_cross_site`。
    pub cookie_cross_site: bool,
    pub session_ttl: chrono::Duration,
    pub session_max_lifetime: chrono::Duration,
    pub registration_open: bool,
    pub trusted_proxy_hops: u32,
    /// 验证码 HMAC 密钥（原始字节）。
    pub code_key: Vec<u8>,
}

impl AuthSettings {
    pub fn from_config(config: &Config) -> anyhow::Result<Self> {
        let raw = config.secrets.code_hmac_key.expose();
        let code_key = if raw.is_empty() {
            // 只有开发环境能走到这里（生产环境配置校验要求必填）。进程重启后旧验证码全部失效。
            tracing::warn!("未配置 IK_SECRETS__CODE_HMAC_KEY，使用进程内随机密钥");
            let mut key = vec![0u8; 32];
            getrandom::fill(&mut key).map_err(|e| anyhow::anyhow!("生成随机密钥失败：{e}"))?;
            key
        } else {
            let key = STANDARD
                .decode(raw)
                .map_err(|e| anyhow::anyhow!("IK_SECRETS__CODE_HMAC_KEY 不是合法 base64：{e}"))?;
            anyhow::ensure!(
                key.len() >= 32,
                "IK_SECRETS__CODE_HMAC_KEY 解码后至少 32 字节"
            );
            key
        };
        Ok(Self {
            cookie_name: config.session.cookie_name.clone(),
            cookie_secure: config.session.cookie_secure,
            cookie_cross_site: config.session.cookie_cross_site,
            session_ttl: chrono::Duration::days(config.session.ttl_days),
            session_max_lifetime: chrono::Duration::days(config.session.max_lifetime_days),
            registration_open: config.auth.registration_open,
            trusted_proxy_hops: config.http.trusted_proxy_hops,
            code_key,
        })
    }
}
