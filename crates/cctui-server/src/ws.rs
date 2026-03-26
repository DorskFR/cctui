use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use cctui_proto::ws::AgentEvent;
use futures_util::StreamExt;
use uuid::Uuid;

use crate::state::AppState;

pub async fn agent_stream(
    ws: WebSocketUpgrade,
    Path(session_id): Path<Uuid>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_agent_stream(socket, session_id, state))
}

async fn handle_heartbeat(event: &AgentEvent, session_id: Uuid, state: &AppState) {
    if let AgentEvent::Heartbeat { tokens_in, tokens_out, cost_usd, .. } = event {
        state.registry.write().await.update_heartbeat(
            &session_id,
            *tokens_in,
            *tokens_out,
            *cost_usd,
        );
    }
}

async fn store_and_broadcast(event: AgentEvent, session_id: Uuid, state: &AppState) {
    let stream_tx = {
        let registry = state.registry.read().await;
        registry.get(&session_id).map(|h| h.stream_tx.clone())
    };

    let payload = match serde_json::to_value(&event) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(session_id = %session_id, %err, "failed to serialize event");
            return;
        }
    };

    let _ = sqlx::query("INSERT INTO stream_events (session_id, payload) VALUES ($1, $2)")
        .bind(session_id)
        .bind(payload)
        .execute(&state.pool)
        .await;

    if let Some(tx) = stream_tx {
        let _ = tx.send(event);
    }
}

async fn dispatch_event(event: AgentEvent, session_id: Uuid, state: &AppState) {
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

async fn run_agent_socket(socket: &mut WebSocket, session_id: Uuid, state: &AppState) {
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

async fn handle_agent_stream(mut socket: WebSocket, session_id: Uuid, state: AppState) {
    let exists = {
        let registry = state.registry.read().await;
        registry.get(&session_id).is_some()
    };

    if !exists {
        tracing::warn!(session_id = %session_id, "agent_stream: session not found");
        return;
    }

    run_agent_socket(&mut socket, session_id, &state).await;

    tracing::info!(session_id = %session_id, "agent stream disconnected");
}
