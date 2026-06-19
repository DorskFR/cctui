//! Server-side machine liveness (CCT-255).
//!
//! A machine's liveness tier is derived purely from the age of
//! `machines.last_seen_at`, which the daemon-WS handler now advances on every
//! [`cctui_proto::ws::DaemonFrameUp::Heartbeat`] (the daemon emits one per ping
//! cadence). This mirrors the session-liveness thresholds in `routes::admin`
//! so machines and sessions read consistently.
//!
//! Transitions (e.g. `online` → `offline` when a daemon dies) are broadcast as
//! a [`cctui_proto::ws::ServerEvent::MachineLiveness`] to webui/TUI — the same
//! way session status changes are pushed — so a killed daemon flips its machine
//! to offline within one liveness window without waiting for a failed dispatch.

use cctui_proto::models::MachineLiveness;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::state::AppState;

/// `Online` (green) within the active window, `Stale` (orange) up to the dead
/// window, `Offline` beyond it. Mirrors `routes::sessions::LIVENESS_*` so machine
/// and session dots age out on the same schedule.
const ONLINE_SECS: i64 = 5 * 60;
const OFFLINE_SECS: i64 = 60 * 60;

/// Map `last_seen_at` age onto the three-tier machine liveness.
#[must_use]
pub fn derive(last_seen_at: DateTime<Utc>) -> MachineLiveness {
    let age = (Utc::now() - last_seen_at).num_seconds();
    if age < ONLINE_SECS {
        MachineLiveness::Online
    } else if age < OFFLINE_SECS {
        MachineLiveness::Stale
    } else {
        MachineLiveness::Offline
    }
}

/// Record `tier` for `machine_id` and broadcast a
/// [`ServerEvent::MachineLiveness`] iff it changed from the last known tier.
/// Idempotent within a tier — only an actual transition hits the wire.
pub fn record_and_broadcast(state: &AppState, machine_id: Uuid, tier: MachineLiveness) {
    let changed = state.machine_liveness.insert(machine_id, tier).is_none_or(|prev| prev != tier);
    if changed {
        tracing::info!(%machine_id, ?tier, "machine liveness changed");
        let _ = state
            .tui_tx
            .send(cctui_proto::ws::ServerEvent::MachineLiveness { machine_id, liveness: tier });
    }
}

/// Re-derive every non-deleted machine's tier from its persisted
/// `last_seen_at` and broadcast any transitions. Run periodically by the reaper
/// so a machine whose daemon died (no more heartbeats) ages from online → stale
/// → offline on its own, without needing any client traffic (CCT-255).
pub async fn sweep(state: &AppState) {
    let rows: Vec<(Uuid, DateTime<Utc>)> = match sqlx::query_as(
        "SELECT id, last_seen_at FROM machines WHERE deleted_at IS NULL AND revoked_at IS NULL",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(%err, "machine liveness sweep query failed");
            return;
        }
    };
    for (id, last_seen_at) in rows {
        record_and_broadcast(state, id, derive(last_seen_at));
    }
}

/// Record `tier` for an enrolled `dispatcher_id` and broadcast a
/// [`ServerEvent::DispatcherLiveness`] iff it changed (CCT-285). Peer of
/// [`record_and_broadcast`].
pub fn record_and_broadcast_dispatcher(
    state: &AppState,
    dispatcher_id: Uuid,
    tier: MachineLiveness,
) {
    let changed =
        state.dispatcher_liveness.insert(dispatcher_id, tier).is_none_or(|prev| prev != tier);
    if changed {
        tracing::info!(%dispatcher_id, ?tier, "dispatcher liveness changed");
        let _ = state.tui_tx.send(cctui_proto::ws::ServerEvent::DispatcherLiveness {
            dispatcher_id,
            liveness: tier,
        });
    }
}

/// Re-derive every live dispatcher's tier from its persisted `last_seen_at` and
/// broadcast any transitions (CCT-285). Peer of [`sweep`].
pub async fn sweep_dispatchers(state: &AppState) {
    let rows: Vec<(Uuid, DateTime<Utc>)> = match sqlx::query_as(
        "SELECT id, last_seen_at FROM dispatchers WHERE deleted_at IS NULL AND revoked_at IS NULL",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(%err, "dispatcher liveness sweep query failed");
            return;
        }
    };
    for (id, last_seen_at) in rows {
        record_and_broadcast_dispatcher(state, id, derive(last_seen_at));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_tiers_by_age() {
        let now = Utc::now();
        assert_eq!(derive(now), MachineLiveness::Online);
        assert_eq!(
            derive(now - chrono::Duration::seconds(ONLINE_SECS + 1)),
            MachineLiveness::Stale
        );
        assert_eq!(
            derive(now - chrono::Duration::seconds(OFFLINE_SECS + 1)),
            MachineLiveness::Offline
        );
    }
}
