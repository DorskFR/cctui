use cctui_proto::adapter::AdapterEvent;

use crate::routes::daemon::bumps::Bumps;
use crate::state::AppState;

/// auto-approve is scoped to tool-use permissions. `ExitPlanMode` and
/// `AskUserQuestion` are user decision points that must always be answered by
/// the user, so they are excluded from the auto-approve short-circuit even when
/// the session flag is set.
fn is_auto_approve_excluded(tool: &str) -> bool {
    tool == "ExitPlanMode" || tool == "AskUserQuestion"
}

fn should_auto_approve(tool: &str, auto_approve_enabled: bool) -> bool {
    auto_approve_enabled && !is_auto_approve_excluded(tool)
}

/// Permission, question and plan prompts: parked for clients and broadcast live.
pub(super) async fn on_prompt_event(state: &AppState, bumps: &Bumps, event: AdapterEvent) {
    match event {
        e @ AdapterEvent::PermissionRequest { .. } => on_permission_request(state, bumps, e).await,
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
                    return;
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
        _ => {}
    }
}

async fn on_permission_request(state: &AppState, bumps: &Bumps, event: AdapterEvent) {
    let AdapterEvent::PermissionRequest { local_id, request_id, tool, input } = event else {
        return;
    };
    // `local_id` is the session id (claude session id / codex rollout
    // id, both used as the sessions PK). Park the request for TUI/web
    // and broadcast so inline prompts appear live.
    let input_preview = {
        let s = if input.is_null() { String::new() } else { input.to_string() };
        s.chars().take(500).collect::<String>()
    };
    // Auto-approve: if the session is in auto-approve mode,
    // answer `allow` immediately without prompting any client.
    let auto_approve_enabled = state.permission_store.read().await.is_auto_approve(&local_id);
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
        return;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_approve_excludes_plan_and_ask_but_allows_tools() {
        assert!(should_auto_approve("Bash", true));
        assert!(should_auto_approve("Edit", true));
        assert!(should_auto_approve("Write", true));
        assert!(!should_auto_approve("ExitPlanMode", true));
        assert!(!should_auto_approve("AskUserQuestion", true));
    }

    #[test]
    fn auto_approve_off_never_approves() {
        assert!(!should_auto_approve("Bash", false));
        assert!(!should_auto_approve("ExitPlanMode", false));
    }
}
