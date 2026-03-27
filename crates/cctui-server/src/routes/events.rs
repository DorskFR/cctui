use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use uuid::Uuid;

use cctui_proto::ws::AgentEvent;

use crate::state::AppState;

/// Event from the transcript streamer.
#[derive(Debug, serde::Deserialize)]
pub struct StreamerEvent {
    #[allow(dead_code)]
    pub session_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub input: Option<serde_json::Value>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub ts: i64,
}

pub async fn ingest(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
    Json(event): Json<StreamerEvent>,
) -> StatusCode {
    // Map streamer event to AgentEvent for broadcast
    let agent_event = match event.event_type.as_str() {
        "user_message" => Some(AgentEvent::Text {
            content: format!("▷ User: {}", event.content.as_deref().unwrap_or("")),
            ts: event.ts,
        }),
        "assistant_message" => Some(AgentEvent::Text {
            content: event.content.clone().unwrap_or_default(),
            ts: event.ts,
        }),
        "tool_call" => Some(AgentEvent::ToolCall {
            tool: event.tool.clone().unwrap_or_default(),
            input: event.input.clone().unwrap_or_default(),
            ts: event.ts,
        }),
        "tool_result" => Some(AgentEvent::ToolResult {
            tool: event.tool_use_id.clone().unwrap_or_default(),
            output_summary: event.content.as_deref().unwrap_or("").chars().take(200).collect(),
            ts: event.ts,
        }),
        _ => None,
    };

    if let Some(ref ae) = agent_event {
        // Store in DB
        let payload = serde_json::to_value(ae).unwrap_or_default();
        let _ = sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, $2, $3)",
        )
        .bind(session_id)
        .bind(&event.event_type)
        .bind(&payload)
        .execute(&state.pool)
        .await;

        // Broadcast to TUI
        let registry = state.registry.read().await;
        if let Some(handle) = registry.get(&session_id) {
            let _ = handle.stream_tx.send(ae.clone());
        }

        // Update heartbeat
        drop(registry);
        let mut registry = state.registry.write().await;
        if let Some(handle) = registry.get_mut(&session_id) {
            handle.last_heartbeat = std::time::Instant::now();
            handle.session.last_heartbeat = chrono::Utc::now();
        }
    }

    StatusCode::OK
}
