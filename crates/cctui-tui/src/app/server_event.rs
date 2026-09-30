use cctui_proto::ws::{AgentEvent, ServerEvent};

use super::action::{Action, HeartbeatUsage};
use super::line::agent_event_to_line;
use super::state::PendingPermission;
use super::toast::Level;

/// The websocket's only entry point into the store. Exhaustive on purpose: a
/// new [`ServerEvent`] variant must not compile until it is handled or waived
/// here, and every waiver carries its reason on the arm.
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
        ServerEvent::SessionEnded { session_id, reason, detail } => {
            let detail = detail.map_or_else(String::new, |d| format!(" — {d}"));
            vec![Action::Toast(
                Level::Info,
                format!("{} ended: {reason:?}{detail}", short_id(&session_id)),
            )]
        }
        ServerEvent::SoftLimitReached { session_id, account_name, retry_after_secs, .. } => {
            vec![Action::Toast(
                Level::Error,
                format!(
                    "{} hit the {account_name} soft limit; retry in {retry_after_secs}s",
                    short_id(&session_id)
                ),
            )]
        }
        ServerEvent::SoftLimitCleared { session_id } => {
            vec![Action::Toast(Level::Info, format!("{} soft limit cleared", short_id(&session_id)))]
        }
        ServerEvent::ToolCallBlocked { session_id, tool_name, rule } => {
            vec![Action::Toast(
                Level::Warn,
                format!("{} blocked {tool_name} ({rule})", short_id(&session_id)),
            )]
        }
        // The socket lagged: the 5s REST refresh is the TUI's whole-state
        // resync, so ask for one now rather than waiting for the tick.
        ServerEvent::Resync { .. } => vec![Action::RefreshSessions],

        ServerEvent::ArchiveManifest { .. } => waived("the TUI has no archive browser"),
        ServerEvent::ArchiveUploaded { .. } => waived("the TUI has no archive browser"),
        ServerEvent::CommandResult { .. } => waived("no delivery-state UI in the TUI yet"),
        ServerEvent::MessageAck { .. } => waived("no delivery-state UI in the TUI yet"),
        ServerEvent::AskQuestion { .. } => waived("the TUI cannot answer asks yet"),
        ServerEvent::AskResolved { .. } => waived("the TUI cannot answer asks yet"),
        ServerEvent::PlanRequest { .. } => waived("the TUI has no plan-approval dialog yet"),
        ServerEvent::PlanResolved { .. } => waived("the TUI has no plan-approval dialog yet"),
        ServerEvent::MachineLiveness { .. } => waived("the TUI shows no machine list"),
        ServerEvent::MachineResources { .. } => waived("the TUI shows no machine list"),
        ServerEvent::DispatcherLiveness { .. } => waived("the TUI shows no dispatcher list"),
        ServerEvent::AccountUsage { .. } => waived("the TUI shows no account panel"),
        ServerEvent::GithubEvent { .. } => {
            waived("PR links come from the session rows the REST refresh returns")
        }
        ServerEvent::PtyChunk { .. } => waived("the TUI has no terminal pane"),
        ServerEvent::ScheduledLaunch { .. } => waived("the TUI has no drafts view"),
        ServerEvent::RoomMembers { .. } => waived("the TUI list does not group by room"),
        ServerEvent::UserActions { .. } => waived("the TUI has no needs-you list yet"),
        ServerEvent::Heartbeat { .. } => waived("liveness tick with nothing to render"),
    }
}

fn waived(reason: &'static str) -> Vec<Action> {
    tracing::trace!(reason, "server event not handled by the TUI");
    Vec::new()
}

/// Toasts are one status-line wide; a full session id would fill it.
fn short_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
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
