use axum::Json;
use axum::extract::State;

use cctui_proto::api::{CheckResponse, HookOutput};

use crate::state::AppState;

/// `PreToolUse` hook payload from Claude Code.
#[derive(Debug, serde::Deserialize)]
pub struct PreToolUsePayload {
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    #[serde(flatten)]
    pub _extra: serde_json::Map<String, serde_json::Value>,
}

pub async fn check(
    State(state): State<AppState>,
    Json(req): Json<PreToolUsePayload>,
) -> Json<CheckResponse> {
    tracing::info!(
        session_id = ?req.session_id,
        tool_name = ?req.tool_name,
        "PreToolUse check"
    );

    let session_id = req.session_id.as_deref();

    // Store tool call as a stream event and broadcast to TUI subscribers
    if let Some(sid) = session_id {
        let tool_name = req.tool_name.as_deref().unwrap_or("unknown");
        let tool_input = req.tool_input.clone().unwrap_or_default();

        let payload = serde_json::json!({
            "type": "tool_call",
            "tool": tool_name,
            "input": tool_input,
            "ts": chrono::Utc::now().timestamp()
        });

        let _ = sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, $2, $3)",
        )
        .bind(sid)
        .bind("tool_call")
        .bind(&payload)
        .execute(&state.pool)
        .await;

        // Broadcast to live TUI subscribers
        {
            let registry = state.registry.read().await;
            if let Some(handle) = registry.get(sid) {
                let event = cctui_proto::ws::AgentEvent::ToolCall {
                    tool: tool_name.to_string(),
                    input: tool_input,
                    ts: chrono::Utc::now().timestamp(),
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
    }

    // Evaluate policy
    let decision = {
        let registry = state.registry.read().await;
        session_id.and_then(|sid| registry.get(sid)).map_or(
            crate::policy::PolicyDecision::Allow,
            |handle| {
                crate::policy::evaluate(
                    &handle.policy_rules,
                    req.tool_name.as_deref().unwrap_or("unknown"),
                    &req.tool_input.clone().unwrap_or_default(),
                )
            },
        )
    };

    match decision {
        crate::policy::PolicyDecision::Allow => Json(CheckResponse {
            hook_specific_output: HookOutput {
                hook_event_name: "PreToolUse".into(),
                permission_decision: "allow".into(),
                permission_decision_reason: None,
            },
        }),
        crate::policy::PolicyDecision::Deny { reason } => Json(CheckResponse {
            hook_specific_output: HookOutput {
                hook_event_name: "PreToolUse".into(),
                permission_decision: "deny".into(),
                permission_decision_reason: Some(reason),
            },
        }),
    }
}
