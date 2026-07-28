//! `GET /event` SSE decoding and the subset of the opencode bus the adapter acts on.

use serde::Deserialize;

use super::client::{MessageInfo, Part};

#[derive(Debug, Clone, Deserialize)]
pub struct SessionRef {
    #[serde(rename = "sessionID")]
    pub session_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageUpdated {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub info: MessageInfo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PartUpdated {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub part: Part,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionErrorProps {
    #[serde(default, rename = "sessionID")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub error: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionStatusProps {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(default)]
    pub status: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PermissionAsked {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(default)]
    pub permission: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PermissionReplied {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum OcEvent {
    #[serde(rename = "message.updated")]
    MessageUpdated { properties: MessageUpdated },
    #[serde(rename = "message.part.updated")]
    PartUpdated { properties: PartUpdated },
    #[serde(rename = "session.idle")]
    SessionIdle { properties: SessionRef },
    #[serde(rename = "session.status")]
    SessionStatus { properties: SessionStatusProps },
    #[serde(rename = "session.error")]
    SessionError { properties: SessionErrorProps },
    #[serde(rename = "session.deleted")]
    SessionDeleted { properties: SessionRef },
    #[serde(rename = "permission.asked")]
    PermissionAsked { properties: PermissionAsked },
    #[serde(rename = "permission.replied")]
    PermissionReplied { properties: PermissionReplied },
    #[serde(other)]
    Other,
}

impl OcEvent {
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::MessageUpdated { properties } => Some(&properties.session_id),
            Self::PartUpdated { properties } => Some(&properties.session_id),
            Self::SessionIdle { properties } | Self::SessionDeleted { properties } => {
                Some(&properties.session_id)
            }
            Self::SessionStatus { properties } => Some(&properties.session_id),
            Self::SessionError { properties } => properties.session_id.as_deref(),
            Self::PermissionAsked { properties } => Some(&properties.session_id),
            Self::PermissionReplied { properties } => Some(&properties.session_id),
            Self::Other => None,
        }
    }
}

/// Status kinds the adapter maps onto a session tempo. `retry` carries the
/// provider-side failure message opencode is backing off from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusKind {
    Busy,
    Retry { attempt: i64, message: String },
    Other(String),
}

#[must_use]
pub fn status_kind(status: &serde_json::Value) -> Option<StatusKind> {
    let ty = status.get("type").and_then(serde_json::Value::as_str)?;
    Some(match ty {
        "busy" => StatusKind::Busy,
        "retry" => StatusKind::Retry {
            attempt: status.get("attempt").and_then(serde_json::Value::as_i64).unwrap_or(0),
            message: status
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
        other => StatusKind::Other(other.to_owned()),
    })
}

/// Incremental `text/event-stream` framer: feeds raw chunks in, yields the
/// `data:` payload of each complete frame.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buf: String,
}

impl SseDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        while let Some(idx) = self.buf.find("\n\n") {
            let frame: String = self.buf.drain(..idx + 2).collect();
            let mut data = String::new();
            for line in frame.lines() {
                if let Some(rest) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(rest.trim_start());
                }
            }
            if !data.is_empty() {
                out.push(data);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_splits_frames_and_ignores_comments() {
        let mut d = SseDecoder::new();
        assert!(d.push("data: {\"a\":1}").is_empty());
        let got = d.push("}\n\n: keepalive\n\ndata: {\"b\":2}\n\n");
        assert_eq!(got, vec!["{\"a\":1}}".to_owned(), "{\"b\":2}".to_owned()]);
    }

    #[test]
    fn decoder_joins_multiline_data() {
        let mut d = SseDecoder::new();
        assert_eq!(d.push("data: one\ndata: two\n\n"), vec!["one\ntwo".to_owned()]);
    }

    #[test]
    fn part_updated_event_parses() {
        let raw = serde_json::json!({
            "id": "evt_1",
            "type": "message.part.updated",
            "properties": {
                "sessionID": "ses_1",
                "time": 1_785_216_933_284u64,
                "part": {
                    "type": "text", "id": "prt_1", "sessionID": "ses_1",
                    "messageID": "msg_1", "text": "hi"
                }
            }
        });
        let e: OcEvent = serde_json::from_value(raw).unwrap();
        assert_eq!(e.session_id(), Some("ses_1"));
        assert!(matches!(e, OcEvent::PartUpdated { .. }));
    }

    #[test]
    fn message_updated_event_parses() {
        let raw = serde_json::json!({
            "id": "evt_2",
            "type": "message.updated",
            "properties": {
                "sessionID": "ses_1",
                "info": {
                    "id": "msg_1", "sessionID": "ses_1", "role": "assistant",
                    "cost": 0.5,
                    "tokens": { "input": 5, "output": 6, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
                    "time": { "created": 1, "completed": 2 }
                }
            }
        });
        match serde_json::from_value::<OcEvent>(raw).unwrap() {
            OcEvent::MessageUpdated { properties } => {
                assert_eq!(properties.info.tokens.unwrap().output, 6);
            }
            other => panic!("expected message.updated, got {other:?}"),
        }
    }

    #[test]
    fn idle_and_permission_events_parse() {
        let idle: OcEvent = serde_json::from_value(serde_json::json!({
            "id": "evt_3", "type": "session.idle", "properties": { "sessionID": "ses_1" }
        }))
        .unwrap();
        assert!(matches!(idle, OcEvent::SessionIdle { .. }));

        let perm: OcEvent = serde_json::from_value(serde_json::json!({
            "id": "evt_4", "type": "permission.asked",
            "properties": { "id": "per_1", "sessionID": "ses_1", "permission": "bash",
                            "patterns": ["rm *"], "metadata": { "command": "rm -rf /" }, "always": [] }
        }))
        .unwrap();
        match perm {
            OcEvent::PermissionAsked { properties } => {
                assert_eq!(properties.id, "per_1");
                assert_eq!(properties.permission, "bash");
            }
            other => panic!("expected permission.asked, got {other:?}"),
        }
    }

    #[test]
    fn unknown_event_types_degrade() {
        let e: OcEvent = serde_json::from_value(serde_json::json!({
            "id": "evt_5", "type": "plugin.added", "properties": { "whatever": true }
        }))
        .unwrap();
        assert!(matches!(e, OcEvent::Other));
        assert_eq!(e.session_id(), None);
    }

    #[test]
    fn status_kinds_map() {
        assert_eq!(status_kind(&serde_json::json!({ "type": "busy" })), Some(StatusKind::Busy));
        assert_eq!(
            status_kind(&serde_json::json!({ "type": "retry", "attempt": 2, "message": "boom" })),
            Some(StatusKind::Retry { attempt: 2, message: "boom".to_owned() })
        );
        assert_eq!(status_kind(&serde_json::json!({})), None);
    }
}
