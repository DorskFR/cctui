//! Wire types shared with `cctui-server`.
//! Port of `channel/src/types.ts`.

use serde::{Deserialize, Serialize};

/// Event sent to `POST /api/v1/events/{session_id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamerEvent {
    pub session_id: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    pub ts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreToolUsePayload {
    pub session_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PendingMessage {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelRegisterResponse {
    pub channel_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SessionPollResponse {
    Waiting,
    Matched {
        session_id: String,
        #[serde(default)]
        transcript_path: Option<String>,
        #[serde(default)]
        model: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct PermissionRequest {
    pub request_id: String,
    pub tool_name: String,
    pub description: String,
    pub input_preview: String,
}

#[derive(Debug, Clone)]
pub struct SessionState {
    pub session_id: String,
    pub transcript_path: Option<String>,
    pub cwd: String,
    pub machine_id: String,
    #[allow(dead_code)]
    pub model: String,
}
