use std::sync::Arc;

use axum::extract::FromRef;
use ik_ai::AiState;
use ik_auth::{AuthSettings, AuthState};
use ik_comments::CommentsState;
use ik_content::ContentState;
use ik_cosmetics::CosmeticsState;
use ik_economy::EconomyState;
use ik_exam::ExamState;
use ik_feed::FeedState;
use ik_kernel::Snowflake;
use ik_kernel::bus::{EventBus, LocalBus};
use ik_kernel::cluster::ClusterBus;
use ik_kernel::config::{Config, MailProvider};
use ik_kernel::mail::{DevLogMailer, Mailer};
use ik_media::MediaState;
use ik_messaging::MessagingState;
use ik_mihoyo::MihoyoState;
use ik_moderation::ModerationState;
use ik_notify::NotifyState;
use ik_profile::ProfileState;
use ik_realtime::{RealtimeState, TypingHandler};
use ik_search::SearchState;
use ik_social::SocialState;
use sqlx::PgPool;

/// 所有 handler 共享的应用状态。克隆代价很低（内部都是 Arc / 连接池句柄）。
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    /// 本实例的事件总线：订阅方（实时网关、搜索索引同步）订阅它。
    pub bus: Arc<LocalBus>,
    /// 业务域发布事件用：单实例就是 `bus`，多实例时还会转给其他实例。
    pub publisher: Arc<dyn EventBus>,
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
    pub search: SearchState,
    pub media: MediaState,
    pub cosmetics: CosmeticsState,
    pub mihoyo: MihoyoState,
    pub moderation: ModerationState,
    pub ai: AiState,
    pub rate_limit: Arc<crate::rate_limit::GlobalLimiter>,
}

impl AppState {
    pub fn new(config: Config, db: PgPool) -> anyhow::Result<Self> {
        let ids = Arc::new(Snowflake::new(config.node_id)?);
        let mailer: Arc<dyn Mailer> = match config.mail.provider {
            MailProvider::DevLog => Arc::new(DevLogMailer),
        };
        let bus = Arc::new(LocalBus::new(4096));
        let publisher: Arc<dyn EventBus> = if config.bus.cluster {
            ClusterBus::start(bus.clone(), db.clone(), config.node_id)?
        } else {
            bus.clone()
        };
        let auth = AuthState::new(
            db.clone(),
            ids.clone(),
            mailer,
            AuthSettings::from_config(&config)?,
            publisher.clone(),
        );
        let media = MediaState::new(
            db.clone(),
            ids.clone(),
            &config.storage,
            config.secrets.code_hmac_key.expose().as_bytes(),
        );
        let content = ContentState::new(db.clone(), ids.clone(), publisher.clone(), media.clone());
        let comments =
            CommentsState::new(db.clone(), ids.clone(), publisher.clone(), media.clone());
        let economy = EconomyState::new(db.clone(), ids.clone());
        let exam = ExamState::new(db.clone(), ids.clone(), config.exam.clone());
        let social = SocialState::new(db.clone());
        let notify = NotifyState::new(db.clone(), publisher.clone(), media.clone());
        let messaging =
            MessagingState::new(db.clone(), ids.clone(), publisher.clone(), media.clone());
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
        let profile = ProfileState::new(
            db.clone(),
            ids.clone(),
            content.clone(),
            comments.clone(),
            media.clone(),
        );
        let feed = FeedState::start(content.clone(), config.hot.clone())?;
        // 索引同步由 `serve` 显式启动（CLI 子命令不需要）。
        let search = SearchState::new(content.clone(), comments.clone(), &config.search)?;
        let rate_limit = crate::rate_limit::GlobalLimiter::new(&config.rate_limit);
        let cosmetics = CosmeticsState::new(db.clone(), ids.clone(), media.clone());
        let mihoyo = MihoyoState::from_config(auth.clone(), &config)?;
        let moderation = ModerationState::new(
            db.clone(),
            ids.clone(),
            publisher.clone(),
            media.clone(),
            config.moderation.auto_hide_threshold,
        );
        let ai = AiState::from_config(
            db.clone(),
            ids.clone(),
            publisher.clone(),
            media.clone(),
            &config.ai,
        )?;
        Ok(Self {
            ai,
            moderation,
            cosmetics,
            mihoyo,
            rate_limit,
            config: Arc::new(config),
            db,
            ids,
            bus,
            publisher,
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
            search,
            media,
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

impl FromRef<AppState> for SearchState {
    fn from_ref(state: &AppState) -> Self {
        state.search.clone()
    }
}

impl FromRef<AppState> for CosmeticsState {
    fn from_ref(state: &AppState) -> Self {
        state.cosmetics.clone()
    }
}

impl FromRef<AppState> for MihoyoState {
    fn from_ref(state: &AppState) -> Self {
        state.mihoyo.clone()
    }
}

impl FromRef<AppState> for ModerationState {
    fn from_ref(state: &AppState) -> Self {
        state.moderation.clone()
    }
}

impl FromRef<AppState> for AiState {
    fn from_ref(state: &AppState) -> Self {
        state.ai.clone()
    }
}

impl FromRef<AppState> for MediaState {
    fn from_ref(state: &AppState) -> Self {
        state.media.clone()
    }
}
