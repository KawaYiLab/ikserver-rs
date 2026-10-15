use std::sync::Arc;

use ik_kernel::Snowflake;
use ik_kernel::bus::EventBus;
use ik_kernel::config::Ai;
use ik_media::MediaState;
use sqlx::PgPool;

use crate::catalog::CatalogCache;
use crate::llm::Providers;

#[derive(Clone)]
pub struct AiState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub bus: Arc<dyn EventBus>,
    pub media: MediaState,
    pub catalog: CatalogCache,
    pub providers: Providers,
    pub config: Arc<Ai>,
}

impl AiState {
    pub fn new(
        db: PgPool,
        ids: Arc<Snowflake>,
        bus: Arc<dyn EventBus>,
        media: MediaState,
        config: Ai,
        providers: Providers,
    ) -> Self {
        Self {
            catalog: CatalogCache::new(db.clone()),
            db,
            ids,
            bus,
            media,
            providers,
            config: Arc::new(config),
        }
    }

    /// 按配置里的提供方建上游客户端。
    pub fn from_config(
        db: PgPool,
        ids: Arc<Snowflake>,
        bus: Arc<dyn EventBus>,
        media: MediaState,
        config: &Ai,
    ) -> anyhow::Result<Self> {
        let providers = Providers::from_config(&config.providers)?;
        Ok(Self::new(db, ids, bus, media, config.clone(), providers))
    }
}

impl std::fmt::Debug for AiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiState").finish_non_exhaustive()
    }
}
