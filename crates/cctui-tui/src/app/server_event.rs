use cctui_proto::ws::{AgentEvent, ServerEvent};

use super::action::{Action, HeartbeatUsage};
use super::line::agent_event_to_line;
use super::state::PendingPermission;

/// The websocket's only entry point into the store. Exhaustive on purpose: a
/// new [`ServerEvent`] variant must not compile until it is handled here.
pub(crate) fn to_actions(event: ServerEvent) -> Vec<Action> {
    match event {
        ServerEvent::PermissionRequest {
            session_id,
            request_id,
            tool_name,
            description,
            input_preview,
        } => vec![Action::PermissionRequested(PendingPermission {
            session_id,
            request_id,
            tool_name,
            description,
            input_preview,
        })],
        ServerEvent::Stream { session_id, data } => vec![stream_action(session_id, &data)],
        ServerEvent::Status { session_id, status } => {
            vec![Action::SessionStatusChanged { session_id, status }]
        }
        ServerEvent::SessionRegistered { session } => {
            vec![Action::SessionRegistered(Box::new(session))]
        }
        ServerEvent::SessionDeregistered { session_id } => {
            vec![Action::SessionDeregistered(session_id)]
        }
        ServerEvent::PermissionResolved { session_id, request_id } => {
            vec![Action::PermissionResolved { session_id, request_id }]
        }
        ServerEvent::ArchiveManifest { .. }
        | ServerEvent::ArchiveUploaded { .. }
        | ServerEvent::CommandResult { .. }
        | ServerEvent::SessionEnded { .. }
        | ServerEvent::AskQuestion { .. }
        | ServerEvent::MessageAck { .. }
        | ServerEvent::MachineLiveness { .. }
        | ServerEvent::MachineResources { .. }
        | ServerEvent::AccountUsage { .. }
        | ServerEvent::DispatcherLiveness { .. }
        | ServerEvent::PlanRequest { .. }
        | ServerEvent::PlanResolved { .. }
        | ServerEvent::GithubEvent { .. }
        | ServerEvent::AskResolved { .. }
        | ServerEvent::SoftLimitReached { .. }
        | ServerEvent::PtyChunk { .. }
        | ServerEvent::ScheduledLaunch { .. }
        | ServerEvent::RoomMembers { .. }
        | ServerEvent::UserActions { .. }
        | ServerEvent::Heartbeat { .. }
        | ServerEvent::Resync { .. }
        | ServerEvent::ToolCallBlocked { .. }
        | ServerEvent::SoftLimitCleared { .. } => Vec::new(),
    }
}

fn stream_action(session_id: String, data: &AgentEvent) -> Action {
    let usage = match data {
        AgentEvent::Heartbeat { tokens_in, tokens_out, cost_usd, .. } => Some(HeartbeatUsage {
            tokens_in: *tokens_in,
            tokens_out: *tokens_out,
            cost_usd: *cost_usd,
        }),
        _ => None,
    };
    Action::StreamLine { session_id, line: agent_event_to_line(data), usage }
}
