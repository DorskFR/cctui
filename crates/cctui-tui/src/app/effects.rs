use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cctui_client::{Client, ConversationFetch, Page, WsClient};
use cctui_proto::drafts::{composer_draft_key, session_history_key};
use cctui_proto::ws::AgentEvent;
use tokio::sync::mpsc;

use super::action::{Action, Effect};
use super::attention::AttentionAction;
use super::conversation::ConversationAction;
use super::conversation_store::{PageKind, PageRequest};
use super::diagnose::DiagnoseAction;
use super::drafts::DraftAction;
use super::identity::AuthAction;
use super::line::agent_event_to_line;
use super::send::SendAction;
use super::state::{ConversationLine, PendingPermission};
use super::toast::Level;

const QUEUE: usize = 256;

/// How long a composer sits still before its draft is written, matching the
/// web UI: a keystroke must not be a request.
const DRAFT_DEBOUNCE: Duration = Duration::from_millis(700);

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
            let mut drafts = DraftSaver::new(Arc::clone(&server));
            while let Some(effect) = rx.recv().await {
                for action in run(&server, &ws, &mut drafts, effect).await {
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

/// Per-key debounce for draft writes: a pending save is replaced, not queued,
/// and the request is off the effect queue so typing never waits on it.
struct DraftSaver {
    server: Arc<Client>,
    pending: HashMap<String, tokio::task::JoinHandle<()>>,
}

impl DraftSaver {
    fn new(server: Arc<Client>) -> Self {
        Self { server, pending: HashMap::new() }
    }

    fn save(&mut self, key: String, text: String) {
        self.pending.retain(|_, handle| !handle.is_finished());
        self.cancel(&key);
        let server = Arc::clone(&self.server);
        let target = key.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(DRAFT_DEBOUNCE).await;
            if let Err(e) = server.put_draft(&target, &text).await {
                tracing::warn!(%e, "draft save failed");
            }
        });
        self.pending.insert(key, handle);
    }

    fn cancel(&mut self, key: &str) {
        if let Some(handle) = self.pending.remove(key) {
            handle.abort();
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn run(
    server: &Client,
    ws: &WsClient,
    drafts: &mut DraftSaver,
    effect: Effect,
) -> Vec<Action> {
    match effect {
        Effect::RefreshSessions => match server.list_sessions().await {
            Ok(resp) => vec![Action::SessionsLoaded(resp.sessions)],
            Err(e) if e.is_unauthorized() => vec![Action::Auth(AuthAction::Rejected)],
            Err(e) => {
                tracing::warn!(%e, "session refresh failed");
                vec![Action::Toast(Level::Warn, "session refresh failed".to_owned())]
            }
        },
        Effect::FetchIdentity => match server.me().await {
            Ok(me) => vec![Action::Auth(AuthAction::Identified(Box::new(me)))],
            Err(e) if e.is_unauthorized() => vec![Action::Auth(AuthAction::Rejected)],
            Err(e) => {
                tracing::warn!(%e, "identity fetch failed");
                Vec::new()
            }
        },
        Effect::FetchPendingPermissions => fetch_pending_permissions(server).await,
        Effect::LoadConversationPage { session_id, kind, page, etag } => {
            load_conversation_page(server, &session_id, kind, page, etag.as_deref()).await
        }
        Effect::MarkSeen { session_id } => {
            if let Err(e) = server.mark_seen(&session_id).await {
                tracing::warn!(%e, "marking the session seen failed");
            }
            Vec::new()
        }
        Effect::LoadDraftIndex => load_draft_index(server).await,
        Effect::LoadDrafts { session_id } => load_drafts(server, session_id).await,
        Effect::SaveDraft { key, text } => {
            drafts.save(key, text);
            Vec::new()
        }
        Effect::DiscardDraft { key } => {
            drafts.cancel(&key);
            if let Err(e) = server.delete_draft(&key).await {
                tracing::warn!(%e, "draft discard failed");
            }
            Vec::new()
        }
        Effect::Subscribe { session_id } => {
            subscribe(ws, session_id).await;
            Vec::new()
        }
        Effect::Unsubscribe { session_id } => {
            if let Err(e) = ws.unsubscribe(session_id).await {
                tracing::warn!(%e, "unsubscribe failed");
            }
            Vec::new()
        }
        Effect::SendMessage { send_id, session_id, content, ask_picks, turn_id } => {
            let client_msg_id = uuid::Uuid::new_v4().to_string();
            let turn_id = turn_id.unwrap_or_else(uuid::Uuid::new_v4);
            match ws
                .send_message_as(
                    session_id,
                    content,
                    client_msg_id.clone(),
                    ask_picks,
                    Some(turn_id),
                )
                .await
            {
                Ok(()) => {
                    vec![Action::Send(SendAction::Dispatched { send_id, client_msg_id, turn_id })]
                }
                Err(e) => vec![Action::Send(SendAction::DispatchFailed {
                    send_id,
                    reason: e.to_string(),
                })],
            }
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
        Effect::SaveUiState(state) => {
            crate::config::uistate::save(&state);
            Vec::new()
        }
        Effect::RespondPermission { session_id, request_id, behavior } => {
            if let Err(e) = ws.respond_permission(session_id, request_id, behavior.to_owned()).await
            {
                tracing::warn!(%e, "permission response failed");
            }
            Vec::new()
        }
        Effect::FetchDiagnose { session_id } => match server.diagnose(&session_id).await {
            Ok(report) => {
                vec![Action::Diagnose(DiagnoseAction::Loaded {
                    session_id,
                    report: Box::new(report),
                })]
            }
            Err(e) if e.is_unauthorized() => vec![Action::Auth(AuthAction::Rejected)],
            Err(e) => {
                tracing::warn!(%e, session_id, "diagnose fetch failed");
                vec![Action::Diagnose(DiagnoseAction::Failed { session_id, error: e.to_string() })]
            }
        },
    }
}

async fn load_draft_index(server: &Client) -> Vec<Action> {
    match server.list_drafts().await {
        Ok(list) => vec![Action::Drafts(DraftAction::IndexLoaded(Box::new(list)))],
        Err(e) => {
            tracing::warn!(%e, "draft index fetch failed");
            Vec::new()
        }
    }
}

/// A draft the web UI has since edited must win over the startup index, so the
/// session's two keys are read again as its conversation opens.
async fn load_drafts(server: &Client, session_id: String) -> Vec<Action> {
    let text = server.get_draft(&composer_draft_key(&session_id)).await;
    let history = server.get_draft(&session_history_key(&session_id)).await;
    if let Err(e) = &text {
        tracing::warn!(%e, session_id, "draft fetch failed");
    }
    vec![Action::Drafts(DraftAction::Loaded {
        session_id,
        text: text.unwrap_or_default(),
        history: history.unwrap_or_default(),
    })]
}

async fn fetch_pending_permissions(server: &Client) -> Vec<Action> {
    match server.pending_permissions().await {
        Ok(items) => {
            let items = items
                .into_iter()
                .map(|p| PendingPermission {
                    session_id: p.session_id,
                    request_id: p.request_id,
                    tool_name: p.tool_name,
                    description: p.description,
                    input_preview: p.input_preview,
                })
                .collect();
            vec![Action::Attention(AttentionAction::PendingPermissionsLoaded(items))]
        }
        Err(e) if e.is_unauthorized() => vec![Action::Auth(AuthAction::Rejected)],
        Err(e) => {
            tracing::warn!(%e, "pending permission fetch failed");
            Vec::new()
        }
    }
}

async fn subscribe(ws: &WsClient, session_id: String) {
    if let Err(e) = ws.subscribe(session_id).await {
        tracing::warn!(%e, "subscribe failed");
    }
}

async fn load_conversation_page(
    server: &Client,
    session_id: &str,
    kind: PageKind,
    page: PageRequest,
    etag: Option<&str>,
) -> Vec<Action> {
    let request = Page { before: page.before, after: page.after, limit: page.limit };
    let fetch = match server.conversation(session_id, request, etag).await {
        Ok(fetch) => fetch,
        Err(e) => {
            tracing::warn!(%e, session_id, "conversation fetch failed");
            return vec![Action::Conversation(ConversationAction::Failed {
                session_id: session_id.to_owned(),
                kind,
            })];
        }
    };

    let ConversationFetch::Page { rows, etag, has_more } = fetch else {
        return vec![Action::Conversation(ConversationAction::NotModified {
            session_id: session_id.to_owned(),
            kind,
        })];
    };

    let total = rows.len();
    let mut undecodable = 0;
    let mut decoded: Vec<(i64, ConversationLine)> = Vec::with_capacity(total);
    for row in rows {
        let seq = row.seq;
        match serde_json::from_value::<AgentEvent>(row.event) {
            // An event with nothing to render is not a decoding failure.
            Ok(event) => decoded.extend(agent_event_to_line(&event).map(|line| (seq, line))),
            Err(_) => undecodable += 1,
        }
    }

    let mut actions = vec![Action::Conversation(ConversationAction::Loaded {
        session_id: session_id.to_owned(),
        kind,
        rows: decoded,
        etag,
        has_more,
    })];
    if undecodable > 0 {
        actions.push(Action::UndecodableAgentEvents(undecodable));
    }
    actions
}
