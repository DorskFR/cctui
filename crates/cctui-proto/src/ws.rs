use serde::{Deserialize, Serialize};
use uuid::Uuid;

// --- Agent → Server (stream events) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Text { content: String, ts: i64 },
    ToolCall { tool: String, input: serde_json::Value, ts: i64 },
    ToolResult { tool: String, output_summary: String, ts: i64 },
    Heartbeat { tokens_in: u64, tokens_out: u64, cost_usd: f64, ts: i64 },
    Reply { content: String, ts: i64 },
}

// --- TUI → Server ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TuiCommand {
    Subscribe { session_id: Uuid },
    Unsubscribe { session_id: Uuid },
    Message { session_id: Uuid, content: String },
}

// --- Server → TUI ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Stream { session_id: Uuid, data: AgentEvent },
    Status { session_id: Uuid, status: crate::models::SessionStatus },
    SessionRegistered { session: crate::models::Session },
    SessionDeregistered { session_id: Uuid },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_event_tagged_serialization() {
        let event = AgentEvent::Text { content: "hello".into(), ts: 1_234_567_890 };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"text""#));
        assert!(json.contains(r#""content":"hello""#));
    }

    #[test]
    fn tui_command_tagged_serialization() {
        let cmd = TuiCommand::Subscribe { session_id: Uuid::nil() };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""type":"subscribe""#));
    }
}
