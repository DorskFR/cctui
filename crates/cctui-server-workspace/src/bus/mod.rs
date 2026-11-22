//! Full-host addressing and reconcile adapters for the shared delivery bus.
pub mod peer;
use crate::state::AppState;
pub use cctui_server_core::bus::*;
use uuid::Uuid;

/// Rebuild and live-push a fresh [`cctui_proto::ws::DaemonFrameDown::Reconcile`] to `machine_id`'s
/// connected daemon. Used when a per-user setting the reconcile derives
/// from (e.g. `harnessMode`) changes, so a daemon picks up the new config without
/// waiting for a reconnect. Best-effort: same `NoDaemon`/`Closed` handling as
/// [`dispatch`] — a machine with no live WS is a no-op error the caller ignores.
pub async fn push_reconcile(state: &AppState, machine_id: Uuid) -> Result<(), BusError> {
    cctui_server_core::targets::daemon::reconcile::push_reconcile(state, machine_id).await
}
