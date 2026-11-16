use std::sync::OnceLock;

use sqlx::PgPool;
use uuid::Uuid;

pub mod channel;
pub mod dispatcher;
pub mod expiry;
pub mod ntfy_channel;
pub mod retention;
pub mod store;

pub use channel::Channels;
pub use store::{NewNotification, Routing, emit};

pub mod kind {
    pub const LEGACY: &str = "legacy";
}

static POOL: OnceLock<PgPool> = OnceLock::new();

pub fn install(pool: &PgPool) {
    let _ = POOL.set(pool.clone());
}

#[must_use]
pub fn installed_pool() -> Option<PgPool> {
    POOL.get().cloned()
}

pub async fn instance_owner(pool: &PgPool) -> Option<Uuid> {
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM users WHERE revoked_at IS NULL AND disabled_at IS NULL \
         ORDER BY created_at, id LIMIT 1",
    )
    .fetch_optional(pool)
    .await;
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

#[cfg(test)]
mod tests;
