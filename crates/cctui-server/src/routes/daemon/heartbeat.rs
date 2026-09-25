use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp};
use uuid::Uuid;

use super::reconcile::archived_jobs;
use crate::state::AppState;

pub(super) fn on_heartbeat(state: &AppState, machine_id: Uuid, frame: DaemonFrameUp) {
    let DaemonFrameUp::Heartbeat {
        bandwidth, update_hook, resources, claude_jobs, harness, ..
    } = frame
    else {
        return;
    };
    crate::machine_liveness::record_and_broadcast(
        state,
        machine_id,
        cctui_proto::models::MachineLiveness::Online,
    );
    if let Some(bandwidth) = &bandwidth {
        detect_divergence(state, machine_id, bandwidth.event_bytes());
    }
    let state = state.clone();
    tokio::spawn(async move {
        // A daemon too old to advertise omits the field; leave the stored
        // flag alone rather than reading silence as "no hook".
        if let Some(has_hook) = update_hook {
            crate::routes::update_hook::record_hook_flag(&state.pool, machine_id, has_hook).await;
        }
        if let Err(err) = sqlx::query("UPDATE machines SET last_seen_at = now() WHERE id = $1")
            .bind(machine_id)
            .execute(&state.pool)
            .await
        {
            tracing::warn!(%err, %machine_id, "heartbeat last_seen_at bump failed");
        }
        if let Some(bandwidth) = bandwidth {
            persist_bandwidth(&state, machine_id, &bandwidth).await;
        }
        if let Some(resources) = resources {
            crate::machine_resources::record_and_broadcast(&state, machine_id, resources).await;
        }
        // Only a daemon that reports its jobs can parse the reply.
        if let Some(shorts) = claude_jobs {
            reconcile_claude_jobs(&state, machine_id, &shorts).await;
        }
        if let Some(report) = harness {
            crate::routes::harness_update::on_heartbeat(&state, machine_id, &report).await;
        }
    });
}

/// Upsert the daemon's last-known per-subsystem byte counters. Fire-
/// and-forget: a failed write only loses one heartbeat's snapshot.
async fn persist_bandwidth(
    state: &AppState,
    machine_id: Uuid,
    bw: &cctui_proto::bandwidth::BandwidthSummary,
) {
    let res = sqlx::query(
        "INSERT INTO machine_bandwidth \
           (machine_id, forward, retransmit, backfill, self_update, blob_put, heartbeat, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, now()) \
         ON CONFLICT (machine_id) DO UPDATE SET \
           forward = EXCLUDED.forward, retransmit = EXCLUDED.retransmit, \
           backfill = EXCLUDED.backfill, self_update = EXCLUDED.self_update, \
           blob_put = EXCLUDED.blob_put, heartbeat = EXCLUDED.heartbeat, updated_at = now()",
    )
    .bind(machine_id)
    .bind(i64::try_from(bw.forward).unwrap_or(i64::MAX))
    .bind(i64::try_from(bw.retransmit).unwrap_or(i64::MAX))
    .bind(i64::try_from(bw.backfill).unwrap_or(i64::MAX))
    .bind(i64::try_from(bw.self_update).unwrap_or(i64::MAX))
    .bind(i64::try_from(bw.blob_put).unwrap_or(i64::MAX))
    .bind(i64::try_from(bw.heartbeat).unwrap_or(i64::MAX))
    .execute(&state.pool)
    .await;
    if let Err(err) = res {
        tracing::warn!(%err, %machine_id, "machine_bandwidth upsert failed");
    }
}

/// The 2026-07-21 failure signature: reported upload bytes climb while
/// persisted `stream_events` inserts don't. In-memory, cheap, piggybacked on the
/// heartbeat; an ERROR is the alert glitchtip forwards.
fn detect_divergence(state: &AppState, machine_id: Uuid, upload_bytes: u64) {
    let inserts = state.machine_event_inserts.get(&machine_id).map_or(0, |v| *v);
    if let Some(d) = state.divergence_tracker.observe(machine_id, upload_bytes, inserts) {
        tracing::error!(
            %machine_id,
            upload_bytes = d.upload_bytes,
            prev_upload_bytes = d.prev_upload_bytes,
            insert_count = d.insert_count,
            "upload/insert divergence — daemon uploading bytes with no new persisted \
             stream_events",
        );
    }
}

/// Answer a heartbeat's `claude_jobs` with the archived subset, if any.
async fn reconcile_claude_jobs(state: &AppState, machine_id: Uuid, shorts: &[String]) {
    if shorts.is_empty() {
        return;
    }
    let session_ids = match archived_jobs(&state.pool, machine_id, shorts).await {
        Ok(ids) => ids,
        Err(err) => {
            tracing::warn!(%err, %machine_id, "archived job lookup failed");
            return;
        }
    };
    if session_ids.is_empty() {
        return;
    }
    tracing::info!(%machine_id, count = session_ids.len(), "archived sessions still have claude jobs");
    if let Err(err) =
        state.bus.command_daemon(machine_id, DaemonFrameDown::ArchivedJobs { session_ids }).await
    {
        tracing::warn!(%err, %machine_id, "could not send ArchivedJobs");
    }
}
