use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{StatusCode, Uri};
use axum::response::IntoResponse;
use cctui_proto::ws::{AgentEvent, ServerEvent, TuiCommand};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::auth::TokenRole;
use crate::state::AppState;

fn extract_token_from_uri(uri: &Uri) -> Option<String> {
    uri.query().and_then(|q| {
        q.split('&').find_map(|param| {
            let mut parts = param.split('=');
            match (parts.next(), parts.next()) {
                (Some("token"), Some(token)) => Some(token.to_string()),
                _ => None,
            }
        })
    })
}

// --- TUI WebSocket ---

pub async fn tui_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    uri: Uri,
) -> Result<impl IntoResponse, StatusCode> {
    let token = extract_token_from_uri(&uri).ok_or(StatusCode::UNAUTHORIZED)?;
    let auth_ctx = state.auth_config.validate(&token).await.ok_or(StatusCode::UNAUTHORIZED)?;

    if !matches!(auth_ctx.role, TokenRole::Admin | TokenRole::User) {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(ws.on_upgrade(move |socket| handle_tui_ws(socket, state)))
}

fn spawn_send_task(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    mut rx: mpsc::Receiver<ServerEvent>,
) {
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let text = match serde_json::to_string(&event) {
                Ok(t) => t,
                Err(err) => {
                    tracing::warn!(%err, "failed to serialize ServerEvent");
                    continue;
                }
            };
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
}

fn spawn_relay_task(
    mut receiver: tokio::sync::broadcast::Receiver<AgentEvent>,
    session_id: String,
    event_tx: mpsc::Sender<ServerEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(agent_event) => {
                    let server_event =
                        ServerEvent::Stream { session_id: session_id.clone(), data: agent_event };
                    if event_tx.send(server_event).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(session_id = %session_id, skipped = n, "TUI receiver lagged");
                }
            }
        }
    })
}

/// Dispatch a client-typed reply to the session's daemon and, when the client
/// opted in with a `client_msg_id`, ack the outcome back to this socket so the
/// UI can show a precise delivery state (sending → delivered / failed) instead
/// of optimistically assuming a sent frame was delivered (CCT-212).
async fn handle_message(
    state: &AppState,
    event_tx: &mpsc::Sender<ServerEvent>,
    session_id: String,
    content: String,
    client_msg_id: Option<String>,
) {
    // NoDaemon / NoAdapter are expected for sessions whose daemon is momentarily
    // offline — that is exactly the case the ack lets the client recover from.
    let dispatch = crate::daemon_dispatch::dispatch(
        state,
        &session_id,
        cctui_proto::adapter::AdapterCommand::Reply { local_id: session_id.clone(), text: content },
    )
    .await;
    let err_reason = dispatch.as_ref().err().map(|err| {
        use crate::daemon_dispatch::Error;
        match err {
            Error::NoDaemon(_) | Error::NoAdapter | Error::NotFound => {
                tracing::debug!(%session_id, ?err, "daemon dispatch skipped");
            }
            _ => tracing::warn!(%session_id, %err, "daemon dispatch failed"),
        }
        err.to_string()
    });
    if let Some(client_msg_id) = client_msg_id {
        let _ = event_tx
            .send(ServerEvent::MessageAck {
                session_id,
                client_msg_id,
                ok: err_reason.is_none(),
                error: err_reason,
            })
            .await;
    }
}

async fn handle_subscribe(
    session_id: String,
    state: &AppState,
    event_tx: &mpsc::Sender<ServerEvent>,
    sub_handles: &mut std::collections::HashMap<String, tokio::task::JoinHandle<()>>,
) {
    let receiver = {
        let registry = state.registry.read().await;
        registry.subscribe(&session_id)
    };

    if let Some(receiver) = receiver {
        let handle = spawn_relay_task(receiver, session_id.clone(), event_tx.clone());
        // Abort any prior relay task for this session on this socket before
        // replacing it. The client re-subscribes on every tab focus/visibility
        // change (CCT-182); without this, each resubscribe leaked an extra
        // relay task that re-delivered every event, duplicating chat messages.
        if let Some(old) = sub_handles.insert(session_id, handle) {
            old.abort();
        }
    } else {
        // Historical/terminated sessions won't be in the registry — this is expected
        tracing::debug!(session_id = %session_id, "tui_ws: session not in registry (historical)");
    }
}

#[allow(clippy::cognitive_complexity)]
async fn run_tui_socket(
    mut stream: futures_util::stream::SplitStream<WebSocket>,
    state: AppState,
    event_tx: mpsc::Sender<ServerEvent>,
) {
    // Relay tasks keyed by session id, so a resubscribe replaces (not stacks)
    // the per-session relay and an unsubscribe can tear it down (CCT-182).
    let mut sub_handles: std::collections::HashMap<String, tokio::task::JoinHandle<()>> =
        std::collections::HashMap::new();

    while let Some(msg) = stream.next().await {
        let text = match msg {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) | Err(_) => break,
            _ => continue,
        };

        let cmd: TuiCommand = match serde_json::from_str(&text) {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!(%err, "failed to parse TuiCommand");
                continue;
            }
        };

        match cmd {
            TuiCommand::Subscribe { session_id } => {
                handle_subscribe(session_id, &state, &event_tx, &mut sub_handles).await;
            }
            TuiCommand::Unsubscribe { session_id } => {
                // Tear down this session's relay task so it stops delivering
                // events to this socket (CCT-182).
                if let Some(handle) = sub_handles.remove(&session_id) {
                    handle.abort();
                }
            }
            TuiCommand::Message { session_id, content, client_msg_id } => {
                handle_message(&state, &event_tx, session_id, content, client_msg_id).await;
            }
            TuiCommand::PermissionResponse { session_id, request_id, behavior } => {
                tracing::info!(
                    session_id = %session_id,
                    request_id = %request_id,
                    behavior = %behavior,
                    "TUI permission response received"
                );
                let allow = {
                    let b = behavior.to_ascii_lowercase();
                    b.starts_with("allow") || b == "accept" || b == "approved"
                };
                let stored_session_id =
                    state.permission_store.write().await.record_decision(&request_id, behavior);
                // Prefer the id attached at submission; fall back to the one
                // the client sent (stale / unknown request_id cases).
                let resolved_session_id =
                    if stored_session_id.is_empty() { session_id } else { stored_session_id };
                // Push the decision down to the adapter so blocking agents
                // (e.g. the codex app-server, which holds the turn open until
                // it gets a reply) are unblocked.
                let dispatch = crate::daemon_dispatch::dispatch(
                    &state,
                    &resolved_session_id,
                    cctui_proto::adapter::AdapterCommand::PermissionResponse {
                        local_id: resolved_session_id.clone(),
                        request_id: request_id.clone(),
                        allow,
                    },
                )
                .await;
                if let Err(err) = dispatch {
                    use crate::daemon_dispatch::Error;
                    match err {
                        Error::NoDaemon(_) | Error::NoAdapter | Error::NotFound => {
                            tracing::debug!(%resolved_session_id, ?err, "permission dispatch skipped");
                        }
                        _ => {
                            tracing::warn!(%resolved_session_id, %err, "permission dispatch failed");
                        }
                    }
                }
                let _ = state.tui_tx.send(ServerEvent::PermissionResolved {
                    session_id: resolved_session_id,
                    request_id,
                });
            }
        }
    }

    for (_, handle) in sub_handles {
        handle.abort();
    }
}

fn spawn_server_event_relay(
    mut receiver: tokio::sync::broadcast::Receiver<ServerEvent>,
    event_tx: mpsc::Sender<ServerEvent>,
) {
    tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if event_tx.send(event).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "TUI server-event relay lagged");
                }
            }
        }
    });
}

async fn handle_tui_ws(socket: WebSocket, state: AppState) {
    let (sink, stream) = socket.split();
    let (tx, rx) = mpsc::channel::<ServerEvent>(256);

    // Relay server-initiated events (e.g. permission requests) to this TUI client
    spawn_server_event_relay(state.tui_tx.subscribe(), tx.clone());

    spawn_send_task(sink, rx);
    run_tui_socket(stream, state, tx).await;

    tracing::info!("TUI WebSocket disconnected");
}
