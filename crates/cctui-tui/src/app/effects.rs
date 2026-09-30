use std::sync::{Arc, Mutex};

use cctui_proto::ws::{AgentEvent, TuiCommand};
use tokio::sync::mpsc;

use super::action::{Action, Effect};
use super::line::agent_event_to_line;
use super::toast::Level;
use crate::client::ServerClient;

const QUEUE: usize = 256;

/// Handle onto the effects worker. [`Effects::dispatch`] never awaits, so the
/// key-handling path never blocks on HTTP or the websocket.
pub struct Effects {
    tx: mpsc::Sender<Effect>,
    commands: Arc<Mutex<mpsc::Sender<TuiCommand>>>,
}

impl Effects {
    /// Effects are executed one at a time: two messages typed in quick
    /// succession must reach the server in the order they were sent.
    pub fn start(
        server: Arc<ServerClient>,
        commands: mpsc::Sender<TuiCommand>,
    ) -> (Self, mpsc::Receiver<Action>) {
        let (tx, mut rx) = mpsc::channel::<Effect>(QUEUE);
        let (action_tx, action_rx) = mpsc::channel::<Action>(QUEUE);
        let commands = Arc::new(Mutex::new(commands));
        let worker_commands = Arc::clone(&commands);

        tokio::spawn(async move {
            while let Some(effect) = rx.recv().await {
                for action in run(&server, &worker_commands, effect).await {
                    if action_tx.send(action).await.is_err() {
                        return;
                    }
                }
            }
        });

        (Self { tx, commands }, action_rx)
    }

    /// Reconnects hand over a fresh command sender; effects already queued pick
    /// up the new one.
    pub fn set_commands(&self, commands: mpsc::Sender<TuiCommand>) {
        if let Ok(mut slot) = self.commands.lock() {
            *slot = commands;
        }
    }

    pub fn dispatch(&self, effect: Effect) {
        if self.tx.try_send(effect).is_err() {
            tracing::warn!("effect queue full; dropping effect");
        }
    }

    pub fn dispatch_all(&self, effects: Vec<Effect>) {
        for effect in effects {
            self.dispatch(effect);
        }
    }
}

fn command_sender(
    commands: &Arc<Mutex<mpsc::Sender<TuiCommand>>>,
) -> Option<mpsc::Sender<TuiCommand>> {
    commands.lock().ok().map(|slot| slot.clone())
}

async fn send_command(commands: &Arc<Mutex<mpsc::Sender<TuiCommand>>>, command: TuiCommand) {
    if let Some(tx) = command_sender(commands) {
        let _ = tx.send(command).await;
    }
}

async fn run(
    server: &ServerClient,
    commands: &Arc<Mutex<mpsc::Sender<TuiCommand>>>,
    effect: Effect,
) -> Vec<Action> {
    match effect {
        Effect::RefreshSessions => match server.list_sessions().await {
            Ok(resp) => vec![Action::SessionsLoaded(resp.sessions)],
            Err(e) => {
                tracing::warn!(%e, "session refresh failed");
                vec![Action::Toast(Level::Warn, "session refresh failed".to_owned())]
            }
        },
        Effect::LoadConversation { session_id, fetch } => {
            let mut actions = Vec::new();
            if fetch {
                actions.extend(load_conversation(server, &session_id).await);
            }
            send_command(commands, TuiCommand::Subscribe { session_id }).await;
            actions
        }
        Effect::Subscribe { session_id } => {
            send_command(commands, TuiCommand::Subscribe { session_id }).await;
            Vec::new()
        }
        Effect::SendMessage { session_id, content } => {
            send_command(
                commands,
                TuiCommand::Message {
                    session_id,
                    content,
                    client_msg_id: None,
                    ask_picks: None,
                    turn_id: None,
                },
            )
            .await;
            Vec::new()
        }
        Effect::Interrupt { session_id } => match server.interrupt_session(&session_id).await {
            Ok(()) => Vec::new(),
            Err(e) => {
                tracing::warn!(%e, "interrupt failed");
                vec![Action::Toast(Level::Error, "interrupt failed".to_owned())]
            }
        },
        Effect::SetAutoApprove { session_id, enabled } => {
            match server.set_auto_approve(&session_id, enabled).await {
                Ok(()) => vec![Action::AutoApproveSet { session_id, enabled }],
                Err(e) => {
                    tracing::warn!(%e, "auto-approve toggle failed");
                    vec![Action::Toast(Level::Error, "auto-approve toggle failed".to_owned())]
                }
            }
        }
        Effect::RespondPermission { session_id, request_id, behavior } => {
            send_command(
                commands,
                TuiCommand::PermissionResponse {
                    session_id,
                    request_id,
                    behavior: behavior.to_owned(),
                },
            )
            .await;
            Vec::new()
        }
    }
}

async fn load_conversation(server: &ServerClient, session_id: &str) -> Vec<Action> {
    let Ok(events) = server.get_conversation(session_id).await else {
        tracing::warn!(session_id, "conversation fetch failed");
        return Vec::new();
    };
    let total = events.len();
    let lines: Vec<_> = events
        .iter()
        .filter_map(|v| serde_json::from_value::<AgentEvent>(v.clone()).ok())
        .map(|e| agent_event_to_line(&e))
        .collect();
    let undecodable = total - lines.len();
    let mut actions = vec![Action::ConversationLoaded { session_id: session_id.to_owned(), lines }];
    if undecodable > 0 {
        actions.push(Action::UndecodableAgentEvents(undecodable));
    }
    actions
}
