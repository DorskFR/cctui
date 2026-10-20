//! `GET /api/event` payloads: event-sourced deltas in a `{ id, type, data }`
//! envelope, where v1 sent message and part snapshots.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::adapters::opencode::client::Tokens;

#[derive(Debug, Clone, Deserialize)]
pub struct SessionRef {
    #[serde(rename = "sessionID")]
    pub session_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Failure {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(default)]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Interrupted {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Step {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(default, rename = "assistantMessageID")]
    pub message_id: String,
    #[serde(default)]
    pub tokens: Option<Tokens>,
    #[serde(default)]
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Text {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(default, rename = "assistantMessageID")]
    pub message_id: String,
    #[serde(default)]
    pub ordinal: u64,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TextDelta {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(default, rename = "assistantMessageID")]
    pub message_id: String,
    #[serde(default)]
    pub ordinal: u64,
    #[serde(default)]
    pub delta: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tool {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub input: Option<Value>,
    #[serde(default)]
    pub content: Vec<Value>,
    #[serde(default)]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Retry {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(default)]
    pub attempt: i64,
    #[serde(default)]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PermissionAsked {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub save: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PermissionReplied {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "requestID")]
    pub request_id: String,
    #[serde(default)]
    pub reply: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Inbox {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "inboxID")]
    pub inbox_id: String,
    #[serde(default)]
    pub item: Option<InboxItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InboxItem {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub payload: Value,
}

/// The subset of the bus the adapter acts on; everything else is `Other`.
#[derive(Debug, Clone)]
pub enum Event {
    ExecutionStarted(SessionRef),
    ExecutionSucceeded(SessionRef),
    ExecutionInterrupted(Interrupted),
    ExecutionFailed(Failure),
    StepStarted(Step),
    StepEnded(Step),
    StepFailed(Failure),
    TextDelta(TextDelta),
    TextEnded(Text),
    ReasoningEnded(Text),
    ToolInputStarted(Tool),
    ToolCalled(Tool),
    ToolSucceeded(Tool),
    ToolFailed(Tool),
    RetryScheduled(Retry),
    InboxEnqueued(Inbox),
    InboxDelivered(Inbox),
    SessionDeleted(SessionRef),
    PermissionAsked(PermissionAsked),
    PermissionReplied(PermissionReplied),
    Other,
}

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    data: Value,
}

fn de<T: DeserializeOwned>(data: Value) -> serde_json::Result<T> {
    serde_json::from_value(data)
}

impl Event {
    /// The tool error variant was never observed on the wire, so both names it
    /// could plausibly carry are accepted.
    pub fn decode(raw: &str) -> serde_json::Result<Self> {
        let Envelope { kind, data } = serde_json::from_str(raw)?;
        Ok(match kind.as_str() {
            "session.execution.started" => Self::ExecutionStarted(de(data)?),
            "session.execution.succeeded" => Self::ExecutionSucceeded(de(data)?),
            "session.execution.interrupted" => Self::ExecutionInterrupted(de(data)?),
            "session.execution.failed" => Self::ExecutionFailed(de(data)?),
            "session.step.started" => Self::StepStarted(de(data)?),
            "session.step.ended" => Self::StepEnded(de(data)?),
            "session.step.failed" => Self::StepFailed(de(data)?),
            "session.text.delta" => Self::TextDelta(de(data)?),
            "session.text.ended" => Self::TextEnded(de(data)?),
            "session.reasoning.ended" => Self::ReasoningEnded(de(data)?),
            "session.tool.input.started" => Self::ToolInputStarted(de(data)?),
            "session.tool.called" => Self::ToolCalled(de(data)?),
            "session.tool.success" => Self::ToolSucceeded(de(data)?),
            "session.tool.failed" | "session.tool.error" => Self::ToolFailed(de(data)?),
            "session.retry.scheduled" => Self::RetryScheduled(de(data)?),
            "session.inbox.enqueued" => Self::InboxEnqueued(de(data)?),
            "session.inbox.delivered" => Self::InboxDelivered(de(data)?),
            "session.deleted" => Self::SessionDeleted(de(data)?),
            "permission.asked" => Self::PermissionAsked(de(data)?),
            "permission.replied" => Self::PermissionReplied(de(data)?),
            _ => Self::Other,
        })
    }

    #[must_use]
    pub const fn session_id(&self) -> Option<&str> {
        let id = match self {
            Self::ExecutionStarted(e) | Self::ExecutionSucceeded(e) | Self::SessionDeleted(e) => {
                &e.session_id
            }
            Self::ExecutionInterrupted(e) => &e.session_id,
            Self::ExecutionFailed(e) | Self::StepFailed(e) => &e.session_id,
            Self::StepStarted(e) | Self::StepEnded(e) => &e.session_id,
            Self::TextDelta(e) => &e.session_id,
            Self::TextEnded(e) | Self::ReasoningEnded(e) => &e.session_id,
            Self::ToolInputStarted(e)
            | Self::ToolCalled(e)
            | Self::ToolSucceeded(e)
            | Self::ToolFailed(e) => &e.session_id,
            Self::RetryScheduled(e) => &e.session_id,
            Self::InboxEnqueued(e) | Self::InboxDelivered(e) => &e.session_id,
            Self::PermissionAsked(e) => &e.session_id,
            Self::PermissionReplied(e) => &e.session_id,
            Self::Other => return None,
        };
        Some(id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::opencode::events::SseDecoder;

    const FIXTURE: &str = include_str!("../fixtures/v2_events.sse");

    fn frames() -> Vec<String> {
        SseDecoder::new().push(FIXTURE)
    }

    #[test]
    fn every_recorded_frame_decodes_and_heartbeats_are_dropped() {
        assert_eq!(FIXTURE.matches("\n: heartbeat\n").count(), 2);
        let frames = frames();
        assert_eq!(frames.len(), 42, "42 data frames; the two heartbeat comments carry no data");
        assert!(frames.iter().all(|f| !f.contains("heartbeat")));
        for raw in &frames {
            assert!(Event::decode(raw).is_ok(), "undecodable frame: {raw}");
        }
    }

    #[test]
    fn noise_degrades_to_other_with_no_session() {
        for raw in frames() {
            let kind =
                serde_json::from_str::<Value>(&raw).unwrap()["type"].as_str().unwrap().to_owned();
            let ignored = kind.starts_with("shell.")
                || kind.starts_with("server.")
                || kind == "session.created"
                || kind == "session.forked"
                || kind == "session.usage.updated"
                || kind == "session.step.streamed"
                || kind.strip_suffix("delta").is_some_and(|head| head.ends_with('.'));
            if ignored {
                let evt = Event::decode(&raw).unwrap();
                assert!(matches!(evt, Event::Other), "{kind} must be ignored");
                assert_eq!(evt.session_id(), None);
            }
        }
    }

    #[test]
    fn a_step_end_carries_its_usage() {
        let raw = frames().into_iter().find(|f| f.contains("\"session.step.ended\"")).unwrap();
        let Event::StepEnded(step) = Event::decode(&raw).unwrap() else { panic!("{raw}") };
        assert_eq!(step.message_id, "msg_1327939a1001VPs7Lc4UOaAGM6");
        let tokens = step.tokens.unwrap();
        assert_eq!((tokens.input, tokens.output), (90, 10));
    }

    #[test]
    fn a_permission_ask_names_its_action_and_resources() {
        let raw = frames().into_iter().find(|f| f.contains("\"permission.asked\"")).unwrap();
        let evt = Event::decode(&raw).unwrap();
        assert_eq!(evt.session_id(), Some("ses_ecd86c6adffeSeDX15STTLCF8s"));
        let Event::PermissionAsked(ask) = evt else { panic!("{raw}") };
        assert_eq!(ask.id, "per_1327939e900126M5lPXw3N2VfG");
        assert_eq!(ask.action, "shell");
        assert_eq!(ask.resources, vec!["ls -la".to_owned()]);
    }

    #[test]
    fn a_tool_error_is_read_under_either_name() {
        for kind in ["session.tool.failed", "session.tool.error"] {
            let raw = serde_json::json!({
                "type": kind,
                "data": { "sessionID": "ses_1", "id": "call_1",
                          "error": { "type": "tool.denied", "message": "no" } }
            })
            .to_string();
            assert!(matches!(Event::decode(&raw).unwrap(), Event::ToolFailed(_)), "{kind}");
        }
    }

    #[test]
    fn a_known_event_with_a_broken_payload_is_an_error() {
        let raw = r#"{"type":"session.execution.succeeded","data":{}}"#;
        assert!(Event::decode(raw).is_err());
    }
}
