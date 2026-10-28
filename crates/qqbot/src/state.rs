use crate::{admin::Settings, client::ApiClient, repo};
use ik_ai::AiState;
use ik_kernel::{Problem, Snowflake, config::Qqbot, jobs::Jobs};
use moka::future::Cache;
use parking_lot::Mutex;
use sqlx::PgPool;
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct QqState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub jobs: Jobs,
    pub ai: AiState,
    pub config: Arc<Qqbot>,
    pub(crate) client: Arc<ApiClient>,
    /// 绑定码的 HMAC 密钥（与验证码同一把，`IK_SECRETS__CODE_HMAC_KEY`）。
    pub(crate) code_key: Arc<Vec<u8>>,
    /// 主站地址（`IK_SITE_URL`），回复里的站内帖子链接按它补全。
    pub(crate) site_url: Arc<str>,
    settings: Cache<(), Arc<Settings>>,
    rates: Arc<Mutex<BTreeMap<String, (Instant, u32)>>>,
}

impl std::fmt::Debug for QqState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QqState").finish_non_exhaustive()
    }
}

impl QqState {
    pub fn new(
        db: PgPool,
        ids: Arc<Snowflake>,
        jobs: Jobs,
        ai: AiState,
        config: Qqbot,
        code_key: Vec<u8>,
    ) -> Result<Self, Problem> {
        let client = Arc::new(ApiClient::new(&config)?);
        Ok(Self {
            db,
            ids,
            jobs,
            ai,
            config: Arc::new(config),
            client,
            code_key: Arc::new(code_key),
            site_url: Arc::from(""),
            settings: Cache::builder()
                .max_capacity(1)
                .time_to_live(Duration::from_secs(60))
                .build(),
            rates: Arc::default(),
        })
    }

    #[must_use]
    pub fn with_site_url(mut self, site_url: &str) -> Self {
        self.site_url = Arc::from(site_url);
        self
    }

    /// Register configured application ids without overwriting administrator choices.
    pub async fn initialize(&self) -> Result<(), Problem> {
        let mut tx = self.db.begin().await?;
        for app in self.config.apps.values() {
            sqlx::query!(
                "INSERT INTO qq_bots(app_id) VALUES ($1) ON CONFLICT DO NOTHING",
                app.app_id
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn settings(&self) -> Result<Arc<Settings>, Problem> {
        self.settings
            .try_get_with((), async {
                let mut conn = self.db.acquire().await?;
                repo::settings(&mut conn).await.map(Arc::new)
            })
            .await
            .map_err(|p: Arc<Problem>| (*p).clone())
    }
    pub(crate) async fn invalidate(&self) {
        self.settings.invalidate(&()).await;
    }

    /// Bounded by configured applications, never by untrusted incoming app ids.
    pub(crate) fn allow_webhook(&self, app_id: &str) -> bool {
        let mut rates = self.rates.lock();
        let entry = rates
            .entry(app_id.to_owned())
            .or_insert((Instant::now(), 0));
        if entry.0.elapsed() >= Duration::from_secs(1) {
            *entry = (Instant::now(), 0);
        }
        if entry.1 >= self.config.webhook_rate_per_second {
            return false;
        }
        entry.1 += 1;
        true
    }
}
