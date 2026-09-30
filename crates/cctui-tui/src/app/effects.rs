use std::sync::Arc;

use cctui_client::{Client, ConversationFetch, Page, WsClient};
use cctui_proto::ws::AgentEvent;
use tokio::sync::mpsc;

use super::action::{Action, Effect};
use super::line::agent_event_to_line;
use super::toast::Level;

const QUEUE: usize = 256;

/// Handle onto the effects worker. [`Effects::dispatch`] never awaits, so the
/// key-handling path never blocks on HTTP or the websocket.
pub struct Effects {
    tx: mpsc::Sender<Effect>,
}

impl Effects {
    /// Effects are executed one at a time: two messages typed in quick
    /// succession must reach the server in the order they were sent.
    pub fn start(server: Arc<Client>, ws: Arc<WsClient>) -> (Self, mpsc::Receiver<Action>) {
        let (tx, mut rx) = mpsc::channel::<Effect>(QUEUE);
        let (action_tx, action_rx) = mpsc::channel::<Action>(QUEUE);

        tokio::spawn(async move {
            while let Some(effect) = rx.recv().await {
                for action in run(&server, &ws, effect).await {
                    if action_tx.send(action).await.is_err() {
                        return;
                    }
                }
            }
        });

        (Self { tx }, action_rx)
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

async fn run(server: &Client, ws: &WsClient, effect: Effect) -> Vec<Action> {
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
            subscribe(ws, session_id).await;
            actions
        }
        Effect::Subscribe { session_id } => {
            subscribe(ws, session_id).await;
            Vec::new()
        }
        Effect::SendMessage { session_id, content } => {
            if let Err(e) = ws.send_message(session_id, content, None, None).await {
                tracing::warn!(%e, "message send failed");
                return vec![Action::Toast(Level::Error, "message send failed".to_owned())];
            }
            Vec::new()
        }
        Effect::Interrupt { session_id } => match server.interrupt(&session_id).await {
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
            if let Err(e) =
                ws.respond_permission(session_id, request_id, behavior.to_owned()).await
            {
                tracing::warn!(%e, "permission response failed");
            }
            Vec::new()
        }
    }
}

async fn subscribe(ws: &WsClient, session_id: String) {
    if let Err(e) = ws.subscribe(session_id).await {
        tracing::warn!(%e, "subscribe failed");
    }
}

async fn load_conversation(server: &Client, session_id: &str) -> Vec<Action> {
    let fetched = match server.conversation(session_id, Page::default(), None).await {
        Ok(fetched) => fetched,
        Err(e) => {
            tracing::warn!(%e, session_id, "conversation fetch failed");
            return Vec::new();
        }
    };
    let ConversationFetch::Page { rows, .. } = fetched else { return Vec::new() };
    let total = rows.len();
    let lines: Vec<_> = rows
        .iter()
        .filter_map(|row| serde_json::from_value::<AgentEvent>(row.event.clone()).ok())
        .map(|e| agent_event_to_line(&e))
        .collect();
    let undecodable = total - lines.len();
    let mut actions = vec![Action::ConversationLoaded { session_id: session_id.to_owned(), lines }];
    if undecodable > 0 {
        actions.push(Action::UndecodableAgentEvents(undecodable));
    }
    actions
}
