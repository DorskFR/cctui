use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::{SessionStatus, TokenUsage};

// --- Agent-facing ---

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub machine_id: String,
    pub working_dir: String,
    pub claude_session_id: Option<String>,
    pub parent_session_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub session_id: Uuid,
    pub ws_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CheckRequest {
    pub session_id: Uuid,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CheckResponse {
    #[serde(rename = "hookSpecificOutput")]
    pub hook_specific_output: HookOutput,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HookOutput {
    #[serde(rename = "hookEventName")]
    pub hook_event_name: String,
    #[serde(rename = "permissionDecision")]
    pub permission_decision: String,
    #[serde(rename = "permissionDecisionReason", skip_serializing_if = "Option::is_none")]
    pub permission_decision_reason: Option<String>,
}

// --- TUI-facing ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListItem {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub machine_id: String,
    pub working_dir: String,
    pub status: SessionStatus,
    pub uptime_secs: i64,
    pub token_usage: TokenUsage,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionListItem>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageRequest {
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}
