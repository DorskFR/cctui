//! v2 bus events → [`Signal`]s, with rows in the same dialects as v1.
//!
//! User prompts reach the bus only as inbox items, so a prompt's row is
//! emitted when its item is delivered.

use std::collections::{HashMap, HashSet};

use cctui_proto::adapter::AdapterEvent;
use serde_json::{Value, json};

use super::events::{Event, Inbox, Step, Text, TextDelta, Tool};
use crate::adapters::opencode::backend::Signal;
use crate::adapters::opencode::normalize::{self as v1, Kind};

/// A tool's name and input arrive ahead of its result, and a prompt's text
/// ahead of its delivery.
#[derive(Debug, Default)]
pub struct Tracker {
    tools: HashMap<String, (String, Value)>,
    inbox: HashMap<String, String>,
    emitted: HashSet<String>,
    emitted_usage: HashSet<String>,
}

impl Tracker {
    pub fn signals(&mut self, session_id: &str, evt: Event) -> Vec<Signal> {
        match evt {
            Event::ExecutionStarted(_) => vec![working(None)],
            Event::ExecutionSucceeded(_) | Event::ExecutionInterrupted(_) => {
                vec![Signal::TurnEnded]
            }
            Event::ExecutionFailed(failure) => {
                vec![Signal::Failed(failure_text(failure.error.as_ref())), Signal::TurnEnded]
            }
            Event::StepStarted(_) => vec![Signal::AssistantActive],
            Event::StepEnded(step) => self.usage(session_id, step).into_iter().collect(),
            Event::StepFailed(failure) => {
                let text = failure_text(failure.error.as_ref());
                vec![working(Some(format!("step failed: {text}")))]
            }
            Event::RetryScheduled(retry) => {
                let text = failure_text(retry.error.as_ref());
                vec![working(Some(format!("provider retry {}: {text}", retry.attempt)))]
            }
            Event::TextDelta(delta) => delta_signal(session_id, &delta).into_iter().collect(),
            Event::TextEnded(text) => self.text(&text, false).into_iter().collect(),
            Event::ReasoningEnded(text) => self.text(&text, true).into_iter().collect(),
            Event::ToolInputStarted(tool) | Event::ToolCalled(tool) => {
                self.remember(tool);
                Vec::new()
            }
            Event::ToolSucceeded(tool) => self.tool_done(tool, false).into_iter().collect(),
            Event::ToolFailed(tool) => self.tool_done(tool, true).into_iter().collect(),
            Event::InboxEnqueued(inbox) => {
                self.enqueue(inbox);
                Vec::new()
            }
            Event::InboxDelivered(inbox) => self.deliver(&inbox).into_iter().collect(),
            Event::SessionDeleted(_) => vec![Signal::Deleted],
            Event::PermissionAsked(ask) => vec![Signal::PermissionAsked {
                request_id: ask.id,
                tool: ask.action,
                input: json!({ "resources": ask.resources, "save": ask.save }),
            }],
            Event::PermissionReplied(reply) => {
                vec![Signal::PermissionReplied { request_id: reply.request_id }]
            }
            Event::Other => Vec::new(),
        }
    }

    fn usage(&mut self, session_id: &str, step: Step) -> Option<Signal> {
        let tokens = step.tokens.filter(|t| !t.is_empty())?;
        if !self.emitted_usage.insert(step.message_id.clone()) {
            return None;
        }
        Some(Signal::Usage(Box::new(AdapterEvent::TokenUsage {
            local_id: session_id.to_owned(),
            message_id: step.message_id,
            input_tokens: tokens.input,
            output_tokens: tokens.output,
            cache_read_tokens: tokens.cache.read,
            cache_creation_tokens: tokens.cache.write,
        })))
    }

    fn text(&mut self, text: &Text, reasoning: bool) -> Option<Signal> {
        let kind = if reasoning { "reasoning" } else { "text" };
        let id = text_id(kind, &text.message_id, text.ordinal);
        if text.text.trim().is_empty() || !self.emitted.insert(id.clone()) {
            return None;
        }
        let payload = if reasoning {
            v1::reasoning_payload(&text.text, Some(&text.message_id))
        } else {
            v1::text_payload("assistant", &text.text, Some(&text.message_id), &id)
        };
        Some(Signal::Rows(vec![(Kind::Message, payload)]))
    }

    fn remember(&mut self, tool: Tool) {
        let entry = self.tools.entry(tool.id).or_default();
        if let Some(name) = tool.name {
            entry.0 = name;
        }
        if let Some(input) = tool.input {
            entry.1 = input;
        }
    }

    fn tool_done(&mut self, tool: Tool, failed: bool) -> Option<Signal> {
        if !self.emitted.insert(format!("tool:{}", tool.id)) {
            return None;
        }
        let (name, input) = self.tools.remove(&tool.id).unwrap_or_default();
        let name =
            if name.is_empty() { tool.name.unwrap_or_else(|| "tool".to_owned()) } else { name };
        let input = tool.input.unwrap_or(input);
        let output = content_text(&tool.content);
        let output = if failed { tool.error.as_ref().map_or(output, error_text) } else { output };
        Some(Signal::Rows(vec![
            (Kind::ToolUse, v1::tool_call_payload(&name, &input, &tool.id)),
            (Kind::ToolUse, v1::tool_result_payload(&output, failed, &tool.id)),
        ]))
    }

    fn enqueue(&mut self, inbox: Inbox) {
        let Some(item) = inbox.item.filter(|item| item.kind == "user") else { return };
        if let Some(text) = item.payload.get("text").and_then(Value::as_str) {
            self.inbox.insert(inbox.inbox_id, text.to_owned());
        }
    }

    fn deliver(&mut self, inbox: &Inbox) -> Option<Signal> {
        let text = self.inbox.remove(&inbox.inbox_id)?;
        if text.trim().is_empty() {
            return None;
        }
        let payload = v1::text_payload("user", &text, None, &inbox.inbox_id);
        Some(Signal::Rows(vec![(Kind::Message, payload)]))
    }
}

fn working(detail: Option<String>) -> Signal {
    Signal::Tempo { tempo: "working".to_owned(), detail }
}

fn text_id(kind: &str, message_id: &str, ordinal: u64) -> String {
    format!("{kind}:{message_id}:{ordinal}")
}

/// A delta for a watched session, named like the row its `text.ended` makes.
fn delta_signal(session_id: &str, delta: &TextDelta) -> Option<Signal> {
    if delta.delta.is_empty() || !crate::text_deltas::wanted(session_id) {
        return None;
    }
    Some(Signal::TextDelta {
        item_id: text_id("text", &delta.message_id, delta.ordinal),
        delta: delta.delta.clone(),
    })
}

fn content_text(content: &[Value]) -> String {
    content
        .iter()
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn failure_text(error: Option<&Value>) -> String {
    error.map_or_else(|| "session error".to_owned(), error_text)
}

/// Render a v2 error (`{ type, message, status, response }`) as a line.
#[must_use]
pub fn error_text(error: &Value) -> String {
    let kind = error.get("type").and_then(Value::as_str).unwrap_or("error");
    let message = error.get("message").and_then(Value::as_str).unwrap_or_default();
    if message.is_empty() { kind.to_owned() } else { format!("{kind}: {message}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::opencode::events::SseDecoder;

    const FIXTURE: &str = include_str!("../fixtures/v2_events.sse");
    const SESSION: &str = "ses_ecd86c6adffeSeDX15STTLCF8s";

    fn replay() -> Vec<Signal> {
        let mut tracker = Tracker::default();
        let mut out = Vec::new();
        for raw in SseDecoder::new().push(FIXTURE) {
            let evt = Event::decode(&raw).unwrap();
            if let Some(session_id) = evt.session_id().map(str::to_owned) {
                assert_eq!(session_id, SESSION);
                out.extend(tracker.signals(&session_id, evt));
            }
        }
        out
    }

    const fn label(signal: &Signal) -> &'static str {
        match signal {
            Signal::Rows(_) => "rows",
            Signal::TextDelta { .. } => "delta",
            Signal::AssistantActive => "assistant",
            Signal::Usage(_) => "usage",
            Signal::Tempo { .. } => "tempo",
            Signal::TurnEnded => "turn-end",
            Signal::Failed(_) => "failed",
            Signal::Deleted => "deleted",
            Signal::PermissionAsked { .. } => "ask",
            Signal::PermissionReplied { .. } => "replied",
        }
    }

    fn rows(signals: &[Signal]) -> Vec<(Kind, Value)> {
        signals
            .iter()
            .filter_map(|s| if let Signal::Rows(rows) = s { Some(rows.clone()) } else { None })
            .flatten()
            .collect()
    }

    #[test]
    fn a_text_delta_is_a_signal_only_for_a_text_watcher() {
        const VOICE: &str = "ses_v2_voice";
        let raw = |session: &str| {
            format!(
                r#"{{"type":"session.text.delta","data":{{"sessionID":"{session}","assistantMessageID":"msg_1","ordinal":0,"delta":"All "}}}}"#
            )
        };
        let mut tracker = Tracker::default();
        let silent =
            tracker.signals("ses_v2_silent", Event::decode(&raw("ses_v2_silent")).unwrap());
        assert!(silent.is_empty(), "no watcher, no delta: {silent:?}");

        crate::text_deltas::set(VOICE, true);
        let got = tracker.signals(VOICE, Event::decode(&raw(VOICE)).unwrap());
        crate::text_deltas::set(VOICE, false);
        match got.as_slice() {
            [Signal::TextDelta { item_id, delta }] => {
                assert_eq!(item_id, "text:msg_1:0", "named like the final row");
                assert_eq!(delta, "All ");
            }
            other => panic!("expected one delta, got {other:?}"),
        }
    }

    /// Three turns: a tool call behind a permission then a text answer, a
    /// retried turn the user interrupted, and a turn the provider rejected.
    #[test]
    fn the_recorded_stream_maps_turn_by_turn() {
        let got: Vec<&str> = replay().iter().map(label).collect();
        assert_eq!(
            got,
            vec![
                "tempo",
                "rows",
                "assistant",
                "ask",
                "replied",
                "rows",
                "usage",
                "assistant",
                "rows",
                "usage",
                "turn-end",
                "tempo",
                "rows",
                "assistant",
                "tempo",
                "assistant",
                "tempo",
                "assistant",
                "turn-end",
                "tempo",
                "rows",
                "assistant",
                "tempo",
                "failed",
                "turn-end",
            ]
        );
    }

    #[test]
    fn user_prompts_and_the_answer_reach_the_transcript() {
        let rows = rows(&replay());
        let users: Vec<&str> = rows
            .iter()
            .filter(|(_, p)| p["role"] == "user")
            .map(|(_, p)| p["text"].as_str().unwrap())
            .collect();
        assert_eq!(users, vec!["list the files", "again", "third"]);
        let first = rows.iter().find(|(_, p)| p["role"] == "user").unwrap();
        assert_eq!(first.1["content"], "▷ User: list the files");
        assert_eq!(first.1["line_id"], "msg_132793980001K2PDSR5bLL6pME");

        let answer = rows.iter().find(|(_, p)| p["role"] == "assistant").unwrap();
        assert_eq!(answer.0, Kind::Message);
        assert_eq!(answer.1["text"], "All done.");
        assert_eq!(answer.1["content"], "All done.");
        assert_eq!(answer.1["message_id"], "msg_132796bc1001lR434V766SMvS9");
    }

    #[test]
    fn a_tool_call_pairs_its_name_input_and_output() {
        let rows = rows(&replay());
        let tools: Vec<&Value> =
            rows.iter().filter(|(k, _)| *k == Kind::ToolUse).map(|(_, p)| p).collect();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["type"], "tool_call");
        assert_eq!(tools[0]["tool"], "shell");
        assert_eq!(tools[0]["input"]["command"], "ls -la");
        assert_eq!(tools[0]["tool_use_id"], "call_1");
        assert_eq!(tools[1]["type"], "tool_result");
        assert_eq!(tools[1]["is_error"], false);
        assert_eq!(tools[1]["tool_use_id"], "call_1");
        assert!(tools[1]["output_summary"].as_str().unwrap().starts_with("total 8\n"));
    }

    #[test]
    fn the_permission_ask_and_reply_share_a_request_id() {
        let signals = replay();
        let ask = signals.iter().find_map(|s| match s {
            Signal::PermissionAsked { request_id, tool, input } => Some((request_id, tool, input)),
            _ => None,
        });
        let (request_id, tool, input) = ask.expect("no permission ask");
        assert_eq!(request_id, "per_1327939e900126M5lPXw3N2VfG");
        assert_eq!(tool, "shell");
        assert_eq!(input["resources"][0], "ls -la");
        assert_eq!(input["save"][0], "ls *");
        assert!(signals.iter().any(|s| matches!(
            s,
            Signal::PermissionReplied { request_id } if request_id == "per_1327939e900126M5lPXw3N2VfG"
        )));
    }

    #[test]
    fn each_step_reports_its_own_usage() {
        let usage: Vec<(String, u64, u64)> = replay()
            .into_iter()
            .filter_map(|s| match s {
                Signal::Usage(evt) => match *evt {
                    AdapterEvent::TokenUsage {
                        message_id, input_tokens, output_tokens, ..
                    } => Some((message_id, input_tokens, output_tokens)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(
            usage,
            vec![
                ("msg_1327939a1001VPs7Lc4UOaAGM6".to_owned(), 90, 10),
                ("msg_132796bc1001lR434V766SMvS9".to_owned(), 100, 5),
            ]
        );
    }

    #[test]
    fn a_retry_and_a_provider_error_say_what_failed() {
        let signals = replay();
        let details: Vec<&str> = signals
            .iter()
            .filter_map(|s| match s {
                Signal::Tempo { detail: Some(d), .. } => Some(d.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            details[0].starts_with("provider retry 2: provider.transport: ConnectionRefused"),
            "{details:?}"
        );
        assert!(details[1].starts_with("provider retry 3: "), "{details:?}");
        assert_eq!(details[2], "step failed: provider.invalid-request: model not found: mock");
        let failed = signals.iter().find_map(|s| match s {
            Signal::Failed(text) => Some(text.as_str()),
            _ => None,
        });
        assert_eq!(failed, Some("provider.invalid-request: model not found: mock"));
    }

    #[test]
    fn a_replayed_event_is_not_emitted_twice() {
        let raw = r#"{"type":"session.text.ended","data":{"sessionID":"ses_1",
            "assistantMessageID":"msg_1","ordinal":0,"text":"hi"}}"#;
        let mut tracker = Tracker::default();
        assert_eq!(tracker.signals("ses_1", Event::decode(raw).unwrap()).len(), 1);
        assert!(tracker.signals("ses_1", Event::decode(raw).unwrap()).is_empty());
    }

    #[test]
    fn a_failed_tool_reports_its_error_against_its_call() {
        let mut tracker = Tracker::default();
        let started = r#"{"type":"session.tool.input.started","data":{"sessionID":"ses_1",
            "id":"call_9","name":"edit"}}"#;
        let failed = r#"{"type":"session.tool.failed","data":{"sessionID":"ses_1","id":"call_9",
            "error":{"type":"permission.rejected","message":"user said no"}}}"#;
        assert!(tracker.signals("ses_1", Event::decode(started).unwrap()).is_empty());
        let got = rows(&tracker.signals("ses_1", Event::decode(failed).unwrap()));
        assert_eq!(got[0].1["tool"], "edit");
        assert_eq!(got[1].1["is_error"], true);
        assert_eq!(got[1].1["output_summary"], "permission.rejected: user said no");
    }

    #[test]
    fn error_text_falls_back_to_the_type() {
        assert_eq!(error_text(&json!({ "type": "provider.auth" })), "provider.auth");
        assert_eq!(error_text(&json!({})), "error");
        assert_eq!(failure_text(None), "session error");
    }
}
