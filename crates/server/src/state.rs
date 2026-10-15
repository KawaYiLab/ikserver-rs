use std::sync::Arc;

use axum::extract::FromRef;
use ik_auth::{AuthSettings, AuthState};
use ik_comments::CommentsState;
use ik_content::ContentState;
use ik_economy::EconomyState;
use ik_exam::ExamState;
use ik_feed::FeedState;
use ik_kernel::Snowflake;
use ik_kernel::bus::LocalBus;
use ik_kernel::config::{Config, MailProvider};
use ik_kernel::mail::{DevLogMailer, Mailer};
use ik_messaging::MessagingState;
use ik_notify::NotifyState;
use ik_profile::ProfileState;
use ik_realtime::{RealtimeState, TypingHandler};
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
    pub messaging: MessagingState,
    pub realtime: RealtimeState,
    pub profile: ProfileState,
    pub feed: FeedState,
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
        let messaging = MessagingState::new(db.clone(), ids.clone(), bus.clone());
        let typing: TypingHandler = {
            let messaging = messaging.clone();
            Arc::new(move |user_id, conversation_id| {
                let messaging = messaging.clone();
                Box::pin(async move {
                    ik_messaging::service::typing(&messaging, user_id, conversation_id).await
                })
            })
        };
        let realtime =
            RealtimeState::start(&bus, config.http.allowed_origins.clone())?.with_typing(typing);
        let profile = ProfileState::new(db.clone(), ids.clone(), content.clone(), comments.clone());
        let feed = FeedState::start(content.clone(), config.hot.clone())?;
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
            messaging,
            realtime,
            profile,
            feed,
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

impl FromRef<AppState> for MessagingState {
    fn from_ref(state: &AppState) -> Self {
        state.messaging.clone()
    }
}

impl FromRef<AppState> for RealtimeState {
    fn from_ref(state: &AppState) -> Self {
        state.realtime.clone()
    }
}

impl FromRef<AppState> for ProfileState {
    fn from_ref(state: &AppState) -> Self {
        state.profile.clone()
    }
}

impl FromRef<AppState> for FeedState {
    fn from_ref(state: &AppState) -> Self {
        state.feed.clone()
    }
}
