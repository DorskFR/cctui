use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use cctui_proto::ws::AgentEvent;
use uuid::Uuid;

use crate::state::AppState;
use crate::transcript_parser;

#[derive(Debug, serde::Deserialize)]
pub struct TranscriptLine {
    pub line: String,
}

#[derive(Debug, serde::Serialize)]
pub struct RawTranscriptLine {
    pub id: i64,
    pub raw_json: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Ingest a raw JSONL transcript line from the channel.
///
/// Stores the line losslessly in session_transcript, parses it into AgentEvents
/// for live WebSocket broadcast, and updates token usage and heartbeat.
pub async fn ingest(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<TranscriptLine>,
) -> StatusCode {
    let line = body.line.trim();
    if line.is_empty() {
        return StatusCode::OK;
    }

    let Ok(session_uuid) = Uuid::parse_str(&session_id) else {
        return StatusCode::BAD_REQUEST;
    };

    // Store raw line in session_transcript (lossless archive)
    if let Err(e) = sqlx::query(
        "INSERT INTO session_transcript (session_id, raw_json) VALUES ($1, $2)",
    )
    .bind(session_uuid)
    .bind(line)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(session_id = %session_id, "failed to store transcript line: {e}");
    }

    let ts = chrono::Utc::now().timestamp();

    // Handle token usage
    if let Some(usage) = transcript_parser::parse_usage(line) {
        state.registry.write().await.update_token_usage(
            &session_id,
            usage.tokens_in,
            usage.tokens_out,
            usage.cost_usd,
        );
    }

    // Parse line into AgentEvents
    let events = transcript_parser::parse_line(line, ts);
    if events.is_empty() {
        return StatusCode::OK;
    }

    // Store parsed events in stream_events (for history queries) and broadcast to TUI.
    // Do DB inserts first, then acquire registry lock briefly for broadcast.
    for event in &events {
        let event_type = match event {
            AgentEvent::Text { .. } => "text",
            AgentEvent::ToolCall { .. } => "tool_call",
            AgentEvent::ToolResult { .. } => "tool_result",
            AgentEvent::Heartbeat { .. } => "heartbeat",
            AgentEvent::Reply { .. } => "reply",
            AgentEvent::TurnEnd { .. } => "turn_end",
        };
        let payload = serde_json::to_value(event).unwrap_or_default();
        let _ = sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, $2, $3)",
        )
        .bind(session_uuid)
        .bind(event_type)
        .bind(&payload)
        .execute(&state.pool)
        .await;
    }

    {
        let registry = state.registry.read().await;
        if let Some(handle) = registry.get(&session_id) {
            for event in &events {
                let _ = handle.stream_tx.send(event.clone());
            }
        }
    }

    // Update heartbeat
    let mut registry = state.registry.write().await;
    if let Some(handle) = registry.get_mut(&session_id) {
        handle.last_heartbeat = std::time::Instant::now();
        handle.session.last_heartbeat = chrono::Utc::now();
    }

    StatusCode::OK
}

/// Fetch raw transcript lines for a session (for full replay and archival).
pub async fn fetch(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<RawTranscriptLine>>, StatusCode> {
    let Ok(session_uuid) = Uuid::parse_str(&session_id) else {
        return Err(StatusCode::BAD_REQUEST);
    };

    let rows: Vec<(i64, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT id, raw_json, created_at FROM session_transcript \
         WHERE session_id = $1 ORDER BY id ASC",
    )
    .bind(session_uuid)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error fetching transcript: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(
        rows.into_iter()
            .map(|(id, raw_json, created_at)| RawTranscriptLine { id, raw_json, created_at })
            .collect(),
    ))
}
