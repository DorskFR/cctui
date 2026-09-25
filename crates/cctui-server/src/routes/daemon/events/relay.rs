use cctui_proto::adapter::{AdapterEvent, EndReason};

use super::session::{persist_failed_spawn, publish_session_ended};
use crate::state::AppState;

/// Replies relayed straight back to waiting callers: diagnose reports, live
/// terminal chunks and command results.
pub(super) async fn on_relay_event(state: &AppState, event: AdapterEvent) {
    match event {
        AdapterEvent::Diagnose { request_id, report, .. } => {
            // Session-diagnose reply: fire the oneshot the
            // `GET /sessions/{id}/diagnose` round-trip parked in the bus. A
            // late reply (route timed out, or a spooled event replayed after
            // reconnect) resolves nothing and is dropped.
            if !state.bus.resolve_diagnose(request_id, report) {
                tracing::debug!(%request_id, "Diagnose reply for unknown request (timed out?)");
            }
        }
        AdapterEvent::PtyChunk { local_id, data } => {
            // Live terminal relay: never persisted — fan the base64
            // chunk straight out to the browsers watching this session.
            state.bus.publish_server(cctui_proto::ws::ServerEvent::PtyChunk {
                session_id: local_id,
                data,
            });
        }
        AdapterEvent::CommandResult { command_id, ok, error } => {
            if !ok {
                tracing::warn!(%command_id, ?error, "command failed on daemon");
            }
            let pending = state.pending_commands.remove(&command_id).map(|(_, c)| c);
            let mut session_id = pending.as_ref().and_then(|c| {
                c.session_id.clone().or_else(|| c.spawn.as_ref().map(|row| row.session_id.clone()))
            });
            // A spawn that never started has no row of its own: write one so
            // the failure shows up on the list with its detail.
            if !ok && let Some(row) = pending.and_then(|c| c.spawn) {
                let detail = error.clone().unwrap_or_else(|| "spawn failed".to_owned());
                let reason = EndReason::SpawnFailed { detail };
                match persist_failed_spawn(&state.pool, &row, &reason).await {
                    Ok(true) => {
                        session_id = Some(row.session_id.clone());
                        publish_session_ended(state, &row.session_id, &reason);
                    }
                    Ok(false) => {}
                    Err(e) => tracing::error!(%command_id, "db error (failed spawn): {e}"),
                }
            }
            state.bus.publish_server(cctui_proto::ws::ServerEvent::CommandResult {
                command_id: command_id.to_string(),
                ok,
                error,
                session_id,
            });
        }
        _ => {}
    }
}
