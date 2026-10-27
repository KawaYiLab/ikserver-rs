use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use ik_auth::AuthState;
use ik_kernel::config::Config;

use crate::upstream::{HttpApi, MihoyoApi};

/// 每 IP 每分钟最多生成 30 个二维码、轮询 600 次（MHY-019）。
const CREATES_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(30) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};
const POLLS_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(600) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};

#[derive(Clone)]
pub struct MihoyoState {
    pub auth: AuthState,
    pub api: Arc<dyn MihoyoApi>,
    pub enabled: bool,
    /// 确定性会话的密钥（D-070 方案 A）。
    pub(crate) session_key: Arc<[u8]>,
    pub(crate) limits: Arc<Limits>,
}

pub(crate) struct Limits {
    pub create: DefaultKeyedRateLimiter<IpAddr>,
    pub poll: DefaultKeyedRateLimiter<IpAddr>,
}

impl MihoyoState {
    pub fn new(
        auth: AuthState,
        api: Arc<dyn MihoyoApi>,
        enabled: bool,
        session_key: Vec<u8>,
    ) -> Self {
        Self {
            auth,
            api,
            enabled,
            session_key: session_key.into(),
            limits: Arc::new(Limits {
                create: RateLimiter::keyed(Quota::per_minute(CREATES_PER_MINUTE)),
                poll: RateLimiter::keyed(Quota::per_minute(POLLS_PER_MINUTE)),
            }),
        }
    }

    /// 扫码登录是否开放：站点参数可在后台修改（`auth.site`），没有时用部署配置。
    pub async fn login_enabled(&self) -> bool {
        match &self.auth.site {
            Some(site) => site.get().await.registration.mihoyo_login,
            None => self.enabled,
        }
    }

    /// 按配置连接真实的米游社上游。
    pub fn from_config(auth: AuthState, config: &Config) -> anyhow::Result<Self> {
        let raw = config.secrets.qr_session_key.expose();
        let key = if raw.is_empty() {
            // 只有单实例的开发环境能走到这里（生产与多实例的配置校验要求必填）。
            // 进程重启后，已经扫码登录过的二维码不能再兑换会话（本来也只有 5 分钟）。
            tracing::warn!("未配置 IK_SECRETS__QR_SESSION_KEY，使用进程内随机密钥");
            let mut key = vec![0u8; 32];
            getrandom::fill(&mut key).map_err(|e| anyhow::anyhow!("生成随机密钥失败：{e}"))?;
            key
        } else {
            let key = STANDARD
                .decode(raw)
                .map_err(|e| anyhow::anyhow!("IK_SECRETS__QR_SESSION_KEY 不是合法 base64：{e}"))?;
            anyhow::ensure!(
                key.len() >= 32,
                "IK_SECRETS__QR_SESSION_KEY 解码后至少 32 字节"
            );
            key
        };
        let mihoyo = &config.mihoyo;
        let api = HttpApi::new(
            &mihoyo.passport_base_url,
            &mihoyo.takumi_base_url,
            std::time::Duration::from_secs(mihoyo.timeout_secs),
        )?;
        Ok(Self::new(auth, Arc::new(api), mihoyo.enabled, key))
    }
}

// 执行器的扩展要求 Debug；内部有密钥，不打印字段。
impl std::fmt::Debug for MihoyoState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MihoyoState").finish_non_exhaustive()
    }
}
