use cctui_proto::adapter::{AdapterEvent, EndReason};

use super::session::{persist_failed_spawn, publish_session_ended};
use crate::state::AppState;

/// Replies relayed straight back to waiting callers: diagnose reports, live
/// terminal chunks, streamed text and command results. Nothing here is
/// persisted, except durable launch verdicts.
pub(super) async fn on_relay_event(
    state: &AppState,
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
        AdapterEvent::TextDelta { local_id, item_id, delta } => {
            state.bus.publish_server(cctui_proto::ws::ServerEvent::TextDelta {
                session_id: local_id,
                item_id,
                delta,
            });
        }
        AdapterEvent::GuardStepState { local_id, state: step } => {
            crate::guard_step::record_and_broadcast(&state.bus, local_id, step);
        }
        AdapterEvent::CommandResult { command_id, ok, error } => {
            on_command_result(state, machine_id, user_id, command_id, ok, error).await;
        }
        _ => {}
    }
}

async fn on_command_result(
    state: &AppState,
    machine_id: uuid::Uuid,
    user_id: uuid::Uuid,
    command_id: uuid::Uuid,
    ok: bool,
    error: Option<String>,
) {
    if !ok {
        tracing::warn!(%command_id, ?error, "command failed on daemon");
    }
    let known =
        match cctui_server_core::machine_launches::find(&state.pool, &command_id.to_string(), None)
            .await
        {
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
    let durable = match cctui_server_core::machine_launches::verdict(
        &state.pool,
        command_id,
        machine_id,
        user_id,
        ok,
        error.as_deref(),
    )
    .await
    {
        Ok(launch) => launch,
        Err(error) => {
            tracing::error!(%command_id, %error, "cannot persist launch verdict");
            return;
        }
    };
    let pending = state.pending_commands.remove(&command_id).map(|(_, c)| c);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A streamed delta fans out to the bus as it is and leaves no row behind.
    #[tokio::test]
    async fn a_text_delta_is_relayed_and_never_persisted() {
        let Some(url) = crate::routes::gateway::test_db_url("relay_text_delta") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let mut frames = state.bus.subscribe_server();
        let session_id = uuid::Uuid::new_v4().to_string();

        on_relay_event(
            &state,
            uuid::Uuid::nil(),
            uuid::Uuid::nil(),
            AdapterEvent::TextDelta {
                local_id: session_id.clone(),
                item_id: "msg_01:0".into(),
                delta: "Hel".into(),
            },
        )
        .await;

        let frame = frames.try_recv().expect("the delta is on the bus");
        assert!(frame.json.contains(r#""type":"text_delta""#), "{}", frame.json);
        assert!(frame.json.contains(r#""delta":"Hel""#), "{}", frame.json);
        let (rows,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM stream_events WHERE session_id = $1")
                .bind(&session_id)
                .fetch_one(&pool)
                .await
                .expect("count rows");
        assert_eq!(rows, 0, "a delta must never reach stream_events");
    }

    /// A refused spawn's waiting request hears of it only once the failed row
    /// is on the list, so the caller and the session list agree.
    #[tokio::test]
    async fn a_refused_spawn_is_written_before_the_waiting_request_hears_of_it() {
        refused_spawn(false).await;
    }

    #[tokio::test]
    async fn a_durable_refusal_recovers_metadata_and_rejects_a_foreign_machine() {
        refused_spawn(true).await;
    }

    async fn refused_spawn(durable: bool) {
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

        if durable {
            let row = state.pending_commands.get(&command_id).unwrap().spawn.clone().unwrap();
            cctui_server_core::machine_launches::prepare(&pool, command_id, uid, &row)
                .await
                .unwrap();
            on_relay_event(
                &state,
                uuid::Uuid::new_v4(),
                uid,
                AdapterEvent::CommandResult {
                    command_id,
                    ok: false,
                    error: Some("foreign refusal".into()),
                },
            )
            .await;
            assert!(state.pending_commands.contains_key(&command_id));
            assert!(state.command_waiters.contains_key(&command_id));
            assert_eq!(
                cctui_server_core::machine_launches::find(&pool, &command_id.to_string(), None)
                    .await
                    .unwrap()
                    .unwrap()
                    .state,
                "prepared"
            );
            state.pending_commands.clear();
        }
        on_relay_event(
            &state,
            mid,
            uid,
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
