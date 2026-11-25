use std::sync::OnceLock;

use cctui_proto::ws::ServerEvent;
use sqlx::PgPool;
use uuid::Uuid;

use super::channel::{Capabilities, Channel, Endpoint, ProviderRef, Rendered, SendError};
use super::store::{NotificationRecord, unread_count};
use cctui_server_kernel::bus::Bus;

pub const ID: &str = "in_app";

static BUS: OnceLock<Bus> = OnceLock::new();

pub fn install(bus: &Bus) {
    let _ = BUS.set(bus.clone());
}

/// The store republishes the record on every change, so the adapter has nothing to send:
/// a routed `in_app` delivery only marks the notice delivered.
pub struct InApp;

#[async_trait::async_trait]
impl Channel for InApp {
    fn id(&self) -> &'static str {
        ID
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { update: true, retract: true, buttons: 3, needs_token: false }
    }

    async fn send(&self, _n: &Rendered, _to: &Endpoint) -> Result<ProviderRef, SendError> {
        Ok(ProviderRef(serde_json::json!({})))
    }

    async fn update(
        &self,
        _n: &Rendered,
        _to: &Endpoint,
        _r: &ProviderRef,
    ) -> Result<(), SendError> {
        Ok(())
    }

    async fn retract(&self, _to: &Endpoint, _r: &ProviderRef) -> Result<(), SendError> {
        Ok(())
    }
}

#[must_use]
pub fn record_event(record: &NotificationRecord) -> ServerEvent {
    ServerEvent::Notification { notification: Box::new(record.clone()) }
}

#[must_use]
pub const fn unread_event(user_id: Uuid, count: i64) -> ServerEvent {
    ServerEvent::NotificationsUnread { user_id, count }
}

pub async fn changed(pool: &PgPool, record: &NotificationRecord) {
    if let Some(bus) = BUS.get() {
        publish(bus, pool, std::slice::from_ref(record), record.user_id).await;
    }
}

pub async fn changed_many(pool: &PgPool, user_id: Uuid, records: &[NotificationRecord]) {
    if let Some(bus) = BUS.get() {
        publish(bus, pool, records, user_id).await;
    }
}

pub async fn publish(bus: &Bus, pool: &PgPool, records: &[NotificationRecord], user_id: Uuid) {
    for record in records {
        bus.publish_server(record_event(record));
    }
    match unread_count(pool, user_id).await {
        Ok(count) => bus.publish_server(unread_event(user_id, count)),
        Err(e) => tracing::warn!(%user_id, "notification unread count failed: {e}"),
    }
}

#[cfg(test)]
#[must_use]
pub fn test_bus() -> &'static Bus {
    BUS.get_or_init(|| Bus::new(Box::new(cctui_server_kernel::bus::NoopTransport)))
}

#[cfg(test)]
#[path = "in_app_tests.rs"]
mod tests;
