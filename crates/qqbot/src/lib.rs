//! QQ official bot channel (ADR 0008). All platform IO runs outside transactions.
mod admin;
mod admin_ext;
pub mod client;
mod commands;
#[cfg(test)]
mod integration;
pub mod jobs;
mod me;
pub mod memory_jobs;
mod repo;
mod state;
pub mod text;
mod webhook;

pub use admin::{BotView, GroupView, Settings, UpdateBot, UpdateGroup};
pub use me::{BindCode, Binding, BindingList};
pub use state::QqState;

use axum::extract::FromRef;
use ik_auth::AuthState;
use utoipa_axum::router::OpenApiRouter;

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    QqState: FromRef<S>,
{
    admin::router()
        .merge(admin_ext::router())
        .merge(me::router())
        .merge(webhook::router())
}
