//! Server-side machine liveness.
//!
//! A machine's liveness tier is derived purely from the age of
//! `machines.last_seen_at`, which the daemon-WS handler advances on every
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

use crate::events::{self, Actor, Event, kind};
use crate::state::AppState;
use crate::store::sessions::SessionRowStatus;

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

/// Record `tier` for `machine_id`, broadcasting iff it changed.
///
/// The broadcast is a [`ServerEvent::MachineLiveness`]; returns whether the
/// tier changed from the last known one. The `online` / `stale` transitions
/// are logged here; `offline` is logged by [`sweep`] once it knows how many
/// sessions the transition ended, so one offline is one row.
pub fn record_and_broadcast(state: &AppState, machine_id: Uuid, tier: MachineLiveness) -> bool {
    let changed = record_tier(&state.machine_liveness, machine_id, tier);
    if changed {
        tracing::info!(%machine_id, ?tier, "machine liveness changed");
        state.bus.publish_server(cctui_proto::ws::ServerEvent::MachineLiveness {
            machine_id,
            liveness: tier,
        });
        match tier {
            MachineLiveness::Online => {
                events::record(
                    state,
                    Event::new(kind::MACHINE_ONLINE, Actor::System).machine(machine_id),
                );
            }
            MachineLiveness::Stale => events::record(
                state,
                Event::new(kind::MACHINE_STALE, Actor::System)
                    .severity(events::Severity::Warn)
                    .machine(machine_id),
            ),
            MachineLiveness::Offline => {}
        }
    }
    changed
}

fn record_tier(
    tiers: &dashmap::DashMap<Uuid, MachineLiveness>,
    machine_id: Uuid,
    tier: MachineLiveness,
) -> bool {
    tiers.insert(machine_id, tier).is_none_or(|prev| prev != tier)
}

/// Re-derive every non-deleted machine's tier from its persisted
/// `last_seen_at` and broadcast any transitions. Run periodically by the reaper
/// so a machine whose daemon died (no more heartbeats) ages from online → stale
/// → offline on its own, without needing any client traffic.
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
    let newly_offline = newly_offline(rows.into_iter().map(|(id, last_seen_at)| {
        let tier = derive(last_seen_at);
        (id, tier, record_and_broadcast(state, id, tier))
    }));
    if newly_offline.is_empty() {
        return;
    }
    let ended = mark_sessions_machine_offline(state, &newly_offline).await;
    for machine_id in newly_offline {
        let count = ended.iter().filter(|(_, m)| *m == machine_id).count();
        events::record(
            state,
            Event::new(kind::MACHINE_OFFLINE, Actor::System)
                .severity(events::Severity::Warn)
                .machine(machine_id)
                .detail(serde_json::json!({ "ended_sessions": count })),
        );
    }
    for (session_id, machine_id) in ended {
        events::record(
            state,
            Event::new(kind::SESSION_ENDED, Actor::System)
                .severity(events::Severity::Warn)
                .session(session_id)
                .machine(machine_id)
                .detail(serde_json::json!({
                    "end_reason": "machine_offline",
                    "end_detail": "machine offline: no daemon heartbeat",
                })),
        );
    }
}

fn newly_offline(transitions: impl Iterator<Item = (Uuid, MachineLiveness, bool)>) -> Vec<Uuid> {
    transitions
        .filter(|&(_, tier, changed)| changed && tier == MachineLiveness::Offline)
        .map(|(id, ..)| id)
        .collect()
}

/// End every still-live session of the given offline machines.
///
/// Marks them `machine_offline`, returning `(session, machine)` per ended row.
/// Soft: the daemon re-registering the session on reconnect reverts it.
async fn mark_sessions_machine_offline(
    state: &AppState,
    machine_ids: &[Uuid],
) -> Vec<(String, Uuid)> {
    match sqlx::query_as::<_, (String, Uuid)>(
        "UPDATE sessions SET status = 'ended', ended_at = now(), end_reason = 'machine_offline', \
             end_detail = 'machine offline: no daemon heartbeat' \
         WHERE machine_uuid = ANY($1) AND status = ANY($2) \
           AND end_reason IS NULL \
         RETURNING id, machine_uuid",
    )
    .bind(machine_ids)
    .bind(SessionRowStatus::names(SessionRowStatus::RUNNING))
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => {
            if !rows.is_empty() {
                tracing::info!(
                    machines = machine_ids.len(),
                    count = rows.len(),
                    "sessions marked machine_offline"
                );
            }
            rows
        }
        Err(err) => {
            tracing::warn!(%err, "machine_offline mark failed");
            Vec::new()
        }
    }
}

/// Record `tier` for an enrolled `dispatcher_id` and broadcast a
/// [`ServerEvent::DispatcherLiveness`] iff it changed. Peer of
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
        state.bus.publish_server(cctui_proto::ws::ServerEvent::DispatcherLiveness {
            dispatcher_id,
            liveness: tier,
        });
        let logged = match tier {
            MachineLiveness::Online => {
                Some((kind::SYSTEM_DISPATCHER_ONLINE, events::Severity::Info))
            }
            MachineLiveness::Offline => {
                Some((kind::SYSTEM_DISPATCHER_OFFLINE, events::Severity::Warn))
            }
            MachineLiveness::Stale => None,
        };
        if let Some((event_kind, severity)) = logged {
            events::record(
                state,
                Event::new(event_kind, Actor::System)
                    .severity(severity)
                    .detail(serde_json::json!({ "dispatcher_id": dispatcher_id })),
            );
        }
    }
}

/// Re-derive every live dispatcher's tier from its persisted `last_seen_at` and
/// broadcast any transitions. Peer of [`sweep`].
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
    fn only_the_transition_to_offline_marks_sessions() {
        let tiers = dashmap::DashMap::new();
        let id = Uuid::from_u128(7);
        let pass = |tier| newly_offline(std::iter::once((id, tier, record_tier(&tiers, id, tier))));
        assert!(pass(MachineLiveness::Online).is_empty());
        assert_eq!(pass(MachineLiveness::Offline), vec![id]);
        assert!(pass(MachineLiveness::Offline).is_empty());
        assert!(pass(MachineLiveness::Offline).is_empty());
        assert!(pass(MachineLiveness::Online).is_empty());
        assert_eq!(pass(MachineLiveness::Offline), vec![id]);
    }

    /// One `machine.offline` per dead-window crossing, none on later sweeps.
    #[tokio::test]
    async fn an_offline_machine_logs_one_offline_row_and_one_end_per_session() {
        let Some(url) = crate::routes::gateway::test_db_url("liveness_offline_events") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, machine) = crate::events::tests::seed_user_machine(&pool, "lv-off").await;
        let a = crate::events::tests::seed_session(&pool, uid, machine, Some("a")).await;
        let b = crate::events::tests::seed_session(&pool, uid, machine, Some("b")).await;
        sqlx::query("UPDATE machines SET last_seen_at = now() - interval '2 hours' WHERE id = $1")
            .bind(machine)
            .execute(&pool)
            .await
            .unwrap();
        state.machine_liveness.insert(machine, MachineLiveness::Online);
        for _ in 0..3 {
            sweep(&state).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let offline: Vec<(serde_json::Value,)> = sqlx::query_as(
            "SELECT detail FROM events WHERE machine_id = $1 AND kind = 'machine.offline'",
        )
        .bind(machine)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(offline.len(), 1, "one machine.offline, not one per sweep or per session");
        assert_eq!(offline[0].0["ended_sessions"], 2);
        for sid in [&a, &b] {
            let rows = crate::events::tests::rows_for_session(&pool, sid).await;
            let ended: Vec<_> = rows.iter().filter(|r| r.kind == "session.ended").collect();
            assert_eq!(ended.len(), 1);
            assert_eq!(ended[0].detail["end_reason"], "machine_offline");
        }
    }

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
