use cctui_proto::adapter::{AdapterEvent, EndReason};
use serde_json::json;
use uuid::Uuid;

use super::bumps::Bumps;
use super::ingest::{Stored, insert_event, note_insert};
use super::registration::{publish_session_registered, upsert_session};
use crate::state::AppState;

mod prompts;
pub(super) mod session;
mod status;
mod stream;
mod todos;

use prompts::{is_auto_approve_excluded, should_auto_approve};
pub use session::truncate_end_detail;
use session::{
    mark_session_ended, persist_failed_spawn, publish_session_ended, update_transcript_mark,
};
use status::{StatusSignals, persist_pr_link_children, update_status_signals};
use stream::insert_token_usage;
use todos::{extract_todos, is_user_turn, record_todos};

#[allow(clippy::too_many_lines)]
#[allow(clippy::cognitive_complexity)]
pub(super) async fn handle_event(
    state: &AppState,
    bumps: &Bumps,
    machine_id: Uuid,
    user_id: Uuid,
    adapter_id: &str,
    event: AdapterEvent,
    stored: Stored,
) -> anyhow::Result<()> {
    let local_id_for_bump = match &event {
        AdapterEvent::Message { local_id, .. }
        | AdapterEvent::ToolUse { local_id, .. }
        | AdapterEvent::Status { local_id, .. } => Some(local_id.clone()),
        _ => None,
    };
    let event_turn_id = match &event {
        AdapterEvent::Message { turn_id, .. } => *turn_id,
        _ => None,
    };
    let broadcast_pair: Option<(String, cctui_proto::ws::AgentEvent)> = match &event {
        AdapterEvent::Message { local_id, payload, .. } => {
            crate::normalize::to_agent_event(adapter_id, "message", payload)
                .map(|ae| (local_id.clone(), ae))
        }
        AdapterEvent::ToolUse { local_id, payload } => {
            crate::normalize::to_agent_event(adapter_id, "tool_use", payload)
                .map(|ae| (local_id.clone(), ae))
        }
        AdapterEvent::SessionEnded { local_id, reason } => crate::normalize::to_agent_event(
            adapter_id,
            "session_ended",
            &json!({ "reason": reason }),
        )
        .map(|ae| (local_id.clone(), ae)),
        _ => None,
    };
    // Whether the broadcast below should actually fire. For Message/ToolUse we
    // only stream to the webui if the event was *newly* inserted — a daemon
    // that replays a session's full history on reconnect (e.g. after a
    // self-update) would otherwise re-stream every message, forcing clients to
    // replay the whole conversation with a long visible lag. The
    // `ON CONFLICT DO NOTHING` dedup already drops the duplicate rows; gating
    // the broadcast on a real insert extends that dedup to the live stream.
    let mut newly_inserted = true;
    // Causal ordering key: stamped onto the live broadcast so it
    // matches the reload path's `seq` (both are `stream_events.id`).
    let mut inserted_seq: Option<i64> = None;
    match event {
        AdapterEvent::SessionStarted { local_id, meta } => {
            let working_dir = meta.working_dir.clone();
            let observed_at = meta.extra.get("observed_at").and_then(serde_json::Value::as_i64);
            let extra = (!meta.extra.is_null()).then(|| meta.extra.clone());
            let spawn_key_hint =
                meta.extra.get("spawn_key").and_then(serde_json::Value::as_str).map(str::to_owned);
            if let Some(spawn_key) = meta.extra.get("spawn_key").and_then(serde_json::Value::as_str)
            {
                crate::routes::gateway::rebind_spawn_key(
                    state,
                    cctui_proto::ids::SpawnKey::from(spawn_key),
                    cctui_proto::ids::SessionId::from(local_id.as_str()),
                )
                .await;
            }
            let Some(first_registration) = upsert_session(
                &state.pool,
                machine_id,
                user_id,
                adapter_id,
                &local_id,
                working_dir,
                meta.parent_local_id.clone(),
                observed_at,
                extra,
            )
            .await?
            else {
                return Ok(());
            };
            if first_registration {
                publish_session_registered(state, &local_id).await;
            }
            crate::auto_archive::claim_intent(state, &local_id, spawn_key_hint.as_deref()).await;
            crate::spawn_labels::claim_intent(&state.pool, &local_id, spawn_key_hint.as_deref())
                .await;
            crate::followup::claim_intent(&state.pool, &local_id, spawn_key_hint.as_deref()).await;
        }
        AdapterEvent::Message { local_id, mut payload, turn_id } => {
            inserted_seq = if let Stored::Done(seq) = stored {
                seq
            } else {
                crate::keepalive::observe_message(state, &local_id, &mut payload).await;
                insert_event(
                    &state.pool,
                    machine_id,
                    user_id,
                    &local_id,
                    "message",
                    payload,
                    turn_id,
                )
                .await?
            };
            newly_inserted = inserted_seq.is_some();
            note_insert(state, machine_id, newly_inserted);
        }
        AdapterEvent::ToolUse { local_id, payload } => {
            inserted_seq = if let Stored::Done(seq) = stored {
                seq
            } else {
                insert_event(&state.pool, machine_id, user_id, &local_id, "tool_use", payload, None)
                    .await?
            };
            newly_inserted = inserted_seq.is_some();
            note_insert(state, machine_id, newly_inserted);
        }
        AdapterEvent::SessionEnded { local_id, reason } => {
            mark_session_ended(state, machine_id, user_id, &local_id, &reason).await?;
            publish_session_ended(state, &local_id, &reason);
        }
        AdapterEvent::TranscriptMark { local_id, offset } => {
            update_transcript_mark(state, &local_id, offset).await?;
        }
        AdapterEvent::TokenUsage {
            local_id,
            message_id,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
        } => {
            insert_token_usage(
                &state.pool,
                &local_id,
                &message_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
            )
            .await?;
        }
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
        AdapterEvent::PermissionRequest { local_id, request_id, tool, input } => {
            // `local_id` is the session id (claude session id / codex rollout
            // id, both used as the sessions PK). Park the request for TUI/web
            // and broadcast so inline prompts appear live.
            let input_preview = {
                let s = if input.is_null() { String::new() } else { input.to_string() };
                s.chars().take(500).collect::<String>()
            };
            // Auto-approve: if the session is in auto-approve mode,
            // answer `allow` immediately without prompting any client.
            let auto_approve_enabled =
                state.permission_store.read().await.is_auto_approve(&local_id);
            if auto_approve_enabled && is_auto_approve_excluded(&tool) {
                tracing::debug!(
                    session_id = %local_id,
                    request_id = %request_id,
                    tool = %tool,
                    "skipping auto-approve for plan/ask decision prompt"
                );
            }
            if should_auto_approve(&tool, auto_approve_enabled) {
                tracing::info!(
                    session_id = %local_id,
                    request_id = %request_id,
                    tool = %tool,
                    "auto-approving permission request"
                );
                let _ = crate::bus::dispatch(
                    state,
                    &local_id,
                    cctui_proto::adapter::AdapterCommand::PermissionResponse {
                        local_id: local_id.clone(),
                        request_id: request_id.clone(),
                        allow: true,
                    },
                )
                .await;
                bumps.heartbeat(&local_id);
                return Ok(());
            }
            state.permission_store.write().await.insert_request(
                crate::routes::permissions::PendingPermission {
                    session_id: local_id.clone(),
                    request_id: request_id.clone(),
                    tool_name: tool.clone(),
                    description: tool.clone(),
                    input_preview: input_preview.clone(),
                    received_at: chrono::Utc::now(),
                },
            );
            state.bus.publish_server(cctui_proto::ws::ServerEvent::PermissionRequest {
                session_id: local_id.clone(),
                request_id,
                tool_name: tool.clone(),
                description: tool,
                input_preview,
            });
            bumps.heartbeat(&local_id);
        }
        AdapterEvent::PermissionResolved { local_id, request_id } => {
            // The adapter observed the agent's permission prompt clear (answered
            // natively, dispatched by us, or timed out). Drop the parked request
            // and tell clients to dismiss the inline prompt. Idempotent: a
            // request answered via cctui already broadcast PermissionResolved on
            // the client path, so a second clear here is a harmless no-op.
            {
                let mut store = state.permission_store.write().await;
                let foreign = store
                    .list_pending()
                    .iter()
                    .any(|p| p.request_id == request_id && p.session_id != local_id);
                if foreign {
                    tracing::warn!(%local_id, %request_id, "PermissionResolved for another session's request");
                    return Ok(());
                }
                store.record_decision(&request_id, "resolved".into());
            }
            state.bus.publish_server(cctui_proto::ws::ServerEvent::PermissionResolved {
                session_id: local_id.clone(),
                request_id,
            });
            bumps.heartbeat(&local_id);
        }
        AdapterEvent::AskQuestion { local_id, question, questions, preamble } => {
            // Live AskUserQuestion: broadcast the pending question so
            // clients render an inline prompt immediately. Ephemeral — not
            // persisted as a stream_event; the full structured tool call still
            // lands in history via the transcript once the turn advances.
            // `questions` carries the structured options so the client renders
            // the interactive form live, not just the flattened text.
            // Park it authoritatively so a client that (re)subscribes after the
            // broadcast still learns the open prompt — the broadcast alone was
            // lost forever if nobody was listening at that instant.
            state.permission_store.write().await.insert_ask(
                crate::routes::permissions::PendingAsk {
                    session_id: local_id.clone(),
                    question: question.clone(),
                    questions: questions.clone(),
                    preamble: preamble.clone(),
                    received_at: chrono::Utc::now(),
                },
            );
            state.bus.publish_server(cctui_proto::ws::ServerEvent::AskQuestion {
                session_id: local_id.clone(),
                question,
                questions,
                preamble,
            });
            bumps.heartbeat(&local_id);
        }
        AdapterEvent::AskResolved { local_id } => {
            state.permission_store.write().await.remove_ask(&local_id);
            state.bus.publish_server(cctui_proto::ws::ServerEvent::AskResolved {
                session_id: local_id.clone(),
            });
            bumps.heartbeat(&local_id);
        }
        AdapterEvent::PlanRequest { local_id, plan, preamble } => {
            // Live ExitPlanMode plan-approval prompt: park it
            // authoritatively (so a (re)subscribing client still learns it) and
            // broadcast so clients render the live Plan card. Mirrors the
            // AskQuestion path; ephemeral, not persisted as a stream_event.
            state.permission_store.write().await.insert_plan(
                crate::routes::permissions::PendingPlan {
                    session_id: local_id.clone(),
                    plan: plan.clone(),
                    preamble: preamble.clone(),
                    received_at: chrono::Utc::now(),
                },
            );
            state.bus.publish_server(cctui_proto::ws::ServerEvent::PlanRequest {
                session_id: local_id.clone(),
                plan,
                preamble,
            });
            bumps.heartbeat(&local_id);
        }
        AdapterEvent::PlanResolved { local_id } => {
            state.permission_store.write().await.remove_plan(&local_id);
            state.bus.publish_server(cctui_proto::ws::ServerEvent::PlanResolved {
                session_id: local_id.clone(),
            });
            bumps.heartbeat(&local_id);
        }
        AdapterEvent::Status {
            local_id,
            tempo,
            state: agent_state,
            detail: _,
            activity,
            name,
            intent,
            model,
            effort,
            permission_mode,
            children,
        } => {
            // Persist the classifier signals + display metadata so
            // `list_sessions` can derive the "needs input" attention flag and
            // show name/model/effort. Status events are otherwise not stored
            // as stream_events (heartbeat bump below handles liveness).
            update_status_signals(
                state,
                &local_id,
                StatusSignals {
                    tempo: tempo.as_deref(),
                    agent_state: agent_state.as_deref(),
                    activity: activity.as_deref(),
                    name: name.as_deref(),
                    intent: intent.as_deref(),
                    model: model.as_deref(),
                    effort: effort.as_deref(),
                    permission_mode: permission_mode
                        .and_then(|m| serde_json::to_value(m).ok())
                        .and_then(|v| v.as_str().map(str::to_owned)),
                    children: &children,
                },
            )
            .await?;
        }
        AdapterEvent::PrLink { local_id, children } => {
            persist_pr_link_children(state, &local_id, &children).await?;
        }
        AdapterEvent::RateLimits { local_id, windows, observed_at } => {
            crate::usage_history::record_agent_limits(state, local_id, &windows, observed_at);
        }
        AdapterEvent::SessionModel { local_id, model } => {
            // Overwrite with the transcript/init-frame ground truth — the model
            // the session is ACTUALLY running. Previously this only
            // filled when unset, so the requested `--model` (delivered first via
            // a Status event) permanently masked a spare-claim/clamp drift. The
            // Status path now fills model only when NULL, so this ground-truth
            // write wins and sticks.
            sqlx::query("UPDATE sessions SET model = $2 WHERE id = $1")
                .bind(&local_id)
                .bind(&model)
                .execute(&state.pool)
                .await
                .map_err(|e| {
                    tracing::error!("db error (session model): {e}");
                    e
                })?;
        }
        _ => {}
    }
    // fold the tool-activity counters into the same heartbeat write for
    // a real `ToolCall` (a tool_result normalizes to `ToolResult` → plain bump).
    let tool_call = match &broadcast_pair {
        Some((_, cctui_proto::ws::AgentEvent::ToolCall { tool, input, .. })) => {
            Some((tool.as_str(), input))
        }
        _ => None,
    };
    let tool_name = tool_call.map(|(tool, _)| tool);
    if let Some(id) = local_id_for_bump {
        let user_turn = is_user_turn(broadcast_pair.as_ref().map(|(_, e)| e));
        bumps.note_activity(&id, newly_inserted, tool_name, user_turn);
        if newly_inserted
            && let Some((tool, input)) = tool_call
            && let Some(todos) = extract_todos(tool, input)
        {
            record_todos(&state.pool, &id, &todos).await;
        }
    }
    if newly_inserted && let Some((session_id, mut data)) = broadcast_pair {
        if let Some(seq) = inserted_seq {
            data.set_seq(seq);
        }
        if let Some(turn_id) = event_turn_id {
            data.set_turn_id(turn_id);
        }
        state.bus.publish_server(cctui_proto::ws::ServerEvent::Stream { session_id, data });
    }
    Ok(())
}
