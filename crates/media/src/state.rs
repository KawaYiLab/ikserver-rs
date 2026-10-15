use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use ik_kernel::config::Storage as StorageConfig;
use ik_kernel::{Problem, Snowflake};
use sqlx::PgPool;

use crate::cdn::Urls;
use crate::dto::MediaView;
use crate::gate::Gate;
use crate::storage::Storage;
use crate::token::TokenKey;

/// 签名上传：每个用户每分钟 60 次（防止批量刷预签名地址）。
const SIGNS_PER_MINUTE: NonZeroU32 = match NonZeroU32::new(60) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};

#[derive(Clone)]
pub struct MediaState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub(crate) urls: Arc<Urls>,
    pub(crate) backend: Option<Arc<Backend>>,
}

/// 配置了对象存储时才有：上传相关的一切。
pub(crate) struct Backend {
    pub storage: Storage,
    pub gate: Gate,
    pub key: TokenKey,
    pub root_prefix: String,
    pub sign_expires: Duration,
    pub sign_limit: DefaultKeyedRateLimiter<i64>,
}

impl MediaState {
    /// `token_secret` 用来派生完成令牌的签名密钥（部署密钥，见 `IK_SECRETS__CODE_HMAC_KEY`）。
    pub fn new(
        db: PgPool,
        ids: Arc<Snowflake>,
        config: &StorageConfig,
        token_secret: &[u8],
    ) -> Self {
        let backend = (!config.bucket.is_empty()).then(|| {
            Arc::new(Backend {
                storage: Storage::new(config),
                gate: Gate::new(
                    config.verify_slots,
                    Duration::from_secs(config.queue_timeout_secs),
                    Duration::from_secs(config.task_timeout_secs),
                ),
                key: TokenKey::derive(token_secret),
                root_prefix: config.root_prefix.clone(),
                sign_expires: Duration::from_secs(config.sign_expires_secs),
                sign_limit: RateLimiter::keyed(Quota::per_minute(SIGNS_PER_MINUTE)),
            })
        });
        Self {
            db,
            ids,
            urls: Arc::new(Urls::new(
                &config.public_base_url,
                &config.endpoint,
                &config.bucket,
            )),
            backend,
        }
    }

    /// 批量取媒体视图（不存在的 id 不出现）。供引用媒体的业务域展示用。
    pub async fn views(&self, ids: &[i64]) -> Result<HashMap<i64, MediaView>, Problem> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut conn = self.db.acquire().await?;
        Ok(crate::repo::by_ids(&mut conn, ids)
            .await?
            .into_iter()
            .map(|row| (row.id, crate::service::view(&self.urls, row)))
            .collect())
    }

    /// 测试用：建桶。
    #[doc(hidden)]
    pub async fn ensure_bucket(&self) -> anyhow::Result<()> {
        if let Some(backend) = &self.backend {
            backend.storage.ensure_bucket().await?;
        }
        Ok(())
    }
}
