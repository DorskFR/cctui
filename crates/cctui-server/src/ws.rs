use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
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

pub async fn agent_stream(
    ws: WebSocketUpgrade,
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    uri: Uri,
) -> Result<impl IntoResponse, StatusCode> {
    let token = extract_token_from_uri(&uri).ok_or(StatusCode::UNAUTHORIZED)?;
    let auth_ctx = state.auth_config.validate(&token).await.ok_or(StatusCode::UNAUTHORIZED)?;

    if auth_ctx.role != TokenRole::Agent {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(ws.on_upgrade(move |socket| handle_agent_stream(socket, session_id, state)))
}

async fn handle_heartbeat(event: &AgentEvent, session_id: &str, state: &AppState) {
    if let AgentEvent::Heartbeat { tokens_in, tokens_out, cost_usd, .. } = event {
        state.registry.write().await.update_heartbeat(
            session_id,
            *tokens_in,
            *tokens_out,
            *cost_usd,
        );
    }
}

async fn store_and_broadcast(event: AgentEvent, session_id: &str, state: &AppState) {
    let stream_tx = {
        let registry = state.registry.read().await;
        registry.get(session_id).map(|h| h.stream_tx.clone())
    };

    let payload = match serde_json::to_value(&event) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(session_id = %session_id, %err, "failed to serialize event");
            return;
        }
    };

    let event_type = match &event {
        AgentEvent::Text { .. } => "text",
        AgentEvent::ToolCall { .. } => "tool_call",
        AgentEvent::ToolResult { .. } => "tool_result",
        AgentEvent::Heartbeat { .. } => "heartbeat",
        AgentEvent::Reply { .. } => "reply",
        AgentEvent::TurnEnd { .. } => "turn_end",
    };

    let _ = sqlx::query(
        "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, $2, $3)",
    )
    .bind(session_id)
    .bind(event_type)
    .bind(payload)
    .execute(&state.pool)
    .await;

    if let Some(tx) = stream_tx {
        let _ = tx.send(event);
    }
}

async fn dispatch_event(event: AgentEvent, session_id: &str, state: &AppState) {
    if matches!(event, AgentEvent::Heartbeat { .. }) {
        handle_heartbeat(&event, session_id, state).await;
    } else {
        store_and_broadcast(event, session_id, state).await;
    }
}

/// Returns `None` to continue, `Some(true)` to break, `Some(false)` for a parsed text message.
const fn classify_message(msg: &Result<Message, axum::Error>) -> Option<bool> {
    match msg {
        Ok(Message::Close(_)) | Err(_) => Some(true),
        Ok(Message::Text(_)) => None,
        _ => Some(false),
    }
}

fn extract_text(msg: Result<Message, axum::Error>) -> Option<String> {
    if let Ok(Message::Text(t)) = msg { Some(t.to_string()) } else { None }
}

async fn run_agent_socket(socket: &mut WebSocket, session_id: &str, state: &AppState) {
    while let Some(msg) = socket.next().await {
        match classify_message(&msg) {
            Some(true) => break,
            Some(false) => continue,
            None => {}
        }

        let Some(text) = extract_text(msg) else { continue };

        let event: AgentEvent = match serde_json::from_str(&text) {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(session_id = %session_id, %err, "failed to parse AgentEvent");
                continue;
            }
        };

        dispatch_event(event, session_id, state).await;
    }
}

async fn handle_agent_stream(mut socket: WebSocket, session_id: String, state: AppState) {
    let exists = {
        let registry = state.registry.read().await;
        registry.get(&session_id).is_some()
    };

    if !exists {
        tracing::warn!(session_id = %session_id, "agent_stream: session not found");
        return;
    }

    run_agent_socket(&mut socket, &session_id, &state).await;

    tracing::info!(session_id = %session_id, "agent stream disconnected");
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

async fn handle_subscribe(
    session_id: String,
    state: &AppState,
    event_tx: &mpsc::Sender<ServerEvent>,
    sub_handles: &mut Vec<tokio::task::JoinHandle<()>>,
) {
    let receiver = {
        let registry = state.registry.read().await;
        registry.subscribe(&session_id)
    };

    if let Some(receiver) = receiver {
        let handle = spawn_relay_task(receiver, session_id, event_tx.clone());
        sub_handles.push(handle);
    } else {
        // Historical/terminated sessions won't be in the registry — this is expected
        tracing::debug!(session_id = %session_id, "tui_ws: session not in registry (historical)");
    }
}

async fn run_tui_socket(
    mut stream: futures_util::stream::SplitStream<WebSocket>,
    state: AppState,
    event_tx: mpsc::Sender<ServerEvent>,
) {
    let mut sub_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

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
            TuiCommand::Unsubscribe { .. } => {}
            TuiCommand::Message { session_id, content } => {
                let mut registry = state.registry.write().await;
                registry.queue_message(&session_id, content);
            }
            TuiCommand::PermissionResponse { session_id, request_id, behavior } => {
                tracing::info!(
                    session_id = %session_id,
                    request_id = %request_id,
                    behavior = %behavior,
                    "TUI permission response received"
                );
                let stored_session_id =
                    state.permission_store.write().await.record_decision(&request_id, behavior);
                // Prefer the id attached at submission; fall back to the one
                // the client sent (stale / unknown request_id cases).
                let resolved_session_id =
                    if stored_session_id.is_empty() { session_id } else { stored_session_id };
                let _ = state.tui_tx.send(ServerEvent::PermissionResolved {
                    session_id: resolved_session_id,
                    request_id,
                });
            }
        }
    }

    for handle in sub_handles {
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
