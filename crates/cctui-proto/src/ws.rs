use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::adapter::{AdapterCommand, AdapterEvent};
use crate::api::DaemonAdapterConfig;

// --- Daemon → Server ---

/// Frames sent by a daemon to the server over `/api/v1/daemon/ws`.
///
/// `Event` is inherently the largest variant (it carries an [`AdapterEvent`]
/// with JSON payloads / many optional fields); boxing it would ripple
/// through every construct/match site for no real benefit on this
/// non-hot-path wire enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
#[allow(clippy::large_enum_variant)]
pub enum DaemonFrameUp {
    /// An adapter produced an event. The server maps `(machine_id, adapter_id,
    /// local_id)` to a stable `server_session_id`, persisting a new row on
    /// `SessionStarted` if one does not exist yet.
    Event { adapter_id: String, event: AdapterEvent },
    /// Optional explicit registration hint when the adapter cannot supply a
    /// full `SessionStarted` yet (e.g. resumed session). Mostly redundant.
    SessionRegistered { adapter_id: String, local_id: String },
    /// Liveness ping. Includes coarse per-daemon stats (counts of adapters
    /// running, queued events) for the future supervisor view.
    Heartbeat { sent_at: chrono::DateTime<chrono::Utc> },
}

/// Frames sent by the server to a daemon over `/api/v1/daemon/ws`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DaemonFrameDown {
    /// Initial declarative state — sent on connect and again whenever the
    /// server mutates `adapters_enabled` for this machine.
    Reconcile { adapters: Vec<DaemonAdapterConfig> },
    /// A command for a specific adapter (and ultimately a specific session).
    Command { adapter_id: String, command: AdapterCommand },
    /// Acknowledge that an event with the given monotonic `seq` has been
    /// durably stored. Lets the daemon trim its on-disk spool.
    Ack { seq: u64 },
}

// --- Agent → Server (stream events) ---

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Free-form text. `meta` marks a message that was injected *to* the agent
    /// rather than typed by the human (harness wake-ups, `<task-notification>`,
    /// `<system-reminder>`, slash-command expansions). Set authoritatively at
    /// the adapter layer (Claude's `isMeta` + known harness tags) so clients
    /// can render it distinctly without re-sniffing strings. `#[serde(default)]`
    /// keeps older stored payloads (no field) decoding as non-meta.
    Text {
        content: String,
        #[serde(default)]
        meta: bool,
        ts: i64,
    },
    ToolCall {
        tool: String,
        input: serde_json::Value,
        ts: i64,
    },
    ToolResult {
        tool: String,
        output_summary: String,
        ts: i64,
    },
    Heartbeat {
        tokens_in: u64,
        tokens_out: u64,
        cost_usd: f64,
        ts: i64,
    },
    Reply {
        content: String,
        ts: i64,
    },
    /// A context reset boundary (`/clear` or `/compact`). The session id rotates
    /// in place under the same worker; rather than splitting into a second
    /// session (archive is worker-scoped, so one `claude rm` would wipe both),
    /// we keep one session and emit this marker so clients can render the cut
    /// distinctly (CCT-158).
    ContextReset {
        ts: i64,
    },
    /// A `/compact` boundary. Unlike `/clear`, `/compact` does NOT rotate the
    /// session id — it appends an `isCompactSummary` line to the same
    /// transcript — so it surfaces as its own event carrying the summary text,
    /// rendered as a distinct "context compacted" block rather than a user
    /// message (CCT-159).
    CompactSummary {
        content: String,
        ts: i64,
    },
    TurnEnd {
        ts: i64,
    },
}

// --- TUI → Server ---

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TuiCommand {
    Subscribe {
        session_id: String,
    },
    Unsubscribe {
        session_id: String,
    },
    /// A typed reply from a client. `client_msg_id` (when present) lets the
    /// server ack the send back to the originating socket via
    /// [`ServerEvent::MessageAck`], so the client can render a precise
    /// per-message delivery state (sending → delivered / failed) instead of
    /// optimistically assuming a frame that left the socket was delivered
    /// (CCT-212). `#[serde(default)]` keeps older clients (no field) working —
    /// they simply receive no ack.
    Message {
        session_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_msg_id: Option<String>,
    },
    PermissionResponse {
        session_id: String,
        request_id: String,
        behavior: String,
    },
}

// --- Server → TUI ---

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Stream {
        session_id: String,
        data: AgentEvent,
    },
    Status {
        session_id: String,
        status: crate::models::SessionStatus,
    },
    SessionRegistered {
        session: crate::models::Session,
    },
    SessionDeregistered {
        session_id: String,
    },
    PermissionRequest {
        session_id: String,
        request_id: String,
        tool_name: String,
        description: String,
        input_preview: String,
    },
    /// A previously-broadcast permission request has been resolved (by TUI
    /// or a web client). Clients should dismiss any inline prompt UI.
    PermissionResolved {
        session_id: String,
        request_id: String,
    },
    /// The agent is blocked on an `AskUserQuestion`; carries the question text
    /// so clients render a live prompt before the transcript flushes the full
    /// tool call (CCT-164). `questions` carries the raw `tool_input.questions`
    /// array so clients render the interactive option-card form live rather
    /// than just the flattened text (CCT-181).
    AskQuestion {
        session_id: String,
        question: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        questions: Option<serde_json::Value>,
    },
    /// A previously-broadcast `AskQuestion` is resolved; clients dismiss the
    /// live prompt (CCT-164).
    AskResolved {
        session_id: String,
    },
    /// Outcome of a client-initiated command (currently `POST /sessions/spawn`).
    /// `command_id` matches the value returned by the spawn route so the
    /// originating client can surface success/failure instead of silently
    /// polling (CCT-131).
    CommandResult {
        command_id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Outcome of a client-sent [`TuiCommand::Message`] carrying a
    /// `client_msg_id`. Sent only to the originating socket. `ok=false` means
    /// the server could not dispatch the reply to the session's daemon (e.g.
    /// the daemon was momentarily offline — `NoDaemon`/`Closed`), so the client
    /// should mark the message failed and offer a retry rather than leaving it
    /// stuck "sending…" until it silently vanishes on the next resubscribe
    /// (CCT-212).
    MessageAck {
        session_id: String,
        client_msg_id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// A machine has just reported a fresh expected-files manifest (CCT-68).
    ArchiveManifest {
        machine_id: uuid::Uuid,
        count: i64,
    },
    /// A single archive file has just finished uploading (CCT-68).
    ArchiveUploaded {
        machine_id: uuid::Uuid,
        project_dir: String,
        session_id: String,
        size_bytes: i64,
        sha256: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_event_tagged_serialization() {
        let event = AgentEvent::Text { content: "hello".into(), meta: false, ts: 1_234_567_890 };
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
            AgentEvent::Text { content: "hello".into(), meta: false, ts: 1 },
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
            data: AgentEvent::Text { content: "hi".into(), meta: false, ts: 1 },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"stream""#));
    }

    #[test]
    fn daemon_frame_up_event_serializes_tagged() {
        let f = DaemonFrameUp::Event {
            adapter_id: "claude-code".into(),
            event: AdapterEvent::SessionStarted {
                local_id: "abc".into(),
                meta: crate::adapter::SessionMeta::default(),
            },
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"event""#));
        assert!(json.contains(r#""adapter_id":"claude-code""#));
        let _back: DaemonFrameUp = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn daemon_frame_down_reconcile_roundtrips() {
        let f = DaemonFrameDown::Reconcile { adapters: vec![] };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"reconcile""#));
        let _back: DaemonFrameDown = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn daemon_frame_down_command_roundtrips() {
        let f = DaemonFrameDown::Command {
            adapter_id: "claude-code".into(),
            command: AdapterCommand::Kill { local_id: "abc".into(), signal: None },
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"command""#));
        let _back: DaemonFrameDown = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn tui_command_message_serialization() {
        let cmd = TuiCommand::Message {
            session_id: "test-session".into(),
            content: "hello".into(),
            client_msg_id: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""type":"message""#));
        assert!(json.contains(r#""content":"hello""#));
        let deserialized: TuiCommand = serde_json::from_str(&json).unwrap();
        match deserialized {
            TuiCommand::Message { content, .. } => assert_eq!(content, "hello"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn tui_command_message_omits_client_msg_id_when_none() {
        // Old clients send no `client_msg_id`; the field is skipped on the wire
        // so the payload stays byte-compatible with pre-CCT-212 readers.
        let cmd = TuiCommand::Message {
            session_id: "s".into(),
            content: "hi".into(),
            client_msg_id: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(!json.contains("client_msg_id"), "None must be skipped: {json}");
    }

    #[test]
    fn tui_command_message_accepts_legacy_payload_without_client_msg_id() {
        // A frame from an older client (no field) must still decode (serde default).
        let legacy = r#"{"type":"message","session_id":"s","content":"hi"}"#;
        let cmd: TuiCommand = serde_json::from_str(legacy).unwrap();
        match cmd {
            TuiCommand::Message { client_msg_id, .. } => assert_eq!(client_msg_id, None),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn tui_command_message_carries_client_msg_id_when_set() {
        let cmd = TuiCommand::Message {
            session_id: "s".into(),
            content: "hi".into(),
            client_msg_id: Some("abc-123".into()),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""client_msg_id":"abc-123""#));
        let back: TuiCommand = serde_json::from_str(&json).unwrap();
        match back {
            TuiCommand::Message { client_msg_id, .. } => {
                assert_eq!(client_msg_id.as_deref(), Some("abc-123"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn server_event_message_ack_roundtrips() {
        let ev = ServerEvent::MessageAck {
            session_id: "s".into(),
            client_msg_id: "abc-123".into(),
            ok: false,
            error: Some("no daemon connected for machine …".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains(r#""type":"message_ack""#));
        assert!(json.contains(r#""ok":false"#));
        assert!(json.contains(r#""client_msg_id":"abc-123""#));
        let _back: ServerEvent = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn server_event_message_ack_omits_error_when_ok() {
        let ev = ServerEvent::MessageAck {
            session_id: "s".into(),
            client_msg_id: "abc-123".into(),
            ok: true,
            error: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(!json.contains("error"), "None error must be skipped: {json}");
    }
}
