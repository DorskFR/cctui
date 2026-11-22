use cctui_proto::adapter::{AdapterEvent, EndReason};

use super::session::{persist_failed_spawn, publish_session_ended};
use crate::context::SessionsContext;

/// Replies relayed straight back to waiting callers: diagnose reports, live
/// terminal chunks, streamed text and command results. Nothing here is
/// persisted, except durable launch verdicts.
pub async fn on_relay_event<S: SessionsContext>(
    state: &S,
    machine_id: uuid::Uuid,
    user_id: uuid::Uuid,
    event: AdapterEvent,
) {
    match event {
        AdapterEvent::Diagnose { request_id, report, .. } => {
            // Session-diagnose reply: fire the oneshot the
            // `GET /sessions/{id}/diagnose` round-trip parked in the bus. A
            // late reply (route timed out, or a spooled event replayed after
            // reconnect) resolves nothing and is dropped.
            if !state.bus().resolve_diagnose(request_id, report) {
                tracing::debug!(%request_id, "Diagnose reply for unknown request (timed out?)");
            }
        }
        AdapterEvent::PtyChunk { local_id, data } => {
            // Live terminal relay: never persisted — fan the base64
            // chunk straight out to the browsers watching this session.
            state.bus().publish_server(cctui_proto::ws::ServerEvent::PtyChunk {
                session_id: local_id,
                data,
            });
        }
        AdapterEvent::TextDelta { local_id, item_id, delta } => {
            state.bus().publish_server(cctui_proto::ws::ServerEvent::TextDelta {
                session_id: local_id,
                item_id,
                delta,
            });
        }
        AdapterEvent::GuardStepState { local_id, state: step } => {
            crate::guard_step::record_and_broadcast(state.bus(), local_id, step);
        }
        AdapterEvent::CommandResult { command_id, ok, error } => {
            on_command_result(state, machine_id, user_id, command_id, ok, error).await;
        }
        _ => {}
    }
}

async fn on_command_result<S: SessionsContext>(
    state: &S,
    machine_id: uuid::Uuid,
    user_id: uuid::Uuid,
    command_id: uuid::Uuid,
    ok: bool,
    error: Option<String>,
) {
    if !ok {
        tracing::warn!(%command_id, ?error, "command failed on daemon");
    }
    let known = match state.machine_launches_find(&command_id.to_string(), None).await {
        Ok(known) => known,
        Err(error) => {
            tracing::error!(%command_id, %error, "cannot read durable launch verdict");
            return;
        }
    };
    if known
        .as_ref()
        .is_some_and(|launch| launch.machine_id != machine_id || launch.user_id != user_id)
    {
        tracing::warn!(%command_id, "refusing foreign machine launch verdict");
        return;
    }
    let durable = match state
        .machine_launches_verdict(command_id, machine_id, user_id, ok, error.as_deref())
        .await
    {
        Ok(launch) => launch,
        Err(error) => {
            tracing::error!(%command_id, %error, "cannot persist launch verdict");
            return;
        }
    };
    let pending = state.pending_commands().remove(&command_id).map(|(_, c)| c);
    let mut session_id = pending.as_ref().and_then(|c| {
        c.session_id.clone().or_else(|| c.spawn.as_ref().map(|row| row.session_id.clone()))
    });
    // A spawn that never started has no row of its own: write one so
    // the failure shows up on the list with its detail.
    let failed = durable
        .map(|launch| launch.failed_row())
        .or_else(|| if known.is_none() { pending.and_then(|c| c.spawn) } else { None });
    if !ok && let Some(row) = failed {
        let detail = error.clone().unwrap_or_else(|| "spawn failed".to_owned());
        let reason = EndReason::SpawnFailed { detail };
        match persist_failed_spawn(state.pool(), &row, &reason).await {
            Ok(true) => {
                session_id = Some(row.session_id.clone());
                publish_session_ended(state, &row.session_id, &reason);
                super::session::record_session_ended(
                    state,
                    &row.session_id,
                    row.machine_id,
                    row.user_id,
                    &reason,
                );
            }
            Ok(false) => {}
            Err(e) => tracing::error!(%command_id, "db error (failed spawn): {e}"),
        }
    }
    // After the failed row is written, so a request answering with the
    // refusal never races the session list.
    crate::commands::resolve_verdict(
        state.command_waiters(),
        command_id,
        crate::commands::CommandVerdict { ok, error: error.clone() },
    );
    state.bus().publish_server(cctui_proto::ws::ServerEvent::CommandResult {
        command_id: command_id.to_string(),
        ok,
        error,
        session_id,
    });
}
