use std::sync::OnceLock;

use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;

pub mod channel;
pub mod dispatcher;
pub mod email;
pub mod expiry;
pub mod in_app;
pub mod ntfy_channel;
pub mod prefs;
pub mod producers;
pub mod quiet;
pub mod retention;
pub mod store;
pub mod tokens;
pub mod web_push;

pub use channel::Channels;
pub use store::{NewNotification, Routing, emit};

pub mod kind {
    pub const APPROVAL: &str = "approval";
    pub const USER_ACTION: &str = "user_action";
    pub const NEEDS_INPUT: &str = "needs_input";
    pub const SESSION_STUCK: &str = "session_stuck";
    pub const SYSTEM_ALERT: &str = "system_alert";
    pub const INBOX_STUCK: &str = "inbox_stuck";
    pub const DISPATCH_INFO: &str = "dispatch_info";
    pub const DISPATCH_FAILED: &str = "dispatch_failed";
    pub const TEST: &str = "test";
    pub const THROTTLED: &str = "notify_throttled";
}

/// When a kind reaches a push channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Push {
    Off,
    Now,
    AfterMinutes(i32),
}

impl Push {
    /// The default routing table; unknown kinds stay in-app only.
    #[must_use]
    pub fn for_kind(kind: &str, blocking: bool) -> Self {
        match kind {
            kind::SESSION_STUCK
            | kind::SYSTEM_ALERT
            | kind::INBOX_STUCK
            | kind::DISPATCH_FAILED => Self::Now,
            kind::APPROVAL => Self::AfterMinutes(5),
            kind::NEEDS_INPUT => Self::AfterMinutes(10),
            kind::USER_ACTION if blocking => Self::AfterMinutes(10),
            _ => Self::Off,
        }
    }
}

/// Kinds that are off everywhere by default leave no row at all.
#[must_use]
pub fn is_recorded(kind: &str) -> bool {
    kind != kind::DISPATCH_INFO
}

static POOL: OnceLock<PgPool> = OnceLock::new();

pub fn install(pool: &PgPool) {
    let _ = POOL.set(pool.clone());
}

#[must_use]
pub fn installed_pool() -> Option<PgPool> {
    POOL.get().cloned()
}

pub const OWNER_SQL: &str = "\
    SELECT id FROM users WHERE revoked_at IS NULL AND disabled_at IS NULL \
    ORDER BY created_at, id LIMIT 1";

pub async fn instance_owner(pool: &PgPool) -> Option<Uuid> {
    let owner = sqlx::query_scalar::<_, Uuid>(OWNER_SQL).fetch_optional(pool).await;
    let reason = match owner {
        Ok(Some(id)) => return Some(id),
        Ok(None) => "no active user to own it".to_owned(),
        Err(e) => format!("owner lookup failed: {e}"),
    };
    tracing::warn!("notification dropped: {reason}");
    None
}

pub async fn emit_now(pool: &PgPool, routing: Routing, channels: &Channels, n: NewNotification) {
    match emit(pool, routing, n).await {
        Ok(record) => dispatcher::run(pool, channels, Some(record.id)).await,
        Err(e) => tracing::warn!("notification emit failed: {e}"),
    }
}

/// Never blocks the caller. Without `owner` the notice goes to the instance owner.
pub fn emit_detached<F>(config: &Config, owner: Option<Uuid>, build: F)
where
    F: FnOnce(Uuid) -> NewNotification + Send + 'static,
{
    let Some(pool) = installed_pool() else {
        tracing::warn!("notification dropped: notification outbox not started");
        return;
    };
    emit_detached_on(pool, config, owner, build);
}

/// [`emit_detached`] on `pool`.
pub fn emit_detached_on<F>(pool: PgPool, config: &Config, owner: Option<Uuid>, build: F)
where
    F: FnOnce(Uuid) -> NewNotification + Send + 'static,
{
    let routing = Routing::from_config(config);
    let channels = Channels::from_config(config);
    tokio::spawn(async move {
        let fallback = if owner.is_none() { instance_owner(&pool).await } else { None };
        let Some(owner) = owner.or(fallback) else { return };
        emit_now(&pool, routing, &channels, build(owner)).await;
    });
}

#[cfg(test)]
pub mod env_owner_tests;
#[cfg(test)]
mod tests;
