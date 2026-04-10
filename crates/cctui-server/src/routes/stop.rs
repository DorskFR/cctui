use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use crate::state::AppState;

/// `Stop` hook payload from Claude Code.
/// Fires after every assistant turn (stop token emitted), not just session end.
#[derive(Debug, serde::Deserialize)]
pub struct StopPayload {
    pub session_id: Option<String>,
    #[serde(flatten)]
    pub _extra: serde_json::Map<String, serde_json::Value>,
}

pub async fn stop(State(state): State<AppState>, Json(req): Json<StopPayload>) -> StatusCode {
    let Some(sid) = req.session_id.as_deref() else {
        return StatusCode::OK;
    };

    tracing::info!(session_id = %sid, "Stop hook: assistant turn complete");

    let ts = chrono::Utc::now().timestamp();

    let payload = serde_json::json!({
        "type": "turn_end",
        "ts": ts,
    });

    let _ = sqlx::query(
        "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, $2, $3)",
    )
    .bind(sid)
    .bind("turn_end")
    .bind(&payload)
    .execute(&state.pool)
    .await;

    {
        let registry = state.registry.read().await;
        if let Some(handle) = registry.get(sid) {
            let event = cctui_proto::ws::AgentEvent::TurnEnd { ts };
            let _ = handle.stream_tx.send(event);
        }
    }

    // Update heartbeat to keep session alive
    {
        let mut registry = state.registry.write().await;
        if let Some(handle) = registry.get_mut(sid) {
            handle.last_heartbeat = std::time::Instant::now();
            handle.session.last_heartbeat = chrono::Utc::now();
        }
    }

    StatusCode::OK
}
