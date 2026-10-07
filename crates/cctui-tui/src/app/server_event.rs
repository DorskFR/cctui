use cctui_proto::ws::{AgentEvent, ServerEvent};

use super::action::{Action, HeartbeatUsage};
use super::attention::AttentionAction;
use super::diagnose::DiagnoseAction;
use super::line::agent_event_to_line;
use super::prompt::PromptAction;
use super::send::SendAction;
use super::session_live::SessionLiveAction;
use super::spawn::SpawnAction;
use super::state::PendingPermission;
use super::terminal::TerminalAction;
use super::toast::Level;

/// The websocket's only entry point into the store. Exhaustive on purpose: a
/// new [`ServerEvent`] variant must not compile until it is handled or waived
/// here, and every waiver carries its reason on the arm.
/// A delivery ack that the spawn dialog also waits on.
fn command_result(
    command_id: &str,
    ok: bool,
    error: Option<String>,
    session_id: Option<String>,
) -> Vec<Action> {
    uuid::Uuid::parse_str(command_id).ok().map_or_else(Vec::new, |command_id| {
        vec![
            Action::Send(SendAction::DeliveryResult { command_id, ok, error: error.clone() }),
            Action::Spawn(SpawnAction::Launched { command_id, ok, error, session_id }),
        ]
    })
}

pub fn to_actions(event: ServerEvent) -> Vec<Action> {
    match event {
        ServerEvent::PermissionRequest {
            session_id,
            request_id,
            tool_name,
            description,
            input_preview,
        } => vec![Action::Attention(AttentionAction::PermissionRequested(PendingPermission {
            session_id,
            request_id,
            tool_name,
            description,
            input_preview,
        }))],
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
            vec![Action::Attention(AttentionAction::PermissionResolved { session_id, request_id })]
        }
        ServerEvent::SessionEnded { session_id, reason, detail } => {
            vec![Action::Attention(AttentionAction::SessionEnded { session_id, reason, detail })]
        }
        ServerEvent::SoftLimitReached { session_id, account_name, retry_after_secs, .. } => {
            let toast = format!(
                "{} hit the {account_name} soft limit; retry in {retry_after_secs}s",
                short_id(&session_id)
            );
            soft_limit(session_id, true, Level::Error, toast)
        }
        ServerEvent::SoftLimitCleared { session_id } => {
            let toast = format!("{} soft limit cleared", short_id(&session_id));
            soft_limit(session_id, false, Level::Info, toast)
        }
        ServerEvent::LimitResetRedeemed { .. } => limit_reset_redeemed(event),
        ServerEvent::ToolCallBlocked { .. } => tool_call_blocked(event),
        ServerEvent::MachineLiveness { machine_id, liveness } => {
            vec![Action::SessionLive(SessionLiveAction::MachineLiveness {
                machine_id: machine_id.to_string(),
                liveness,
            })]
        }
        // The socket lagged: the REST refresh is the TUI's whole-state resync,
        // so ask for one now rather than waiting for the poll.
        ServerEvent::Resync { .. } => vec![Action::RefreshSessions],

        ServerEvent::ArchiveManifest { .. } => waived("the TUI has no archive browser"),
        ServerEvent::ArchiveUploaded { .. } => waived("the TUI does not browse archives"),
        ServerEvent::CommandResult { command_id, ok, error, session_id } => {
            command_result(&command_id, ok, error, session_id)
        }
        ServerEvent::MessageAck { client_msg_id, ok, error, command_id, .. } => {
            vec![Action::Send(SendAction::Acked { client_msg_id, ok, error, command_id })]
        }
        ServerEvent::AskQuestion { session_id, question, questions, preamble } => {
            vec![Action::Prompt(PromptAction::AskRequested {
                session_id,
                question,
                questions,
                preamble,
            })]
        }
        ServerEvent::AskResolved { session_id } => {
            vec![Action::Prompt(PromptAction::AskResolved { session_id })]
        }
        ServerEvent::PlanRequest { session_id, plan, preamble } => {
            vec![Action::Prompt(PromptAction::PlanRequested { session_id, plan, preamble })]
        }
        ServerEvent::PlanResolved { session_id } => {
            vec![Action::Prompt(PromptAction::PlanResolved { session_id })]
        }
        ServerEvent::MachineResources { .. } => waived("the machines view refetches over REST"),
        ServerEvent::DispatcherLiveness { .. } => {
            waived("the dispatchers view refetches over REST")
        }
        ServerEvent::AccountUsage { .. } => {
            waived("the accounts and usage views refetch over REST")
        }
        ServerEvent::GithubEvent { .. } => {
            waived("PR links come from the session rows the REST refresh returns")
        }
        ServerEvent::PtyChunk { session_id, data } => {
            vec![Action::Terminal(TerminalAction::Chunk { session_id, data })]
        }
        ServerEvent::ScheduledLaunch { .. } => {
            waived("a draft is a session row the list refresh carries")
        }
        ServerEvent::RoomMembers { .. } => waived("the TUI list does not group by room"),
        ServerEvent::UserActions { .. } => waived("no needs-you list; the counts ride on the rows"),
        ServerEvent::Heartbeat { .. } => waived("liveness tick with nothing to render"),
    }
}

/// A soft-limit frame both marks the row and says so once in the status line.
fn soft_limit(session_id: String, active: bool, level: Level, toast: String) -> Vec<Action> {
    vec![
        Action::Diagnose(DiagnoseAction::SoftLimit { session_id, active }),
        Action::Toast(level, toast),
    ]
}

/// A blocked tool call is only ever news for the status line.
fn tool_call_blocked(event: ServerEvent) -> Vec<Action> {
    let ServerEvent::ToolCallBlocked { session_id, tool_name, rule } = event else {
        return Vec::new();
    };
    vec![Action::Toast(
        Level::Warn,
        format!("{} blocked {tool_name} ({rule})", short_id(&session_id)),
    )]
}

/// An automatic limit-reset claim is only ever news for the status line.
fn limit_reset_redeemed(event: ServerEvent) -> Vec<Action> {
    let ServerEvent::LimitResetRedeemed { account_name, outcome, .. } = event else {
        return Vec::new();
    };
    vec![Action::Toast(Level::Info, format!("{account_name}: limit reset {outcome}"))]
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
    let line = agent_event_to_line(data).map(Box::new);
    Action::StreamLine { session_id, seq: data.seq(), line, usage }
}

#[cfg(test)]
mod tests {
    use cctui_proto::ws::{AgentEvent, ServerEvent};

    use super::{Action, to_actions};

    #[test]
    fn a_heartbeat_moves_the_usage_without_adding_a_line() {
        let event = ServerEvent::Stream {
            session_id: "s-a".to_owned(),
            data: AgentEvent::Heartbeat {
                tokens_in: 5,
                tokens_out: 6,
                cost_usd: 0.2,
                ts: 1,
                seq: Some(3),
            },
        };
        match to_actions(event).as_slice() {
            [Action::StreamLine { line, usage, .. }] => {
                assert!(line.is_none(), "a heartbeat has nothing to render");
                assert!(usage.is_some());
            }
            _ => panic!("expected one stream action"),
        }
    }
}
