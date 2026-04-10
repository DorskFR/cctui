use serde::{Deserialize, Serialize};

// --- Agent → Server (stream events) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Text { content: String, ts: i64 },
    ToolCall { tool: String, input: serde_json::Value, ts: i64 },
    ToolResult { tool: String, output_summary: String, ts: i64 },
    Heartbeat { tokens_in: u64, tokens_out: u64, cost_usd: f64, ts: i64 },
    Reply { content: String, ts: i64 },
    TurnEnd { ts: i64 },
}

// --- TUI → Server ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TuiCommand {
    Subscribe { session_id: String },
    Unsubscribe { session_id: String },
    Message { session_id: String, content: String },
}

// --- Server → TUI ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Stream { session_id: String, data: AgentEvent },
    Status { session_id: String, status: crate::models::SessionStatus },
    SessionRegistered { session: crate::models::Session },
    SessionDeregistered { session_id: String },
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
        let cmd = TuiCommand::Subscribe { session_id: "test-session".into() };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""type":"subscribe""#));
    }

    #[test]
    fn agent_event_reply_serialization() {
        let event = AgentEvent::Reply { content: "acknowledged".into(), ts: 100 };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"reply""#));
        assert!(json.contains(r#""content":"acknowledged""#));
    }

    #[test]
    fn agent_event_tool_call_serialization() {
        let event = AgentEvent::ToolCall {
            tool: "Bash".into(),
            input: serde_json::json!({"command": "ls"}),
            ts: 42,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"tool_call""#));
        assert!(json.contains(r#""tool":"Bash""#));
    }

    #[test]
    fn agent_event_tool_result_serialization() {
        let event = AgentEvent::ToolResult {
            tool: "Bash".into(),
            output_summary: "file.txt".into(),
            ts: 42,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"tool_result""#));
        assert!(json.contains(r#""output_summary":"file.txt""#));
    }

    #[test]
    fn agent_event_heartbeat_serialization() {
        let event =
            AgentEvent::Heartbeat { tokens_in: 100, tokens_out: 50, cost_usd: 0.01, ts: 42 };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"heartbeat""#));
        assert!(json.contains(r#""tokens_in":100"#));
    }

    #[test]
    fn agent_event_roundtrip_all_variants() {
        let variants = vec![
            AgentEvent::Text { content: "hello".into(), ts: 1 },
            AgentEvent::ToolCall { tool: "Read".into(), input: serde_json::json!({}), ts: 2 },
            AgentEvent::ToolResult { tool: "Read".into(), output_summary: "ok".into(), ts: 3 },
            AgentEvent::Heartbeat { tokens_in: 10, tokens_out: 5, cost_usd: 0.001, ts: 4 },
            AgentEvent::Reply { content: "done".into(), ts: 5 },
            AgentEvent::TurnEnd { ts: 6 },
        ];
        for event in variants {
            let json = serde_json::to_string(&event).unwrap();
            let deserialized: AgentEvent = serde_json::from_str(&json).unwrap();
            let re_json = serde_json::to_string(&deserialized).unwrap();
            assert_eq!(json, re_json, "roundtrip failed for {json}");
        }
    }

    #[test]
    fn server_event_serialization() {
        let event = ServerEvent::Stream {
            session_id: "test-session".into(),
            data: AgentEvent::Text { content: "hi".into(), ts: 1 },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"stream""#));
    }

    #[test]
    fn tui_command_message_serialization() {
        let cmd =
            TuiCommand::Message { session_id: "test-session".into(), content: "hello".into() };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""type":"message""#));
        assert!(json.contains(r#""content":"hello""#));
        let deserialized: TuiCommand = serde_json::from_str(&json).unwrap();
        match deserialized {
            TuiCommand::Message { content, .. } => assert_eq!(content, "hello"),
            _ => panic!("wrong variant"),
        }
    }
}
