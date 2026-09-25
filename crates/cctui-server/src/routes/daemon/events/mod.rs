use cctui_proto::adapter::AdapterEvent;
use serde_json::json;
use uuid::Uuid;

use super::bumps::Bumps;
use super::ingest::Stored;
use crate::state::AppState;

mod prompts;
mod relay;
pub(super) mod session;
mod status;
mod stream;
mod todos;

use todos::{extract_todos, is_user_turn, record_todos};

pub(super) async fn handle_event(
    state: &AppState,
    bumps: &Bumps,
    machine_id: Uuid,
    user_id: Uuid,
    adapter_id: &str,
    event: AdapterEvent,
    stored: Stored,
) -> anyhow::Result<()> {
    let tail = Tail::of(adapter_id, &event);
    let inserted = match event {
        e @ (AdapterEvent::Message { .. } | AdapterEvent::ToolUse { .. }) => {
            stream::on_stream_event(state, machine_id, user_id, e, stored).await?
        }
        e @ AdapterEvent::TokenUsage { .. } => {
            stream::on_token_usage(&state.pool, e).await?;
            Inserted::UNSTORED
        }
        e @ (AdapterEvent::SessionStarted { .. }
        | AdapterEvent::SessionEnded { .. }
        | AdapterEvent::TranscriptMark { .. }
        | AdapterEvent::SessionModel { .. }) => {
            session::on_session_event(state, machine_id, user_id, adapter_id, e).await?;
            Inserted::UNSTORED
        }
        e @ (AdapterEvent::PermissionRequest { .. }
        | AdapterEvent::PermissionResolved { .. }
        | AdapterEvent::AskQuestion { .. }
        | AdapterEvent::AskResolved { .. }
        | AdapterEvent::PlanRequest { .. }
        | AdapterEvent::PlanResolved { .. }) => {
            prompts::on_prompt_event(state, bumps, e).await;
            Inserted::UNSTORED
        }
        e @ (AdapterEvent::Status { .. }
        | AdapterEvent::PrLink { .. }
        | AdapterEvent::RateLimits { .. }) => {
            status::on_status_event(state, e).await?;
            Inserted::UNSTORED
        }
        e @ (AdapterEvent::Diagnose { .. }
        | AdapterEvent::PtyChunk { .. }
        | AdapterEvent::CommandResult { .. }) => {
            relay::on_relay_event(state, e).await;
            Inserted::UNSTORED
        }
        _ => Inserted::UNSTORED,
    };
    tail.finish(state, bumps, inserted).await;
    Ok(())
}

/// Whether an event's `stream_events` row was newly written, and its id.
///
/// For Message/ToolUse we only stream to the webui if the event was *newly*
/// inserted — a daemon that replays a session's full history on reconnect
/// (e.g. after a self-update) would otherwise re-stream every message, forcing
/// clients to replay the whole conversation with a long visible lag. The
/// `ON CONFLICT DO NOTHING` dedup already drops the duplicate rows; gating the
/// broadcast on a real insert extends that dedup to the live stream.
#[derive(Clone, Copy)]
struct Inserted {
    newly: bool,
    /// Causal ordering key: stamped onto the live broadcast so it matches the
    /// reload path's `seq` (both are `stream_events.id`).
    seq: Option<i64>,
}

impl Inserted {
    /// An event with no `stream_events` row of its own.
    const UNSTORED: Self = Self { newly: true, seq: None };

    const fn from_seq(seq: Option<i64>) -> Self {
        Self { newly: seq.is_some(), seq }
    }
}

/// What an event leaves for after its handler: the activity bump and the live
/// stream broadcast.
struct Tail {
    bump: Option<String>,
    turn_id: Option<Uuid>,
    broadcast: Option<(String, cctui_proto::ws::AgentEvent)>,
}

impl Tail {
    fn of(adapter_id: &str, event: &AdapterEvent) -> Self {
        let bump = match event {
            AdapterEvent::Message { local_id, .. }
            | AdapterEvent::ToolUse { local_id, .. }
            | AdapterEvent::Status { local_id, .. } => Some(local_id.clone()),
            _ => None,
        };
        let turn_id = match event {
            AdapterEvent::Message { turn_id, .. } => *turn_id,
            _ => None,
        };
        let broadcast = match event {
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
        Self { bump, turn_id, broadcast }
    }

    async fn finish(self, state: &AppState, bumps: &Bumps, inserted: Inserted) {
        let Self { bump, turn_id, broadcast } = self;
        // fold the tool-activity counters into the same heartbeat write for
        // a real `ToolCall` (a tool_result normalizes to `ToolResult` → plain bump).
        let tool_call = match &broadcast {
            Some((_, cctui_proto::ws::AgentEvent::ToolCall { tool, input, .. })) => {
                Some((tool.as_str(), input))
            }
            _ => None,
        };
        let tool_name = tool_call.map(|(tool, _)| tool);
        if let Some(id) = bump {
            let user_turn = is_user_turn(broadcast.as_ref().map(|(_, e)| e));
            bumps.note_activity(&id, inserted.newly, tool_name, user_turn);
            if inserted.newly
                && let Some((tool, input)) = tool_call
                && let Some(todos) = extract_todos(tool, input)
            {
                record_todos(&state.pool, &id, &todos).await;
            }
        }
        if inserted.newly
            && let Some((session_id, mut data)) = broadcast
        {
            if let Some(seq) = inserted.seq {
                data.set_seq(seq);
            }
            if let Some(turn_id) = turn_id {
                data.set_turn_id(turn_id);
            }
            state.bus.publish_server(cctui_proto::ws::ServerEvent::Stream { session_id, data });
        }
    }
}
