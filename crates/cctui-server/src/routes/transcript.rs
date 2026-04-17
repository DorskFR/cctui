use axum::Json;
use axum::extract::{Path, Query, State};
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

#[derive(Debug, serde::Deserialize)]
pub struct FetchParams {
    pub after_id: Option<i64>,
    pub limit: Option<i64>,
}

/// Ingest a raw JSONL transcript line from the channel.
///
/// Stores the line losslessly in `session_transcript`, parses it into `AgentEvents`
/// for live `WebSocket` broadcast, and updates token usage and heartbeat.
#[allow(clippy::significant_drop_tightening)]
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

    // Store raw line in session_transcript (lossless archive).
    // Return 500 on failure so the channel knows the line was not persisted.
    if let Err(e) =
        sqlx::query("INSERT INTO session_transcript (session_id, raw_json) VALUES ($1, $2)")
            .bind(session_uuid)
            .bind(line)
            .execute(&state.pool)
            .await
    {
        tracing::error!(session_id = %session_id, "failed to store transcript line: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    let ts = chrono::Utc::now().timestamp();

    // Parse once; reuse the value for both usage extraction and event parsing.
    let parsed: Option<serde_json::Value> = serde_json::from_str(line).ok();

    // Handle token usage
    if let Some(ref d) = parsed
        && let Some(usage) = transcript_parser::parse_usage_value(d)
    {
        state.registry.write().await.update_token_usage(
            &session_id,
            usage.tokens_in,
            usage.tokens_out,
            usage.cost_usd,
        );
    }

    // Parse line into AgentEvents
    let events =
        parsed.as_ref().map(|d| transcript_parser::parse_line_value(d, ts)).unwrap_or_default();
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

    // Update heartbeat and resurrect the session if it was disconnected.
    let resurrected = state.registry.write().await.touch(&session_id);
    if let Some(status) = resurrected {
        let _ = state
            .tui_tx
            .send(cctui_proto::ws::ServerEvent::Status { session_id: session_id.clone(), status });
    }

    StatusCode::OK
}

const DEFAULT_FETCH_LIMIT: i64 = 500;
const MAX_FETCH_LIMIT: i64 = 2000;

/// Fetch raw transcript lines for a session (for replay and archival).
///
/// Supports cursor-based pagination via `?after_id=<id>&limit=<n>` (default limit: 500, max: 2000).
pub async fn fetch(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<FetchParams>,
) -> Result<Json<Vec<RawTranscriptLine>>, StatusCode> {
    let Ok(session_uuid) = Uuid::parse_str(&session_id) else {
        return Err(StatusCode::BAD_REQUEST);
    };

    let limit = params.limit.unwrap_or(DEFAULT_FETCH_LIMIT).clamp(1, MAX_FETCH_LIMIT);
    let after_id = params.after_id.unwrap_or(0);

    let rows: Vec<(i64, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT id, raw_json, created_at FROM session_transcript \
         WHERE session_id = $1 AND id > $2 ORDER BY id ASC LIMIT $3",
    )
    .bind(session_uuid)
    .bind(after_id)
    .bind(limit)
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
