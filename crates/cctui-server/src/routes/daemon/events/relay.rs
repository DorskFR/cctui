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
            crate::state::resolve_verdict(
                &state.command_waiters,
                command_id,
                crate::state::CommandVerdict { ok, error: error.clone() },
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A refused spawn's waiting request hears of it only once the failed row
    /// is on the list, so the caller and the session list agree.
    #[tokio::test]
    async fn a_refused_spawn_is_written_before_the_waiting_request_hears_of_it() {
        let Some(url) = crate::routes::gateway::test_db_url("relay_refused_spawn") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("relay-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        let mid = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(mid)
            .bind(uid)
            .bind(format!("m-{mid}"))
            .bind(format!("mk-{mid}"))
            .execute(&pool)
            .await
            .expect("seed machine");
        let state = AppState::for_test(pool.clone());
        let command_id = uuid::Uuid::new_v4();
        let session_id = uuid::Uuid::new_v4().to_string();
        let rx = crate::state::wait_for_verdict(&state.command_waiters, command_id);
        crate::state::track_command(
            &state.pending_commands,
            command_id,
            None,
            Some(crate::state::FailedSpawnRow {
                session_id: session_id.clone(),
                machine_id: mid,
                user_id: uid,
                adapter_id: "claude-code".into(),
                working_dir: "/w".into(),
                name: None,
                model: None,
                effort: None,
            }),
        );

        on_relay_event(
            &state,
            AdapterEvent::CommandResult {
                command_id,
                ok: false,
                error: Some("adapter claude-code command queue is full".into()),
            },
        )
        .await;

        let verdict = rx.await.expect("the waiting request is told");
        assert!(!verdict.ok);
        assert_eq!(verdict.error.as_deref(), Some("adapter claude-code command queue is full"));
        let (status, end_reason): (String, Option<String>) =
            sqlx::query_as("SELECT status, end_reason FROM sessions WHERE id = $1")
                .bind(&session_id)
                .fetch_one(&pool)
                .await
                .expect("the failed row exists");
        assert_eq!(status, "ended");
        assert_eq!(end_reason.as_deref(), Some("spawn_failed"));
        assert!(state.command_waiters.is_empty());
    }
}
