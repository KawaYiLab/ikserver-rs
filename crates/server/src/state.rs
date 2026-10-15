use std::sync::Arc;

use axum::extract::FromRef;
use ik_auth::{AuthSettings, AuthState};
use ik_comments::CommentsState;
use ik_content::ContentState;
use ik_economy::EconomyState;
use ik_exam::ExamState;
use ik_kernel::Snowflake;
use ik_kernel::bus::LocalBus;
use ik_kernel::config::{Config, MailProvider};
use ik_kernel::mail::{DevLogMailer, Mailer};
use ik_notify::NotifyState;
use ik_realtime::RealtimeState;
use ik_social::SocialState;
use sqlx::PgPool;

/// 所有 handler 共享的应用状态。克隆代价很低（内部都是 Arc / 连接池句柄）。
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub bus: Arc<LocalBus>,
    pub auth: AuthState,
    pub content: ContentState,
    pub comments: CommentsState,
    pub economy: EconomyState,
    pub exam: ExamState,
    pub social: SocialState,
    pub notify: NotifyState,
    pub realtime: RealtimeState,
}

impl AppState {
    pub fn new(config: Config, db: PgPool) -> anyhow::Result<Self> {
        let ids = Arc::new(Snowflake::new(config.node_id)?);
        let mailer: Arc<dyn Mailer> = match config.mail.provider {
            MailProvider::DevLog => Arc::new(DevLogMailer),
        };
        let bus = Arc::new(LocalBus::new(4096));
        let auth = AuthState::new(
            db.clone(),
            ids.clone(),
            mailer,
            AuthSettings::from_config(&config)?,
            bus.clone(),
        );
        let content = ContentState::new(db.clone(), ids.clone(), bus.clone());
        let comments = CommentsState::new(db.clone(), ids.clone(), bus.clone());
        let economy = EconomyState::new(db.clone(), ids.clone());
        let exam = ExamState::new(db.clone(), ids.clone(), config.exam.clone());
        let social = SocialState::new(db.clone());
        let notify = NotifyState::new(db.clone(), bus.clone());
        let realtime = RealtimeState::start(&bus, config.http.allowed_origins.clone())?;
        Ok(Self {
            config: Arc::new(config),
            db,
            ids,
            bus,
            auth,
            content,
            comments,
            economy,
            exam,
            social,
            notify,
            realtime,
        })
    }
}

impl FromRef<AppState> for AuthState {
    fn from_ref(state: &AppState) -> Self {
        state.auth.clone()
    }
}

impl FromRef<AppState> for ContentState {
    fn from_ref(state: &AppState) -> Self {
        state.content.clone()
    }
}

impl FromRef<AppState> for CommentsState {
    fn from_ref(state: &AppState) -> Self {
        state.comments.clone()
    }
}

impl FromRef<AppState> for EconomyState {
    fn from_ref(state: &AppState) -> Self {
        state.economy.clone()
    }
}

impl FromRef<AppState> for ExamState {
    fn from_ref(state: &AppState) -> Self {
        state.exam.clone()
    }
}

impl FromRef<AppState> for SocialState {
    fn from_ref(state: &AppState) -> Self {
        state.social.clone()
    }
}

impl FromRef<AppState> for NotifyState {
    fn from_ref(state: &AppState) -> Self {
        state.notify.clone()
    }
}

impl FromRef<AppState> for RealtimeState {
    fn from_ref(state: &AppState) -> Self {
        state.realtime.clone()
    }
}
