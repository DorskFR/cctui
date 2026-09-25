use std::collections::{HashMap, VecDeque};

use cctui_proto::codex_catalog::CodexModel;
use serde_json::{Value, json};

use super::rpc::ApprovalKind;

/// A turn lifecycle transition parsed from a `turn/started` or `turn/completed`
/// notification. The driver tracks the active turn id from these so a
/// follow-up message is routed via `turn/steer` into the running turn instead
/// of a second `turn/start` codex would reject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnLifecycle {
    Started { turn_id: String },
    Completed { turn_id: String },
}

/// Extract a [`TurnLifecycle`] from a `turn/started` / `turn/completed`
/// notification. Both carry `params.turn.id`; anything else yields `None`.
#[must_use]
pub fn turn_lifecycle(v: &Value) -> Option<TurnLifecycle> {
    let method = v.get("method").and_then(Value::as_str)?;
    let turn_id = v.pointer("/params/turn/id").and_then(Value::as_str)?.to_owned();
    match method {
        "turn/started" => Some(TurnLifecycle::Started { turn_id }),
        "turn/completed" => Some(TurnLifecycle::Completed { turn_id }),
        _ => None,
    }
}

/// Tracks the session's in-flight turn. `turn/started` sets the
/// active turn; a `turn/completed` for the SAME turn clears it. The active id
/// selects `turn/steer` (with it as `expectedTurnId`) over `turn/start`.
#[derive(Debug, Default)]
pub struct ActiveTurn {
    id: Option<String>,
}

impl ActiveTurn {
    pub fn apply(&mut self, ev: &TurnLifecycle) {
        match ev {
            TurnLifecycle::Started { turn_id } => self.id = Some(turn_id.clone()),
            TurnLifecycle::Completed { turn_id } => {
                if self.id.as_deref() == Some(turn_id.as_str()) {
                    self.id = None;
                }
            }
        }
    }

    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    pub fn clear(&mut self) {
        self.id = None;
    }
}

/// Accumulates streamed item deltas by item id. Codex ships an item
/// as `item/started` → `item/<kind>/delta`* → `item/completed`. The completed
/// item is authoritative for rendering — mirroring the claude adapter, which
/// drops partial SSE deltas in favour of the coalesced final frame — so deltas
/// never emit their own events. They are consumed here only to back-fill a
/// completed item the server left text-empty: reasoning items in particular
/// ship `content: []` / encrypted content while the visible reasoning arrived
/// solely via `item/reasoning/textDelta`, so without this they render blank.
#[derive(Debug, Default)]
pub struct ItemAccumulator {
    /// `agentMessage` / `plan` text (`item/agentMessage|plan/delta`).
    text: HashMap<String, String>,
    /// `reasoning` content (`item/reasoning/textDelta`).
    reasoning: HashMap<String, String>,
    /// `reasoning` summary (`item/reasoning/summaryTextDelta`).
    summary: HashMap<String, String>,
    /// `commandExecution` aggregated output (`item/commandExecution/outputDelta`).
    output: HashMap<String, String>,
    /// itemId → item type, seeded from `item/started`.
    started: HashMap<String, String>,
}

fn push_delta(map: &mut HashMap<String, String>, v: &Value) {
    if let (Some(id), Some(delta)) = (
        v.pointer("/params/itemId").and_then(Value::as_str),
        v.pointer("/params/delta").and_then(Value::as_str),
    ) {
        map.entry(id.to_owned()).or_default().push_str(delta);
    }
}

/// A JSON `content`/`summary` field carries no renderable text: absent, an
/// empty array, or an array of only empty strings.
fn is_text_empty(field: Option<&Value>) -> bool {
    match field {
        None | Some(Value::Null) => true,
        Some(Value::Array(a)) => {
            a.iter().all(|e| e.as_str().is_none_or(str::is_empty) && e.get("text").is_none())
        }
        Some(Value::String(s)) => s.is_empty(),
        _ => false,
    }
}

impl ItemAccumulator {
    /// Feed one inbound notification: record `item/started` item types and
    /// append `item/*/delta` text by item id. No-op for anything else.
    pub fn note(&mut self, v: &Value) {
        match v.get("method").and_then(Value::as_str) {
            Some("item/started") => {
                if let (Some(id), Some(ty)) = (
                    v.pointer("/params/item/id").and_then(Value::as_str),
                    v.pointer("/params/item/type").and_then(Value::as_str),
                ) {
                    self.started.insert(id.to_owned(), ty.to_owned());
                }
            }
            Some("item/agentMessage/delta" | "item/plan/delta") => push_delta(&mut self.text, v),
            Some("item/reasoning/textDelta") => push_delta(&mut self.reasoning, v),
            Some("item/reasoning/summaryTextDelta") => push_delta(&mut self.summary, v),
            Some("item/commandExecution/outputDelta" | "command/exec/outputDelta") => {
                push_delta(&mut self.output, v);
            }
            _ => {}
        }
    }

    /// If `v` is an `item/completed`, back-fill any empty text/output field on
    /// the item from the accumulated stream, then forget that item's buffers.
    /// Every other notification is returned unchanged.
    #[must_use]
    pub fn enrich_completed(&mut self, mut v: Value) -> Value {
        if v.get("method").and_then(Value::as_str) != Some("item/completed") {
            return v;
        }
        let Some(item) = v.pointer_mut("/params/item") else { return v };
        let Some(id) = item.get("id").and_then(Value::as_str).map(str::to_owned) else {
            return v;
        };
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "agentMessage" | "plan" => {
                if let Some(buf) = self.text.get(&id).filter(|b| !b.is_empty())
                    && item.get("text").and_then(Value::as_str).unwrap_or_default().is_empty()
                {
                    item["text"] = json!(buf);
                }
            }
            "reasoning" => {
                if let Some(buf) = self.reasoning.get(&id).filter(|b| !b.is_empty())
                    && is_text_empty(item.get("content"))
                {
                    item["content"] = json!([buf]);
                }
                if let Some(buf) = self.summary.get(&id).filter(|b| !b.is_empty())
                    && is_text_empty(item.get("summary"))
                {
                    item["summary"] = json!([buf]);
                }
            }
            "commandExecution" => {
                if let Some(buf) = self.output.get(&id).filter(|b| !b.is_empty())
                    && item
                        .get("aggregatedOutput")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .is_empty()
                {
                    item["aggregatedOutput"] = json!(buf);
                }
            }
            _ => {}
        }
        self.forget(&id);
        v
    }

    fn forget(&mut self, id: &str) {
        self.text.remove(id);
        self.reasoning.remove(id);
        self.summary.remove(id);
        self.output.remove(id);
        self.started.remove(id);
    }
}

/// How a user message is delivered given the current active turn:
/// steer into a running turn, else start a fresh one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptDispatch {
    Start,
    Steer { turn_id: String },
}

#[must_use]
pub fn prompt_dispatch(active: &ActiveTurn) -> PromptDispatch {
    active.id().map_or(PromptDispatch::Start, |turn_id| PromptDispatch::Steer {
        turn_id: turn_id.to_owned(),
    })
}

/// How to recover from a `turn/steer` failure. A turn that just ended
/// (the common `expectedTurnId` race) frees the turn slot, so the message is
/// retried as a fresh `turn/start`; a turn that is running but non-steerable
/// (`/review` or manual `/compact`, `activeTurnNotSteerable`) would reject a
/// `turn/start` too, so the message is rejected visibly instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteerRecovery {
    FallbackToStart,
    Reject,
}

#[must_use]
pub fn steer_recovery(error: &str) -> SteerRecovery {
    if error.to_lowercase().contains("steerable") {
        SteerRecovery::Reject
    } else {
        SteerRecovery::FallbackToStart
    }
}

/// What the driver knows about its one thread between frames.
#[derive(Default)]
pub(super) struct ThreadState {
    /// Empty until `thread/start|resume|fork` answers.
    pub(super) local_id: String,
    pub(super) codex_version: Option<String>,
    pub(super) rollout_path: Option<String>,
    pub(super) active_turn: ActiveTurn,
    pub(super) items: ItemAccumulator,
    /// `request_id` (surfaced to TUI) → (`rpc_id` echoed to codex, decision kind).
    pub(super) pending_approvals: HashMap<String, (Value, ApprovalKind)>,
    /// Parked `item/tool/requestUserInput` requests: the next user reply
    /// answers the oldest one (codex blocks the turn on it) rather than
    /// starting a fresh turn.
    pub(super) pending_questions: VecDeque<(Value, Vec<String>)>,
    /// `turn/steer` request id → its text, for the `turn/start` fallback.
    pub(super) steer_texts: HashMap<i64, String>,
    /// In-place model/effort override. A `SetModel` records it here; every
    /// subsequent `turn/start` carries it so codex adopts it as the later
    /// default. Left `None` at launch — the spawn-time `-c model=`/
    /// `-c model_reasoning_effort=` flags already seed the initial turns.
    pub(super) override_model: Option<String>,
    pub(super) override_effort: Option<String>,
    /// `model/list` pages accumulated over this session's authenticated
    /// connection; the counter bounds `nextCursor` following.
    pub(super) model_catalog: Vec<CodexModel>,
    pub(super) model_catalog_pages: usize,
    /// `model/list` issued before the thread request to reject an unknown
    /// `-c model=` up front; while set, that request is part of the handshake.
    pub(super) validating_model: bool,
    pub(super) catalog_sent: bool,
}

#[cfg(test)]
mod tests {
    use super::super::rpc::{Incoming, classify};
    use super::*;
    use cctui_proto::adapter::AdapterEvent;

    // --- active-turn routing via turn/steer ------------------------

    #[test]
    fn turn_lifecycle_parses_started_and_completed() {
        let started = json!({"method": "turn/started", "params": {"threadId": "t", "turn": {
            "id": "turn-1", "items": [], "status": "inProgress"}}});
        assert_eq!(
            turn_lifecycle(&started),
            Some(TurnLifecycle::Started { turn_id: "turn-1".to_owned() })
        );
        let completed = json!({"method": "turn/completed", "params": {"threadId": "t", "turn": {
            "id": "turn-1", "items": [], "status": "completed"}}});
        assert_eq!(
            turn_lifecycle(&completed),
            Some(TurnLifecycle::Completed { turn_id: "turn-1".to_owned() })
        );
        assert_eq!(turn_lifecycle(&json!({"method": "thread/status/changed", "params": {}})), None);
        assert_eq!(turn_lifecycle(&json!({"method": "turn/started", "params": {}})), None);
    }

    #[test]
    fn active_turn_tracks_started_then_completed() {
        let mut active = ActiveTurn::default();
        assert_eq!(active.id(), None);
        active.apply(&TurnLifecycle::Started { turn_id: "turn-1".to_owned() });
        assert_eq!(active.id(), Some("turn-1"));
        active.apply(&TurnLifecycle::Completed { turn_id: "other".to_owned() });
        assert_eq!(active.id(), Some("turn-1"));
        active.apply(&TurnLifecycle::Completed { turn_id: "turn-1".to_owned() });
        assert_eq!(active.id(), None);
    }

    #[test]
    fn active_turn_started_supersedes_previous() {
        let mut active = ActiveTurn::default();
        active.apply(&TurnLifecycle::Started { turn_id: "turn-1".to_owned() });
        active.apply(&TurnLifecycle::Started { turn_id: "turn-2".to_owned() });
        assert_eq!(active.id(), Some("turn-2"));
        active.clear();
        assert_eq!(active.id(), None);
    }

    #[test]
    fn prompt_dispatch_selects_steer_when_turn_active() {
        let mut active = ActiveTurn::default();
        assert_eq!(prompt_dispatch(&active), PromptDispatch::Start);
        active.apply(&TurnLifecycle::Started { turn_id: "turn-9".to_owned() });
        assert_eq!(
            prompt_dispatch(&active),
            PromptDispatch::Steer { turn_id: "turn-9".to_owned() }
        );
    }

    #[test]
    fn steer_recovery_rejects_non_steerable_else_falls_back() {
        assert_eq!(
            steer_recovery("codex app-server error -32000: activeTurnNotSteerable"),
            SteerRecovery::Reject
        );
        assert_eq!(steer_recovery("turn is not steerable"), SteerRecovery::Reject);
        assert_eq!(
            steer_recovery("codex app-server error -32602: expectedTurnId mismatch"),
            SteerRecovery::FallbackToStart
        );
    }

    // --- item/started + delta accumulation, new item types ---------

    const ITEM_STREAM_FIXTURE: &str = include_str!("../fixtures/item_stream.jsonl");

    fn fixture_lines() -> Vec<Value> {
        ITEM_STREAM_FIXTURE
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Value>(l).expect("fixture line is valid JSON"))
            .collect()
    }

    #[test]
    fn item_started_and_deltas_emit_no_event_of_their_own() {
        for v in fixture_lines() {
            let method = v.get("method").and_then(Value::as_str).unwrap_or_default();
            if method == "item/started" || method.ends_with("Delta") || method.contains("/delta") {
                assert!(
                    matches!(classify("t", &v), Incoming::Traced { .. }),
                    "{method} must not emit its own event",
                );
            }
        }
    }

    #[test]
    fn accumulator_backfills_agent_message_text() {
        // A completed agentMessage whose `text` the server left empty is
        // back-filled from the concatenated `item/agentMessage/delta` stream.
        let mut acc = ItemAccumulator::default();
        acc.note(
            &json!({"method":"item/started","params":{"item":{"id":"m","type":"agentMessage"}}}),
        );
        acc.note(
            &json!({"method":"item/agentMessage/delta","params":{"itemId":"m","delta":"Hel"}}),
        );
        acc.note(&json!({"method":"item/agentMessage/delta","params":{"itemId":"m","delta":"lo"}}));
        let completed = json!({"method":"item/completed","params":{"item":{"id":"m","type":"agentMessage","text":""}}});
        let enriched = acc.enrich_completed(completed);
        assert_eq!(enriched.pointer("/params/item/text").and_then(Value::as_str), Some("Hello"));
    }

    #[test]
    fn accumulator_keeps_authoritative_completed_text() {
        // When the completed item already carries text, the stream is dropped
        // (the completed frame is authoritative) — no duplication.
        let mut acc = ItemAccumulator::default();
        acc.note(
            &json!({"method":"item/agentMessage/delta","params":{"itemId":"m","delta":"partial"}}),
        );
        let completed = json!({"method":"item/completed","params":{"item":{"id":"m","type":"agentMessage","text":"final answer"}}});
        let enriched = acc.enrich_completed(completed);
        assert_eq!(
            enriched.pointer("/params/item/text").and_then(Value::as_str),
            Some("final answer")
        );
    }

    #[test]
    fn accumulator_backfills_reasoning_from_text_deltas() {
        // Reasoning ships `content: []` + encrypted content on completion; the
        // visible reasoning only arrived via `item/reasoning/textDelta`.
        let mut acc = ItemAccumulator::default();
        acc.note(&json!({"method":"item/started","params":{"item":{"id":"r","type":"reasoning"}}}));
        acc.note(&json!({"method":"item/reasoning/textDelta","params":{"itemId":"r","contentIndex":0,"delta":"think "}}));
        acc.note(&json!({"method":"item/reasoning/textDelta","params":{"itemId":"r","contentIndex":0,"delta":"hard"}}));
        let completed = json!({"method":"item/completed","params":{"item":{
            "id":"r","type":"reasoning","content":[],"summary":[],"encrypted_content":"gAAAA"}}});
        let enriched = acc.enrich_completed(completed);
        let content = enriched.pointer("/params/item/content").and_then(Value::as_array).unwrap();
        assert_eq!(content[0].as_str(), Some("think hard"));
    }

    #[test]
    fn accumulator_backfills_command_output() {
        let mut acc = ItemAccumulator::default();
        acc.note(&json!({"method":"item/commandExecution/outputDelta","params":{"itemId":"c","delta":"line1\n"}}));
        let completed = json!({"method":"item/completed","params":{"item":{
            "id":"c","type":"commandExecution","command":"ls","aggregatedOutput":""}}});
        let enriched = acc.enrich_completed(completed);
        assert_eq!(
            enriched.pointer("/params/item/aggregatedOutput").and_then(Value::as_str),
            Some("line1\n")
        );
    }

    #[test]
    fn accumulator_forgets_item_after_completion() {
        // A second item reusing a fresh id must not inherit a prior buffer.
        let mut acc = ItemAccumulator::default();
        acc.note(&json!({"method":"item/agentMessage/delta","params":{"itemId":"m","delta":"x"}}));
        let _ = acc.enrich_completed(
            json!({"method":"item/completed","params":{"item":{"id":"m","type":"agentMessage","text":""}}}),
        );
        // Re-completing the same id with empty text now has nothing to inject.
        let again = acc.enrich_completed(
            json!({"method":"item/completed","params":{"item":{"id":"m","type":"agentMessage","text":""}}}),
        );
        assert_eq!(again.pointer("/params/item/text").and_then(Value::as_str), Some(""));
    }

    #[test]
    fn enrich_completed_passes_non_completed_through() {
        let mut acc = ItemAccumulator::default();
        let v = json!({"method":"turn/started","params":{"turn":{"id":"t"}}});
        assert_eq!(acc.enrich_completed(v.clone()), v);
    }

    #[test]
    fn fixture_stream_drives_full_pipeline() {
        // The full started→delta→completed sequence in the fixture: only the
        // completed items emit events, deltas are accumulated, and the empty
        // reasoning item is back-filled from its text deltas.
        let mut acc = ItemAccumulator::default();
        let mut completed_types: Vec<String> = Vec::new();
        let mut reasoning_text: Option<String> = None;
        for v in fixture_lines() {
            acc.note(&v);
            let v = acc.enrich_completed(v);
            if let Incoming::Event(evt) = classify("t", &v) {
                match evt {
                    AdapterEvent::Message { payload, .. }
                    | AdapterEvent::ToolUse { payload, .. } => {
                        if let Some(ty) = payload.get("type").and_then(Value::as_str) {
                            completed_types.push(ty.to_owned());
                            if ty == "reasoning" {
                                reasoning_text = payload
                                    .get("content")
                                    .and_then(Value::as_array)
                                    .and_then(|a| a.first())
                                    .and_then(Value::as_str)
                                    .map(str::to_owned);
                            }
                        }
                    }
                    // The failed turn/completed surfaces a failed Status.
                    AdapterEvent::Status { state, .. } => {
                        assert_eq!(state.as_deref(), Some("failed"));
                    }
                    other => panic!("unexpected event {other:?}"),
                }
            }
        }
        for ty in [
            "agentMessage",
            "reasoning",
            "commandExecution",
            "plan",
            "fileChange",
            "enteredReviewMode",
            "mcpToolCall",
            "dynamicToolCall",
            "imageView",
            "contextCompaction",
        ] {
            assert!(completed_types.contains(&ty.to_owned()), "missing completed item {ty}");
        }
        assert_eq!(reasoning_text.as_deref(), Some("First I will list the files."));
    }
}
