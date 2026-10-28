use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use ik_kernel::config::Storage as StorageConfig;
use ik_kernel::{Problem, Snowflake};
use sqlx::{PgConnection, PgPool};

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
    pub staging_ttl: Duration,
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
                staging_ttl: Duration::from_secs(config.staging_ttl_hours * 3600),
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

    /// 测试用：没有对象存储（上传不可用），地址前缀为 `https://cdn.test`。
    #[doc(hidden)]
    pub fn for_tests(db: PgPool, ids: Arc<Snowflake>) -> Self {
        let config = StorageConfig {
            public_base_url: "https://cdn.test".to_owned(),
            ..StorageConfig::default()
        };
        Self::new(db, ids, &config, b"test")
    }

    /// 批量取媒体视图（不存在的 id 不出现）。供引用媒体的业务域展示用。
    ///
    /// 这一组查询都用调用方手里的连接：拿着一个连接再去池里要第二个，并发一高所有请求会互相等到超时。
    pub async fn views(
        &self,
        conn: &mut PgConnection,
        ids: &[i64],
    ) -> Result<HashMap<i64, MediaView>, Problem> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        Ok(crate::repo::by_ids(conn, ids)
            .await?
            .into_iter()
            .map(|row| (row.id, crate::service::view(&self.urls, row)))
            .collect())
    }

    /// 原尺寸 WebP 地址（按媒体 id；不存在的 id 不出现）。
    pub async fn webp_urls(
        &self,
        conn: &mut PgConnection,
        ids: &[i64],
    ) -> Result<HashMap<i64, String>, Problem> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        Ok(crate::repo::by_ids(conn, ids)
            .await?
            .into_iter()
            .map(|row| {
                let size = u64::try_from(row.size_bytes).unwrap_or(0);
                (row.id, self.urls.webp_url(&row.object_key, size))
            })
            .collect())
    }

    /// 发给识图大模型的图片地址（按媒体 id；不存在的 id 不出现）。上游自己去拉：JPEG、PNG、WebP 且不大于
    /// 8 MB 的用原图，其余（GIF、AVIF、过大的图）用 CDN 转出的 WebP，各家上游都认、体积也小。
    pub async fn vision_urls(
        &self,
        conn: &mut PgConnection,
        ids: &[i64],
    ) -> Result<HashMap<i64, String>, Problem> {
        const ORIGINAL_MAX_BYTES: i64 = 8 * 1024 * 1024;
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        Ok(crate::repo::by_ids(conn, ids)
            .await?
            .into_iter()
            .map(|row| {
                let plain = matches!(row.mime.as_str(), "image/jpeg" | "image/png" | "image/webp");
                let url = if plain && row.size_bytes <= ORIGINAL_MAX_BYTES {
                    self.urls.public_url(&row.object_key)
                } else {
                    self.urls
                        .webp_url(&row.object_key, u64::try_from(row.size_bytes).unwrap_or(0))
                };
                (row.id, url)
            })
            .collect())
    }

    /// 暂存前缀（`[root/]staging/`）。
    pub(crate) fn staging_prefix(backend: &Backend) -> String {
        if backend.root_prefix.is_empty() {
            "staging/".to_owned()
        } else {
            format!("{}/staging/", backend.root_prefix)
        }
    }

    /// 清理一次过期的暂存对象（没有完成的上传，UPL-060）。返回删除数。
    pub async fn sweep_staging(&self) -> anyhow::Result<usize> {
        let Some(backend) = &self.backend else {
            return Ok(0);
        };
        let ttl = chrono::Duration::from_std(backend.staging_ttl)?;
        self.delete_staging_before(chrono::Utc::now() - ttl).await
    }

    /// 删除最后修改时间早于 `before` 的暂存对象。返回删除数。
    pub async fn delete_staging_before(
        &self,
        before: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<usize> {
        let Some(backend) = &self.backend else {
            return Ok(0);
        };
        Ok(backend
            .storage
            .delete_older_than(&Self::staging_prefix(backend), before)
            .await?)
    }

    /// 这些作者的头像缩略图地址（按用户 id）。没有头像、已注销的不出现。
    pub async fn avatar_urls<'a>(
        &self,
        conn: &mut PgConnection,
        authors: impl IntoIterator<Item = &'a ik_auth::service::Author>,
    ) -> Result<HashMap<i64, String>, Problem> {
        let pairs: Vec<(i64, i64)> = authors
            .into_iter()
            .filter(|a| !a.gone)
            .filter_map(|a| a.avatar_media_id.map(|m| (a.id.get(), m)))
            .collect();
        let mut media_ids: Vec<i64> = pairs.iter().map(|(_, m)| *m).collect();
        media_ids.sort_unstable();
        media_ids.dedup();
        let views = self.views(conn, &media_ids).await?;
        Ok(pairs
            .into_iter()
            .filter_map(|(user, media)| views.get(&media).map(|v| (user, v.thumb_url.clone())))
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

// 执行器的扩展要求 Debug；内部有存储凭据，不打印字段。
impl std::fmt::Debug for MediaState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaState").finish_non_exhaustive()
    }
}
