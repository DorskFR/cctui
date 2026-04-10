use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use crate::state::AppState;

/// `PostToolUse` hook payload from Claude Code.
/// Fires after a tool executes successfully with the full untruncated tool result.
#[derive(Debug, serde::Deserialize)]
pub struct PostToolUsePayload {
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    pub tool_response: Option<serde_json::Value>,
    #[serde(flatten)]
    pub _extra: serde_json::Map<String, serde_json::Value>,
}

pub async fn post_tool_use(
    State(state): State<AppState>,
    Json(req): Json<PostToolUsePayload>,
) -> StatusCode {
    let Some(sid) = req.session_id.as_deref() else {
        return StatusCode::OK;
    };

    let tool_name = req.tool_name.as_deref().unwrap_or("unknown");

    tracing::info!(
        session_id = %sid,
        tool_name = %tool_name,
        "PostToolUse hook"
    );

    let ts = chrono::Utc::now().timestamp();

    // Extract output summary from tool_response
    let output_summary = req
        .tool_response
        .as_ref()
        .and_then(|v| v.as_str())
        .map(|s| if s.len() > 500 { format!("{}…", &s[..500]) } else { s.to_string() })
        .or_else(|| {
            req.tool_response.as_ref().map(|v| {
                let s = v.to_string();
                if s.len() > 500 { format!("{}…", &s[..500]) } else { s }
            })
        })
        .unwrap_or_default();

    let payload = serde_json::json!({
        "type": "tool_result",
        "tool": tool_name,
        "input": req.tool_input,
        "response": req.tool_response,
        "output_summary": output_summary,
        "ts": ts,
    });

    let _ = sqlx::query(
        "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, $2, $3)",
    )
    .bind(sid)
    .bind("tool_result")
    .bind(&payload)
    .execute(&state.pool)
    .await;

    {
        let registry = state.registry.read().await;
        if let Some(handle) = registry.get(sid) {
            let event = cctui_proto::ws::AgentEvent::ToolResult {
                tool: tool_name.to_string(),
                output_summary,
                ts,
            };
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
