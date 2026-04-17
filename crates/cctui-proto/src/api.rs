use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::{SessionStatus, TokenUsage};

// --- Agent-facing ---

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub machine_id: String,
    pub working_dir: String,
    pub claude_session_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub session_id: String,
    pub ws_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CheckRequest {
    pub session_id: String,
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
    pub id: String,
    pub parent_id: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnRequest {
    pub machine_id: String,
    pub working_dir: String,
    pub prompt: Option<String>,
    pub prompt_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnResponse {
    pub command_id: Uuid,
    pub status: String,
}

/// One row of the per-user archive index (all archives reachable via the
/// caller's `user_id`, across all of that user's machines).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveIndexEntry {
    pub machine_id: Uuid,
    pub project_dir: String,
    pub session_id: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub line_count: Option<i32>,
    pub uploaded_at: chrono::DateTime<chrono::Utc>,
}

/// One row of the skill registry (one per skill name — last-write-wins).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIndexEntry {
    pub name: String,
    pub version: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub uploaded_by_machine: Option<Uuid>,
    pub uploaded_at: chrono::DateTime<chrono::Utc>,
    pub content_type: String,
}
