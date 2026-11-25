//! [`crate::events`] recorded through the host state.
pub use crate::events::{Actor, Event, EventRecord, Severity, is_system_kind, kind, prune};

use crate::context::KernelContext;

pub fn record<S: KernelContext>(state: &S, event: Event) {
    crate::events::record(state.pool(), state.bus(), event);
}

pub async fn record_now<S: KernelContext>(
    state: &S,
    event: Event,
) -> Result<EventRecord, sqlx::Error> {
    crate::events::record_now(state.pool(), state.bus(), event).await
}
