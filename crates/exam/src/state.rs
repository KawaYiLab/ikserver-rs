use std::sync::Arc;

use ik_kernel::Snowflake;
use ik_kernel::config::Exam;
use ik_kernel::site::Site;
use sqlx::PgPool;

#[derive(Clone)]
pub struct ExamState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    /// 部署配置里的考试参数；设置了 `site` 时以后台的为准。
    pub settings: Arc<Exam>,
    pub site: Option<Site>,
}

impl ExamState {
    pub fn new(db: PgPool, ids: Arc<Snowflake>, settings: Exam) -> Self {
        Self {
            db,
            ids,
            settings: Arc::new(settings),
            site: None,
        }
    }

    /// 改用后台可改的站点参数（`serve` 启动时调用）。
    #[must_use]
    pub fn with_site(mut self, site: Site) -> Self {
        self.site = Some(site);
        self
    }

    /// 当前生效的考试参数。
    pub async fn params(&self) -> Arc<Exam> {
        match &self.site {
            Some(site) => Arc::new(Exam::from(&site.get().await.exam)),
            None => self.settings.clone(),
        }
    }
}
