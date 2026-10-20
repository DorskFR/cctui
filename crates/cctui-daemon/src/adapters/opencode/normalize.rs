//! `OpenCode` parts/messages → [`AdapterEvent`] payloads.
//!
//! Payloads are emitted in BOTH dialects at once: the canonical client shape
//! (`type`/`content`) that the server's read path passes through for adapters
//! without a dedicated mapper, and the claude-daemon shape (`role`/`text`) that
//! the server's live broadcast (`to_agent_event`) understands. Emitting only
//! one of the two renders the session as "No events yet" on the other path.

use std::collections::{HashMap, HashSet};

use cctui_proto::adapter::AdapterEvent;
use serde_json::{Value, json};

use super::backend::Signal;
use super::client::{MessageInfo, Part, ToolState};
use super::events::{OcEvent, StatusKind, status_kind};

const TOOL_OUTPUT_CAP: usize = 4000;

/// Which `AdapterEvent` a payload belongs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Message,
    ToolUse,
}

/// Whether a part is final enough to emit.
///
/// Text/reasoning parts stream in place, so only a part with an end time (or a
/// user part, which never gets one) is emitted — otherwise every chunk of the
/// stream becomes its own row.
#[must_use]
pub fn is_final(part: &Part, role: &str) -> bool {
    match part {
        Part::Text { time, synthetic, .. } => {
            synthetic != &Some(true) && (role == "user" || time.and_then(|t| t.end).is_some())
        }
        Part::Reasoning { time, .. } => time.and_then(|t| t.end).is_some(),
        Part::Tool { state, .. } => {
            matches!(state, ToolState::Completed { .. } | ToolState::Error { .. })
        }
        Part::Other => false,
    }
}

/// Map one final part to its payloads. A completed tool yields two rows (the
/// call and its result) so the conversation shows what the tool returned.
#[must_use]
pub fn part_payloads(part: &Part, role: &str) -> Vec<(Kind, Value)> {
    match part {
        Part::Text { id, text, message_id, .. } => {
            if text.trim().is_empty() {
                return Vec::new();
            }
            vec![(Kind::Message, text_payload(role, text, message_id.as_deref(), id))]
        }
        Part::Reasoning { text, message_id, .. } => {
            if text.trim().is_empty() {
                return Vec::new();
            }
            vec![(Kind::Message, reasoning_payload(text, message_id.as_deref()))]
        }
        Part::Tool { id, tool, call_id, state, .. } => {
            let call_id = call_id.as_deref().unwrap_or(id);
            match state {
                ToolState::Completed { input, output, .. } => vec![
                    (Kind::ToolUse, tool_call_payload(tool, input, call_id)),
                    (Kind::ToolUse, tool_result_payload(output, false, call_id)),
                ],
                ToolState::Error { input, error } => vec![
                    (Kind::ToolUse, tool_call_payload(tool, input, call_id)),
                    (Kind::ToolUse, tool_result_payload(error, true, call_id)),
                ],
                ToolState::Pending | ToolState::Running { .. } => Vec::new(),
            }
        }
        Part::Other => Vec::new(),
    }
}

/// A user turn carries no id of its own, and the server dedupes on
/// `digest(payload)` — without the part id the same prose sent twice in one
/// session is silently dropped. The id comes from the part so a replay of the
/// same part still hashes identically.
pub(super) fn text_payload(
    role: &str,
    text: &str,
    message_id: Option<&str>,
    part_id: &str,
) -> Value {
    if role == "user" {
        return json!({
            "type": "text",
            "content": format!("▷ User: {text}"),
            "role": "user",
            "text": text,
            "meta": false,
            "line_id": part_id,
        });
    }
    json!({
        "type": "text",
        "content": text,
        "role": "assistant",
        "text": text,
        "message_id": message_id,
    })
}

pub(super) fn reasoning_payload(text: &str, message_id: Option<&str>) -> Value {
    json!({
        "type": "text",
        "content": text,
        "role": "assistant_thinking",
        "text": text,
        "message_id": message_id,
    })
}

pub(super) fn tool_call_payload(tool: &str, input: &Value, call_id: &str) -> Value {
    json!({ "type": "tool_call", "tool": tool, "input": input, "id": call_id, "tool_use_id": call_id })
}

pub(super) fn tool_result_payload(output: &str, error: bool, call_id: &str) -> Value {
    let capped: String = output.chars().take(TOOL_OUTPUT_CAP).collect();
    json!({
        "type": "tool_result",
        "output_summary": capped,
        "kind": "tool_result",
        "content": capped,
        "is_error": error,
        "error": error,
        "tool_use_id": call_id,
    })
}

/// Per-message token usage. Emitted once the message completes and only when
/// opencode actually reported counts.
#[must_use]
pub fn token_usage(local_id: &str, info: &MessageInfo) -> Option<AdapterEvent> {
    if info.role != "assistant" || info.time.completed.is_none() {
        return None;
    }
    let tokens = info.tokens?;
    if tokens.is_empty() {
        return None;
    }
    Some(AdapterEvent::TokenUsage {
        local_id: local_id.to_owned(),
        message_id: info.id.clone(),
        input_tokens: tokens.input,
        output_tokens: tokens.output,
        cache_read_tokens: tokens.cache.read,
        cache_creation_tokens: tokens.cache.write,
    })
}

/// Render an opencode error object (`{ name, data: { message } }`) as a line.
#[must_use]
pub fn error_text(error: &Value) -> String {
    let name = error.get("name").and_then(Value::as_str).unwrap_or("error");
    let message = error
        .get("data")
        .and_then(|d| d.get("message"))
        .and_then(Value::as_str)
        .or_else(|| error.get("message").and_then(Value::as_str))
        .unwrap_or_default();
    if message.is_empty() { name.to_owned() } else { format!("{name}: {message}") }
}

/// v1 re-sends whole snapshots: a part needs its message's role, and what was
/// already emitted must not be emitted again.
#[derive(Debug, Default)]
pub struct Tracker {
    roles: HashMap<String, String>,
    emitted_parts: HashSet<String>,
    emitted_usage: HashSet<String>,
    /// Bytes of each streaming text part already forwarded as deltas.
    streamed: HashMap<String, usize>,
}

impl Tracker {
    pub fn signals(&mut self, session_id: &str, evt: OcEvent) -> Vec<Signal> {
        match evt {
            OcEvent::MessageUpdated { properties } => self.message(session_id, &properties.info),
            OcEvent::PartUpdated { properties } => self.part(session_id, &properties.part),
            OcEvent::SessionIdle { .. } => vec![Signal::TurnEnded],
            OcEvent::SessionStatus { properties } => {
                status_kind(&properties.status).map(status_signal).into_iter().collect()
            }
            OcEvent::SessionError { properties } => {
                let text = properties
                    .error
                    .as_ref()
                    .map_or_else(|| "session error".to_owned(), error_text);
                vec![Signal::Failed(text)]
            }
            OcEvent::SessionDeleted { .. } => vec![Signal::Deleted],
            OcEvent::PermissionAsked { properties } => vec![Signal::PermissionAsked {
                request_id: properties.id,
                tool: properties.permission,
                input: properties.metadata,
            }],
            OcEvent::PermissionReplied { properties } => {
                vec![Signal::PermissionReplied { request_id: properties.id }]
            }
            OcEvent::Other => Vec::new(),
        }
    }

    fn message(&mut self, session_id: &str, info: &MessageInfo) -> Vec<Signal> {
        let mut out = Vec::new();
        if info.role == "assistant" {
            out.push(Signal::AssistantActive);
        }
        self.roles.insert(info.id.clone(), info.role.clone());
        if let Some(usage) = token_usage(session_id, info)
            && self.emitted_usage.insert(info.id.clone())
        {
            out.push(Signal::Usage(Box::new(usage)));
        }
        if let Some(error) = info.error.as_ref() {
            out.push(Signal::Failed(error_text(error)));
        }
        out
    }

    fn part(&mut self, session_id: &str, part: &Part) -> Vec<Signal> {
        let role = part
            .message_id()
            .and_then(|id| self.roles.get(id))
            .map_or("assistant", String::as_str)
            .to_owned();
        if !is_final(part, &role) {
            return self.stream_delta(session_id, part, &role).into_iter().collect();
        }
        self.streamed.remove(part.id());
        if !self.emitted_parts.insert(part.id().to_owned()) {
            return Vec::new();
        }
        vec![Signal::Rows(part_payloads(part, &role))]
    }

    /// The text a streaming assistant part grew by since its last snapshot,
    /// for a session with a text watcher. A snapshot that does not extend the
    /// last one restarts the count without emitting.
    fn stream_delta(&mut self, session_id: &str, part: &Part, role: &str) -> Option<Signal> {
        let Part::Text { id, text, synthetic, .. } = part else { return None };
        if role != "assistant" || *synthetic == Some(true) {
            return None;
        }
        if !crate::text_deltas::wanted(session_id) {
            self.streamed.remove(id);
            return None;
        }
        let seen = self.streamed.entry(id.clone()).or_default();
        let delta = text.get(*seen..).filter(|d| !d.is_empty());
        *seen = text.len();
        delta.map(|delta| Signal::TextDelta { item_id: id.clone(), delta: delta.to_owned() })
    }
}

fn status_signal(kind: StatusKind) -> Signal {
    let (tempo, detail) = match kind {
        StatusKind::Busy => ("working".to_owned(), None),
        StatusKind::Retry { attempt, message } => {
            ("working".to_owned(), Some(format!("provider retry {attempt}: {message}")))
        }
        StatusKind::Other(other) => (other, None),
    };
    Signal::Tempo { tempo, detail }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::opencode::client::{MessageTime, PartTime, TokenCache, Tokens};

    fn text_part(text: &str, end: Option<i64>) -> Part {
        Part::Text {
            id: "prt_1".to_owned(),
            message_id: Some("msg_1".to_owned()),
            text: text.to_owned(),
            time: Some(PartTime { start: 1, end }),
            synthetic: None,
        }
    }

    fn part_updated(session_id: &str, part: Part) -> OcEvent {
        OcEvent::PartUpdated {
            properties: super::super::events::PartUpdated {
                session_id: session_id.to_owned(),
                part,
            },
        }
    }

    #[test]
    fn streaming_snapshots_become_deltas_only_for_a_text_watcher() {
        let mut tracker = Tracker::default();
        let unwatched =
            tracker.signals("ses_v1_silent", part_updated("ses_v1_silent", text_part("Hel", None)));
        assert!(unwatched.is_empty(), "no watcher, no delta: {unwatched:?}");

        crate::text_deltas::set("ses_v1_voice", true);
        let mut got = Vec::new();
        for text in ["Hel", "Hello", "Hello", "Hello there"] {
            got.extend(
                tracker
                    .signals("ses_v1_voice", part_updated("ses_v1_voice", text_part(text, None))),
            );
        }
        let done = tracker.signals(
            "ses_v1_voice",
            part_updated("ses_v1_voice", text_part("Hello there", Some(9))),
        );
        crate::text_deltas::set("ses_v1_voice", false);

        let deltas: Vec<(String, String)> = got
            .into_iter()
            .map(|s| match s {
                Signal::TextDelta { item_id, delta } => (item_id, delta),
                other => panic!("expected a delta, got {other:?}"),
            })
            .collect();
        assert_eq!(
            deltas,
            vec![
                ("prt_1".to_owned(), "Hel".to_owned()),
                ("prt_1".to_owned(), "lo".to_owned()),
                ("prt_1".to_owned(), " there".to_owned()),
            ]
        );
        assert!(
            matches!(done.as_slice(), [Signal::Rows(rows)] if rows.len() == 1),
            "the final part is still a row: {done:?}"
        );
        assert!(tracker.streamed.is_empty(), "a finished part is forgotten");
    }

    #[test]
    fn streaming_text_is_not_final_until_it_ends() {
        assert!(!is_final(&text_part("partial", None), "assistant"));
        assert!(is_final(&text_part("done", Some(9)), "assistant"));
    }

    #[test]
    fn user_text_is_final_without_an_end_time() {
        assert!(is_final(&text_part("hi", None), "user"));
    }

    #[test]
    fn synthetic_text_is_dropped() {
        let part = Part::Text {
            id: "prt_1".to_owned(),
            message_id: None,
            text: "system reminder".to_owned(),
            time: Some(PartTime { start: 1, end: Some(2) }),
            synthetic: Some(true),
        };
        assert!(!is_final(&part, "assistant"));
    }

    #[test]
    fn assistant_text_carries_both_dialects() {
        let got = part_payloads(&text_part("hello", Some(2)), "assistant");
        assert_eq!(got.len(), 1);
        let (kind, p) = &got[0];
        assert_eq!(*kind, Kind::Message);
        assert_eq!(p["type"], "text");
        assert_eq!(p["content"], "hello");
        assert_eq!(p["role"], "assistant");
        assert_eq!(p["text"], "hello");
        assert_eq!(p["message_id"], "msg_1");
    }

    #[test]
    fn user_text_is_prefixed_in_the_canonical_field_only() {
        let (_, p) = part_payloads(&text_part("do it", None), "user").remove(0);
        assert_eq!(p["content"], "▷ User: do it");
        assert_eq!(p["text"], "do it");
        assert_eq!(p["role"], "user");
    }

    #[test]
    fn repeated_user_text_differs_by_part_id() {
        let mut part = text_part("continue", None);
        let (_, first) = part_payloads(&part, "user").remove(0);
        let (_, replay) = part_payloads(&part, "user").remove(0);
        assert_eq!(first, replay);
        if let Part::Text { id, .. } = &mut part {
            "prt_2".clone_into(id);
        }
        let (_, second) = part_payloads(&part, "user").remove(0);
        assert_eq!(first["text"], second["text"]);
        assert_ne!(first, second);
    }

    #[test]
    fn empty_text_emits_nothing() {
        assert!(part_payloads(&text_part("   ", Some(2)), "assistant").is_empty());
    }

    #[test]
    fn reasoning_maps_to_thinking_text() {
        let part = Part::Reasoning {
            id: "prt_2".to_owned(),
            message_id: Some("msg_1".to_owned()),
            text: "thinking".to_owned(),
            time: Some(PartTime { start: 1, end: Some(2) }),
        };
        assert!(is_final(&part, "assistant"));
        let (_, p) = part_payloads(&part, "assistant").remove(0);
        assert_eq!(p["role"], "assistant_thinking");
        assert_eq!(p["content"], "thinking");
    }

    #[test]
    fn completed_tool_emits_call_and_result() {
        let part = Part::Tool {
            id: "prt_3".to_owned(),
            message_id: Some("msg_1".to_owned()),
            tool: "read".to_owned(),
            call_id: Some("call_1".to_owned()),
            state: ToolState::Completed {
                input: json!({ "filePath": "a.rs" }),
                output: "fn main() {}".to_owned(),
                title: Some("a.rs".to_owned()),
            },
        };
        assert!(is_final(&part, "assistant"));
        let got = part_payloads(&part, "assistant");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].1["type"], "tool_call");
        assert_eq!(got[0].1["tool"], "read");
        assert_eq!(got[0].1["input"]["filePath"], "a.rs");
        assert_eq!(got[1].1["type"], "tool_result");
        assert_eq!(got[1].1["output_summary"], "fn main() {}");
        assert_eq!(got[1].1["content"], "fn main() {}");
        assert_eq!(got[1].1["kind"], "tool_result");
        assert_eq!(got[1].1["is_error"], false);
        assert_eq!(got[0].1["tool_use_id"], "call_1");
        assert_eq!(got[0].1["id"], "call_1");
        assert_eq!(got[1].1["tool_use_id"], "call_1");
    }

    #[test]
    fn a_tool_without_a_call_id_pairs_on_its_part_id() {
        let part = Part::Tool {
            id: "prt_7".to_owned(),
            message_id: None,
            tool: "bash".to_owned(),
            call_id: None,
            state: ToolState::Error { input: json!({}), error: "denied".to_owned() },
        };
        let got = part_payloads(&part, "assistant");
        assert_eq!(got[0].1["tool_use_id"], "prt_7");
        assert_eq!(got[1].1["tool_use_id"], "prt_7");
    }

    #[test]
    fn errored_tool_marks_the_result() {
        let part = Part::Tool {
            id: "prt_4".to_owned(),
            message_id: None,
            tool: "bash".to_owned(),
            call_id: None,
            state: ToolState::Error { input: json!({}), error: "denied".to_owned() },
        };
        let got = part_payloads(&part, "assistant");
        assert_eq!(got[1].1["is_error"], true);
        assert_eq!(got[1].1["output_summary"], "denied");
    }

    #[test]
    fn running_tool_is_not_emitted() {
        let part = Part::Tool {
            id: "prt_5".to_owned(),
            message_id: None,
            tool: "bash".to_owned(),
            call_id: None,
            state: ToolState::Running { input: json!({}), title: None },
        };
        assert!(!is_final(&part, "assistant"));
        assert!(part_payloads(&part, "assistant").is_empty());
    }

    #[test]
    fn tool_output_is_capped() {
        let part = Part::Tool {
            id: "prt_6".to_owned(),
            message_id: None,
            tool: "bash".to_owned(),
            call_id: None,
            state: ToolState::Completed {
                input: json!({}),
                output: "x".repeat(TOOL_OUTPUT_CAP + 500),
                title: None,
            },
        };
        let got = part_payloads(&part, "assistant");
        assert_eq!(got[1].1["output_summary"].as_str().unwrap().len(), TOOL_OUTPUT_CAP);
    }

    fn assistant_info(completed: Option<i64>, tokens: Option<Tokens>) -> MessageInfo {
        MessageInfo {
            id: "msg_1".to_owned(),
            role: "assistant".to_owned(),
            time: MessageTime { created: 1, completed },
            tokens,
            ..MessageInfo::default()
        }
    }

    #[test]
    fn token_usage_emitted_on_completion() {
        let tokens = Tokens {
            input: 120,
            output: 42,
            reasoning: 0,
            cache: TokenCache { read: 7, write: 3 },
        };
        let evt = token_usage("ses_1", &assistant_info(Some(2), Some(tokens))).unwrap();
        match evt {
            AdapterEvent::TokenUsage {
                local_id,
                message_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
            } => {
                assert_eq!(local_id, "ses_1");
                assert_eq!(message_id, "msg_1");
                assert_eq!(
                    (input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens),
                    (120, 42, 7, 3)
                );
            }
            other => panic!("expected TokenUsage, got {other:?}"),
        }
    }

    #[test]
    fn token_usage_skipped_while_in_flight_or_empty() {
        let tokens = Tokens { input: 1, ..Tokens::default() };
        assert!(token_usage("ses_1", &assistant_info(None, Some(tokens))).is_none());
        assert!(token_usage("ses_1", &assistant_info(Some(2), None)).is_none());
        assert!(token_usage("ses_1", &assistant_info(Some(2), Some(Tokens::default()))).is_none());

        let mut user = assistant_info(Some(2), Some(tokens));
        user.role = "user".to_owned();
        assert!(token_usage("ses_1", &user).is_none());
    }

    #[test]
    fn error_text_prefers_the_nested_message() {
        assert_eq!(
            error_text(&json!({ "name": "ProviderAuthError", "data": { "message": "401" } })),
            "ProviderAuthError: 401"
        );
        assert_eq!(error_text(&json!({ "name": "UnknownError" })), "UnknownError");
    }
}
