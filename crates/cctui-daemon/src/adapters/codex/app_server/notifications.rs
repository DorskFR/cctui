use cctui_proto::adapter::AdapterEvent;
use serde::Deserialize;
use serde_json::{Value, json};

use super::rpc::Incoming;

/// What the adapter does with one `ServerNotification` method. Every method in
/// the pinned schema resolves to a non-[`Disposition::Unknown`] arm — enforced
/// against `schema/codex_app_server_protocol.schemas.json` by
/// `every_schema_notification_has_a_disposition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Mapped onto a dedicated adapter event.
    Mapped,
    /// Consumed elsewhere in the driver ([`ItemAccumulator`],
    /// [`turn_lifecycle`]) or a stream cctui does not render.
    Stream(&'static str),
    /// No dedicated home: recorded into the timeline as a generic notice with
    /// this severity.
    Notice(&'static str),
    /// Not in the pinned schema.
    Unknown,
}

/// Every `ServerNotification` method the pinned schema defines, paired with
/// what the adapter does with it. Kept in lockstep with
/// `schema/codex_app_server_protocol.schemas.json` in both directions by
/// `every_schema_notification_has_a_disposition` and
/// `disposition_table_has_no_methods_the_schema_lacks`.
const NOTIFICATION_DISPOSITIONS: &[(&str, Disposition)] = &[
    ("item/completed", Disposition::Mapped),
    ("thread/status/changed", Disposition::Mapped),
    ("thread/tokenUsage/updated", Disposition::Mapped),
    ("thread/name/updated", Disposition::Mapped),
    ("error", Disposition::Mapped),
    ("turn/completed", Disposition::Mapped),
    ("turn/plan/updated", Disposition::Mapped),
    ("thread/compacted", Disposition::Mapped),
    ("account/rateLimits/updated", Disposition::Mapped),
    ("turn/started", Disposition::Stream("turn lifecycle")),
    ("item/started", Disposition::Stream("item accumulator")),
    ("item/agentMessage/delta", Disposition::Stream("delta coalesced into item/completed")),
    ("item/plan/delta", Disposition::Stream("delta coalesced into item/completed")),
    ("item/reasoning/textDelta", Disposition::Stream("delta coalesced into item/completed")),
    ("item/reasoning/summaryTextDelta", Disposition::Stream("delta coalesced into item/completed")),
    ("item/reasoning/summaryPartAdded", Disposition::Stream("delta coalesced into item/completed")),
    (
        "item/commandExecution/outputDelta",
        Disposition::Stream("delta coalesced into item/completed"),
    ),
    ("command/exec/outputDelta", Disposition::Stream("delta coalesced into item/completed")),
    ("item/fileChange/outputDelta", Disposition::Stream("delta coalesced into item/completed")),
    ("process/outputDelta", Disposition::Stream("delta coalesced into item/completed")),
    (
        "item/commandExecution/terminalInteraction",
        Disposition::Stream("superseded by the completed item"),
    ),
    ("item/fileChange/patchUpdated", Disposition::Stream("superseded by the completed item")),
    ("item/mcpToolCall/progress", Disposition::Stream("superseded by the completed item")),
    ("turn/diff/updated", Disposition::Stream("superseded by the completed item")),
    ("serverRequest/resolved", Disposition::Stream("approval bookkeeping")),
    (
        "fuzzyFileSearch/sessionUpdated",
        Disposition::Stream("file-picker session cctui does not drive"),
    ),
    (
        "fuzzyFileSearch/sessionCompleted",
        Disposition::Stream("file-picker session cctui does not drive"),
    ),
    ("thread/realtime/started", Disposition::Stream("realtime voice session")),
    ("thread/realtime/itemAdded", Disposition::Stream("realtime voice session")),
    ("thread/realtime/item/started", Disposition::Stream("realtime voice session")),
    ("thread/realtime/item/transcript/delta", Disposition::Stream("realtime voice session")),
    ("thread/realtime/item/completed", Disposition::Stream("realtime voice session")),
    ("thread/realtime/transcript/delta", Disposition::Stream("realtime voice session")),
    ("thread/realtime/transcript/done", Disposition::Stream("realtime voice session")),
    ("thread/realtime/outputAudio/delta", Disposition::Stream("realtime voice session")),
    ("thread/realtime/sdp", Disposition::Stream("realtime voice session")),
    ("thread/realtime/error", Disposition::Stream("realtime voice session")),
    ("thread/realtime/closed", Disposition::Stream("realtime voice session")),
    ("fs/changed", Disposition::Stream("high-volume inventory churn")),
    ("app/list/updated", Disposition::Stream("high-volume inventory churn")),
    ("skills/changed", Disposition::Stream("high-volume inventory churn")),
    ("thread/queue/changed", Disposition::Stream("high-volume inventory churn")),
    ("mcpServer/event/stream/notification", Disposition::Stream("opaque MCP passthrough")),
    ("warning", Disposition::Notice("warning")),
    ("guardianWarning", Disposition::Notice("warning")),
    ("configWarning", Disposition::Notice("warning")),
    ("deprecationNotice", Disposition::Notice("warning")),
    ("windows/worldWritableWarning", Disposition::Notice("warning")),
    ("autoApprovalReview/strictReviewRequired", Disposition::Notice("warning")),
    ("turn/moderationMetadata", Disposition::Notice("warning")),
    ("model/rerouted", Disposition::Notice("info")),
    ("model/verification", Disposition::Notice("info")),
    ("model/safetyBuffering/updated", Disposition::Notice("info")),
    ("modelProvider/authRecoveryStarted", Disposition::Notice("info")),
    ("modelProvider/authRecoveryCompleted", Disposition::Notice("info")),
    ("mcpServer/startupStatus/updated", Disposition::Notice("info")),
    ("mcpServer/oauthLogin/completed", Disposition::Notice("info")),
    ("process/exited", Disposition::Notice("info")),
    ("thread/environment/connected", Disposition::Notice("info")),
    ("thread/environment/disconnected", Disposition::Notice("info")),
    ("thread/settings/updated", Disposition::Notice("info")),
    ("thread/goal/updated", Disposition::Notice("info")),
    ("thread/goal/cleared", Disposition::Notice("info")),
    ("thread/project/updated", Disposition::Notice("info")),
    ("project/changed", Disposition::Notice("info")),
    ("thread/started", Disposition::Notice("info")),
    ("thread/archived", Disposition::Notice("info")),
    ("thread/unarchived", Disposition::Notice("info")),
    ("thread/deleted", Disposition::Notice("info")),
    ("thread/closed", Disposition::Notice("info")),
    ("thread/reverted", Disposition::Notice("info")),
    ("hook/started", Disposition::Notice("info")),
    ("hook/completed", Disposition::Notice("info")),
    ("item/autoApprovalReview/started", Disposition::Notice("info")),
    ("item/autoApprovalReview/completed", Disposition::Notice("info")),
    ("account/updated", Disposition::Notice("info")),
    ("account/login/completed", Disposition::Notice("info")),
    ("remoteControl/status/changed", Disposition::Notice("info")),
    ("externalAgentConfig/import/progress", Disposition::Notice("info")),
    ("externalAgentConfig/import/completed", Disposition::Notice("info")),
    ("windowsSandbox/setupCompleted", Disposition::Notice("info")),
];

/// Resolve a `ServerNotification` method to its [`Disposition`].
#[must_use]
pub fn disposition(method: &str) -> Disposition {
    NOTIFICATION_DISPOSITIONS
        .iter()
        .find(|(m, _)| *m == method)
        .map_or(Disposition::Unknown, |(_, d)| *d)
}

pub(super) fn map_notification(local_id: &str, method: &str, v: &Value) -> Incoming {
    match disposition(method) {
        Disposition::Mapped => match method {
            // Emit on `item/completed` only; `item/started` and `item/<kind>/delta`
            // are consumed by [`ItemAccumulator`] in the driver, not here.
            "item/completed" => map_item_completed(local_id, v),
            // Thread liveness/attention → Status (drives the dots + ✋).
            "thread/status/changed" => map_status(local_id, v),
            // Per-turn token usage → TokenUsage.
            "thread/tokenUsage/updated" => map_token_usage(local_id, v),
            // Thread rename → Status carrying just the name (display gated on).
            "thread/name/updated" => map_name(local_id, v),
            // Structured turn errors → failed Status.
            "error" => map_error_notification(local_id, v),
            "turn/completed" => map_turn_completed(local_id, v),
            "turn/plan/updated" => map_plan_updated(local_id, v),
            "thread/compacted" => map_compacted(local_id, v),
            "account/rateLimits/updated" => crate::adapters::codex::rate_limits::from_notification(
                local_id, v,
            )
            .map_or_else(
                || Incoming::Traced { method: method.to_owned(), reason: "no rate-limit windows" },
                Incoming::Event,
            ),
            _ => unreachable!("Disposition::Mapped without a mapper: {method}"),
        },
        Disposition::Stream(reason) => Incoming::Traced { method: method.to_owned(), reason },
        Disposition::Notice(level) => {
            Incoming::Event(notice_event(local_id, method, level, v.get("params")))
        }
        Disposition::Unknown => Incoming::Unhandled {
            method: method.to_owned(),
            event: notice_event(local_id, method, "unhandled", v.get("params")),
        },
    }
}

/// A notification with no dedicated rendering, carried into the timeline as a
/// `codexNotice` item. `cctui-server`'s codex normalizer turns it into a
/// notice line; without an entry there it would be dropped as an unknown item.
fn notice_event(local_id: &str, method: &str, level: &str, params: Option<&Value>) -> AdapterEvent {
    AdapterEvent::Message {
        local_id: local_id.to_owned(),
        payload: json!({
            "type": "codexNotice",
            "level": level,
            "method": method,
            "text": notice_text(method, params),
            "params": params.cloned().unwrap_or(Value::Null),
        }),
        turn_id: None,
    }
}

/// Best-effort human summary of a notice: the schema's `message` field where
/// there is one, else a compact rendering of the params.
fn notice_text(method: &str, params: Option<&Value>) -> String {
    let detail = params.and_then(|p| {
        for key in ["message", "reason", "name", "status", "state"] {
            if let Some(s) = p.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()) {
                return Some(s.to_owned());
            }
        }
        None
    });
    detail.map_or_else(|| method.to_owned(), |d| format!("{method}: {d}"))
}

/// Map `turn/plan/updated` → a `plan` item, the codex half of the agent task
/// list. Rendered as an assistant plan line by the existing `plan` normalizer.
/// Codex `ThreadStatus.type`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ThreadStatusType {
    Active,
    Idle,
    SystemError,
    NotLoaded,
    #[default]
    #[serde(other)]
    Unknown,
}

/// Codex `Turn.status`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::adapters::codex) enum TurnStatus {
    Completed,
    Interrupted,
    Failed,
    InProgress,
    #[default]
    #[serde(other)]
    Unknown,
}

/// Status of one `turn/plan/updated` step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
    #[default]
    #[serde(other)]
    Unknown,
}

/// Parse a codex status value; a non-string reads as the `Unknown` variant.
pub(in crate::adapters::codex) fn parse_status<T: for<'de> Deserialize<'de> + Default>(
    v: &Value,
) -> T {
    T::deserialize(v).unwrap_or_default()
}

fn map_plan_updated(local_id: &str, v: &Value) -> Incoming {
    let Some(steps) = v.pointer("/params/plan").and_then(Value::as_array) else {
        return Incoming::Traced { method: "turn/plan/updated".to_owned(), reason: "no plan" };
    };
    let rendered = steps
        .iter()
        .map(|s| {
            let text = s.get("step").and_then(Value::as_str).unwrap_or_default();
            let mark = match s.get("status").map_or(PlanStepStatus::Unknown, parse_status) {
                PlanStepStatus::Completed => "x",
                PlanStepStatus::InProgress => "~",
                PlanStepStatus::Pending | PlanStepStatus::Unknown => " ",
            };
            format!("- [{mark}] {text}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let explanation = v.pointer("/params/explanation").and_then(Value::as_str);
    let text = explanation
        .filter(|e| !e.is_empty())
        .map_or_else(|| rendered.clone(), |e| format!("{e}\n\n{rendered}"));
    Incoming::Event(AdapterEvent::Message {
        local_id: local_id.to_owned(),
        payload: json!({"type": "plan", "text": text, "plan": steps}),
        turn_id: None,
    })
}

/// Map `thread/compacted` → a `contextCompaction` item: the context-reset
/// boundary the claude adapter already emits on `/clear` + `/compact`.
fn map_compacted(local_id: &str, v: &Value) -> Incoming {
    Incoming::Event(AdapterEvent::Message {
        local_id: local_id.to_owned(),
        payload: json!({
            "type": "contextCompaction",
            "text": v
                .pointer("/params/summary")
                .and_then(Value::as_str)
                .unwrap_or("context compacted"),
        }),
        turn_id: None,
    })
}

/// Map the structured `error` notification → [`AdapterEvent::Status`].
/// `willRetry: true` means codex is retrying the turn itself, so only the
/// detail is surfaced; a non-retried error marks the session failed.
fn map_error_notification(local_id: &str, v: &Value) -> Incoming {
    let Some(message) = v.pointer("/params/error/message").and_then(Value::as_str) else {
        return Incoming::Traced { method: "error".to_owned(), reason: "no error message" };
    };
    let will_retry = v.pointer("/params/willRetry").and_then(Value::as_bool).unwrap_or(false);
    let (state, activity) =
        if will_retry { (None, None) } else { (Some("failed"), Some("failure")) };
    Incoming::Event(AdapterEvent::Status {
        local_id: local_id.to_owned(),
        tempo: None,
        state: state.map(str::to_owned),
        detail: Some(message.to_owned()),
        activity: activity.map(str::to_owned),
        name: None,
        intent: None,
        model: None,
        effort: None,
        permission_mode: None,
        children: vec![],
    })
}

/// Map `turn/completed` whose `turn.status == "failed"` → failed
/// [`AdapterEvent::Status`] carrying the turn error message.
/// Successful turns stay ignored: idle status arrives via
/// `thread/status/changed`.
fn map_turn_completed(local_id: &str, v: &Value) -> Incoming {
    if v.pointer("/params/turn/status").map_or(TurnStatus::Unknown, parse_status)
        != TurnStatus::Failed
    {
        return Incoming::Traced { method: "turn/completed".to_owned(), reason: "turn not failed" };
    }
    let detail = v
        .pointer("/params/turn/error/message")
        .and_then(Value::as_str)
        .unwrap_or("turn failed")
        .to_owned();
    Incoming::Event(AdapterEvent::Status {
        local_id: local_id.to_owned(),
        tempo: None,
        state: Some("failed".to_owned()),
        detail: Some(detail),
        activity: Some("failure".to_owned()),
        name: None,
        intent: None,
        model: None,
        effort: None,
        permission_mode: None,
        children: vec![],
    })
}

fn map_item_completed(local_id: &str, v: &Value) -> Incoming {
    let Some(item) = v.pointer("/params/item") else {
        return Incoming::Traced { method: "item/completed".to_owned(), reason: "no item" };
    };
    Incoming::Event(item_event(local_id, item))
}

/// Split one codex `ThreadItem` onto `ToolUse` vs `Message`. Shared by the
/// live `item/completed` path and [`crate::adapters::codex::thread_read`]'s replayed history so
/// both render through one mapping.
#[must_use]
pub fn item_event(local_id: &str, item: &Value) -> AdapterEvent {
    let payload = item.clone();
    match item.get("type").and_then(Value::as_str).unwrap_or("") {
        "commandExecution"
        | "fileChange"
        | "mcpToolCall"
        | "dynamicToolCall"
        | "collabAgentToolCall"
        | "webSearch"
        | "imageView"
        | "imageGeneration" => AdapterEvent::ToolUse { local_id: local_id.to_owned(), payload },
        _ => AdapterEvent::Message { local_id: local_id.to_owned(), payload, turn_id: None },
    }
}

/// Map `thread/status/changed` → [`AdapterEvent::Status`]. The codex
/// `ThreadStatus` (`active` / `idle` / `systemError`) plus `activeFlags`
/// (`waitingOnApproval` / `waitingOnUserInput`) project onto the same
/// `tempo`/`state`/`activity` the classifier consumes: a waiting flag means
/// `tempo = "blocked"` (the ✋ "needs input" signal).
fn map_status(local_id: &str, v: &Value) -> Incoming {
    let Some(status) = v.pointer("/params/status") else {
        return Incoming::Traced {
            method: "thread/status/changed".to_owned(),
            reason: "no status",
        };
    };
    let ty = status.get("type").map_or(ThreadStatusType::Unknown, parse_status);
    let waiting = status.get("activeFlags").and_then(Value::as_array).is_some_and(|flags| {
        flags
            .iter()
            .filter_map(Value::as_str)
            .any(|f| f == "waitingOnApproval" || f == "waitingOnUserInput")
    });
    let (tempo, state, activity) = match ty {
        ThreadStatusType::Active if waiting => (Some("blocked"), Some("working"), None),
        ThreadStatusType::Active => (Some("active"), Some("working"), None),
        ThreadStatusType::Idle => (None, Some("idle"), None),
        ThreadStatusType::SystemError => (None, Some("failed"), Some("failure")),
        ThreadStatusType::NotLoaded | ThreadStatusType::Unknown => {
            return Incoming::Traced {
                method: "thread/status/changed".to_owned(),
                reason: "status not actionable",
            };
        }
    };
    Incoming::Event(AdapterEvent::Status {
        local_id: local_id.to_owned(),
        tempo: tempo.map(str::to_owned),
        state: state.map(str::to_owned),
        detail: None,
        activity: activity.map(str::to_owned),
        name: None,
        intent: None,
        model: None,
        effort: None,
        permission_mode: None,
        children: vec![],
    })
}

/// Map `thread/tokenUsage/updated` → [`AdapterEvent::TokenUsage`], keyed by
/// `turnId` so the server's per-message aggregation sums turns into the
/// thread total. `inputTokens` includes cached input; subtract it so the
/// non-cached/cached split matches the claude adapter's semantics.
fn map_token_usage(local_id: &str, v: &Value) -> Incoming {
    let turn_id = v.pointer("/params/turnId").and_then(Value::as_str).unwrap_or("");
    if turn_id.is_empty() {
        return Incoming::Traced {
            method: "thread/tokenUsage/updated".to_owned(),
            reason: "no turn id",
        };
    }
    let Some(last) = v.pointer("/params/tokenUsage/last") else {
        return Incoming::Traced {
            method: "thread/tokenUsage/updated".to_owned(),
            reason: "no last usage",
        };
    };
    let g = |k: &str| last.get(k).and_then(Value::as_u64).unwrap_or(0);
    let cached = g("cachedInputTokens");
    Incoming::Event(AdapterEvent::TokenUsage {
        local_id: local_id.to_owned(),
        message_id: turn_id.to_owned(),
        input_tokens: g("inputTokens").saturating_sub(cached),
        output_tokens: g("outputTokens"),
        cache_read_tokens: cached,
        cache_creation_tokens: 0,
    })
}

/// Map `thread/name/updated` → [`AdapterEvent::Status`] carrying just the
/// name. Adapter-level parity with claude; the web display of the name is
/// tracked separately.
fn map_name(local_id: &str, v: &Value) -> Incoming {
    let Some(name) = v.pointer("/params/name").and_then(Value::as_str) else {
        return Incoming::Traced { method: "thread/name/updated".to_owned(), reason: "no name" };
    };
    Incoming::Event(AdapterEvent::Status {
        local_id: local_id.to_owned(),
        tempo: None,
        state: None,
        detail: None,
        activity: None,
        name: Some(name.to_owned()),
        intent: None,
        model: None,
        effort: None,
        permission_mode: None,
        children: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::super::rpc::classify;
    use super::*;

    #[test]
    fn codex_status_fixtures_parse_into_enums() {
        let thread = |raw: Value| parse_status::<ThreadStatusType>(&raw);
        assert_eq!(thread(json!("active")), ThreadStatusType::Active);
        assert_eq!(thread(json!("idle")), ThreadStatusType::Idle);
        assert_eq!(thread(json!("systemError")), ThreadStatusType::SystemError);
        assert_eq!(thread(json!("notLoaded")), ThreadStatusType::NotLoaded);
        assert_eq!(thread(json!("somethingNew")), ThreadStatusType::Unknown);
        assert_eq!(thread(json!(3)), ThreadStatusType::Unknown);

        let turn = |raw: Value| parse_status::<TurnStatus>(&raw);
        assert_eq!(turn(json!("completed")), TurnStatus::Completed);
        assert_eq!(turn(json!("interrupted")), TurnStatus::Interrupted);
        assert_eq!(turn(json!("failed")), TurnStatus::Failed);
        assert_eq!(turn(json!("inProgress")), TurnStatus::InProgress);
        assert_eq!(turn(json!("in_progress")), TurnStatus::Unknown);

        let step = |raw: Value| parse_status::<PlanStepStatus>(&raw);
        assert_eq!(step(json!("pending")), PlanStepStatus::Pending);
        assert_eq!(step(json!("in_progress")), PlanStepStatus::InProgress);
        assert_eq!(step(json!("completed")), PlanStepStatus::Completed);
        assert_eq!(step(json!("skipped")), PlanStepStatus::Unknown);
    }

    #[test]
    fn command_execution_item_maps_to_tool_use() {
        let v = json!({
            "method": "item/completed",
            "params": {"item": {"type": "commandExecution", "command": "ls", "status": "completed"}},
        });
        match classify("sess", &v) {
            Incoming::Event(AdapterEvent::ToolUse { local_id, .. }) => assert_eq!(local_id, "sess"),
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn agent_message_item_maps_to_message() {
        let v = json!({
            "method": "item/completed",
            "params": {"item": {"type": "agentMessage", "text": "done"}},
        });
        match classify("sess", &v) {
            Incoming::Event(AdapterEvent::Message { local_id, .. }) => assert_eq!(local_id, "sess"),
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn item_started_is_ignored_to_avoid_duplicates() {
        let v = json!({
            "method": "item/started",
            "params": {"item": {"type": "commandExecution", "command": "ls"}},
        });
        assert!(matches!(classify("sess", &v), Incoming::Traced { .. }));
    }

    #[test]
    fn turn_lifecycle_is_traced_not_dropped() {
        assert!(matches!(
            classify("s", &json!({"method": "turn/completed", "params": {}})),
            Incoming::Traced { .. }
        ));
        match classify("s", &json!({"method": "thread/started", "params": {}})) {
            Incoming::Event(AdapterEvent::Message { payload, .. }) => {
                assert_eq!(payload["type"], "codexNotice");
            }
            other => panic!("expected a notice, got {other:?}"),
        }
    }

    /// The vendored protocol schema is the source of truth for what codex can
    /// send. Every `ServerNotification` method in it must resolve to a real
    /// disposition — a re-vendored schema with a new method fails here instead
    /// of silently falling through at runtime.
    #[test]
    fn every_schema_notification_has_a_disposition() {
        let raw = include_str!("../schema/codex_app_server_protocol.schemas.json");
        let schema: Value = serde_json::from_str(raw).expect("schema parses");
        let variants = schema
            .pointer("/definitions/ServerNotification/oneOf")
            .and_then(Value::as_array)
            .expect("ServerNotification oneOf");
        let methods: Vec<&str> = variants
            .iter()
            .filter_map(|v| v.pointer("/properties/method/enum/0").and_then(Value::as_str))
            .collect();
        assert_eq!(methods.len(), variants.len(), "every variant pins one method");
        assert!(methods.len() >= 81, "schema shrank unexpectedly: {} methods", methods.len());

        let missing: Vec<&str> =
            methods.iter().copied().filter(|m| disposition(m) == Disposition::Unknown).collect();
        assert!(missing.is_empty(), "codex notifications with no disposition: {missing:?}");
    }

    /// The reverse guard: nothing in the table has been invented or left
    /// behind by a protocol removal.
    #[test]
    fn disposition_table_has_no_methods_the_schema_lacks() {
        let raw = include_str!("../schema/codex_app_server_protocol.schemas.json");
        let schema: Value = serde_json::from_str(raw).expect("schema parses");
        let known: std::collections::HashSet<String> = schema
            .pointer("/definitions/ServerNotification/oneOf")
            .and_then(Value::as_array)
            .expect("ServerNotification oneOf")
            .iter()
            .filter_map(|v| {
                v.pointer("/properties/method/enum/0").and_then(Value::as_str).map(str::to_owned)
            })
            .collect();
        let stale: Vec<&str> = NOTIFICATION_DISPOSITIONS
            .iter()
            .map(|(m, _)| *m)
            .filter(|m| !known.contains(*m))
            .collect();
        assert!(stale.is_empty(), "table lists methods the schema does not: {stale:?}");
        let uncovered: Vec<&String> = known
            .iter()
            .filter(|m| !NOTIFICATION_DISPOSITIONS.iter().any(|(t, _)| t == m))
            .collect();
        assert!(uncovered.is_empty(), "schema methods absent from the table: {uncovered:?}");
        assert_eq!(NOTIFICATION_DISPOSITIONS.len(), known.len(), "one entry per method");
    }

    #[test]
    fn unknown_method_is_unhandled_not_dropped() {
        let v = json!({"method": "future/thing", "params": {"message": "hi"}});
        match classify("s", &v) {
            Incoming::Unhandled { method, event } => {
                assert_eq!(method, "future/thing");
                let AdapterEvent::Message { payload, .. } = event else { panic!("want Message") };
                assert_eq!(payload["level"], "unhandled");
                assert_eq!(payload["text"], "future/thing: hi");
            }
            other => panic!("expected Unhandled, got {other:?}"),
        }
    }

    #[test]
    fn warning_family_surfaces_as_warning_notices() {
        for method in
            ["warning", "guardianWarning", "configWarning", "deprecationNotice", "model/rerouted"]
        {
            let v = json!({"method": method, "params": {"message": "careful"}});
            match classify("s", &v) {
                Incoming::Event(AdapterEvent::Message { payload, .. }) => {
                    assert_eq!(payload["type"], "codexNotice");
                    assert_eq!(payload["method"], method);
                }
                other => panic!("{method}: expected a notice, got {other:?}"),
            }
        }
        let warn = json!({"method": "warning", "params": {"message": "m"}});
        let Incoming::Event(AdapterEvent::Message { payload, .. }) = classify("s", &warn) else {
            panic!("want Message")
        };
        assert_eq!(payload["level"], "warning");
    }

    #[test]
    fn plan_updated_renders_a_checklist() {
        let v = json!({"method": "turn/plan/updated", "params": {
            "threadId": "t", "turnId": "u", "explanation": "why",
            "plan": [
                {"step": "one", "status": "completed"},
                {"step": "two", "status": "in_progress"},
                {"step": "three", "status": "pending"},
            ],
        }});
        let Incoming::Event(AdapterEvent::Message { payload, .. }) = classify("s", &v) else {
            panic!("want Message")
        };
        assert_eq!(payload["type"], "plan");
        let text = payload["text"].as_str().unwrap();
        assert!(text.starts_with("why\n\n"), "{text}");
        assert!(text.contains("- [x] one"), "{text}");
        assert!(text.contains("- [~] two"), "{text}");
        assert!(text.contains("- [ ] three"), "{text}");
    }

    #[test]
    fn compacted_emits_a_context_reset_marker() {
        let v = json!({"method": "thread/compacted", "params": {"threadId": "t", "summary": "s"}});
        let Incoming::Event(AdapterEvent::Message { payload, .. }) = classify("s", &v) else {
            panic!("want Message")
        };
        assert_eq!(payload["type"], "contextCompaction");
        assert_eq!(payload["text"], "s");
    }

    // --- v2 notification mapping (codex-cli 0.135 wire payloads) ----------

    #[test]
    fn status_active_with_waiting_flag_is_blocked() {
        let v = json!({"method":"thread/status/changed","params":{
            "threadId":"t","status":{"type":"active","activeFlags":["waitingOnApproval"]}}});
        match classify("t", &v) {
            Incoming::Event(AdapterEvent::Status { tempo, .. }) => {
                assert_eq!(tempo.as_deref(), Some("blocked"));
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn status_active_no_flags_is_active_idle_is_idle() {
        let active = json!({"method":"thread/status/changed","params":{
            "status":{"type":"active","activeFlags":[]}}});
        let Incoming::Event(AdapterEvent::Status { tempo, .. }) = classify("t", &active) else {
            panic!("expected Status")
        };
        assert_eq!(tempo.as_deref(), Some("active"));

        let idle = json!({"method":"thread/status/changed","params":{"status":{"type":"idle"}}});
        let Incoming::Event(AdapterEvent::Status { tempo, state, .. }) = classify("t", &idle)
        else {
            panic!("expected Status")
        };
        assert_eq!(tempo, None);
        assert_eq!(state.as_deref(), Some("idle"));
    }

    #[test]
    fn waiting_status_classifies_as_needs_input() {
        use cctui_proto::classifier::{Bucket, ClassifyInput, PrStatus, classify as bucket_of};
        let v = json!({"method":"thread/status/changed","params":{
            "status":{"type":"active","activeFlags":["waitingOnUserInput"]}}});
        let Incoming::Event(AdapterEvent::Status { tempo, state, activity, .. }) =
            classify("t", &v)
        else {
            panic!("expected Status")
        };
        let input = ClassifyInput {
            tempo: tempo.as_deref(),
            state: state.as_deref(),
            activity: activity.as_deref(),
            children: &[],
            q: None,
            soft_limit_blocked: None,
        };
        let empty: std::collections::HashMap<String, PrStatus> = std::collections::HashMap::new();
        assert_eq!(bucket_of(&input, &empty), Bucket::Blocked);
    }

    #[test]
    fn token_usage_maps_last_keyed_by_turn() {
        let v = json!({"method":"thread/tokenUsage/updated","params":{
            "turnId":"turn-1",
            "tokenUsage":{"last":{"totalTokens":11617,"inputTokens":11592,
                "cachedInputTokens":9600,"outputTokens":25}}}});
        match classify("t", &v) {
            Incoming::Event(AdapterEvent::TokenUsage {
                message_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                ..
            }) => {
                assert_eq!(message_id, "turn-1");
                assert_eq!(input_tokens, 11592 - 9600); // non-cached input
                assert_eq!(output_tokens, 25);
                assert_eq!(cache_read_tokens, 9600);
            }
            other => panic!("expected TokenUsage, got {other:?}"),
        }
    }

    #[test]
    fn token_usage_without_turn_id_emits_nothing() {
        let v = json!({"method":"thread/tokenUsage/updated","params":{
            "tokenUsage":{"last":{"inputTokens":1}}}});
        assert!(matches!(classify("t", &v), Incoming::Traced { .. }));
    }

    #[test]
    fn thread_name_maps_to_status_name() {
        let v = json!({"method":"thread/name/updated","params":{"name":"my-thread"}});
        match classify("t", &v) {
            Incoming::Event(AdapterEvent::Status { name, .. }) => {
                assert_eq!(name.as_deref(), Some("my-thread"));
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn error_notification_without_retry_maps_to_failed_status() {
        let v = json!({"method": "error", "params": {
            "threadId": "t", "turnId": "u", "willRetry": false,
            "error": {"message": "usage limit exceeded", "codexErrorInfo": "usageLimitExceeded"}}});
        match classify("t", &v) {
            Incoming::Event(AdapterEvent::Status { state, detail, activity, .. }) => {
                assert_eq!(state.as_deref(), Some("failed"));
                assert_eq!(detail.as_deref(), Some("usage limit exceeded"));
                assert_eq!(activity.as_deref(), Some("failure"));
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn error_notification_with_retry_surfaces_detail_only() {
        let v = json!({"method": "error", "params": {
            "threadId": "t", "turnId": "u", "willRetry": true,
            "error": {"message": "server overloaded"}}});
        match classify("t", &v) {
            Incoming::Event(AdapterEvent::Status { state, detail, activity, .. }) => {
                assert_eq!(state, None);
                assert_eq!(activity, None);
                assert_eq!(detail.as_deref(), Some("server overloaded"));
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn failed_turn_completed_maps_to_failed_status() {
        let v = json!({"method": "turn/completed", "params": {"threadId": "t", "turn": {
            "id": "u", "items": [], "status": "failed",
            "error": {"message": "context window exceeded"}}}});
        match classify("t", &v) {
            Incoming::Event(AdapterEvent::Status { state, detail, .. }) => {
                assert_eq!(state.as_deref(), Some("failed"));
                assert_eq!(detail.as_deref(), Some("context window exceeded"));
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn successful_turn_completed_emits_no_status() {
        let v = json!({"method": "turn/completed", "params": {"threadId": "t", "turn": {
            "id": "u", "items": [], "status": "completed"}}});
        assert!(matches!(classify("t", &v), Incoming::Traced { .. }));
    }

    #[test]
    fn new_tool_items_classify_as_tool_use() {
        for ty in
            ["dynamicToolCall", "collabAgentToolCall", "webSearch", "imageView", "imageGeneration"]
        {
            let v = json!({"method":"item/completed","params":{"item":{"type":ty,"id":"x"}}});
            assert!(
                matches!(classify("s", &v), Incoming::Event(AdapterEvent::ToolUse { .. })),
                "{ty} should be a ToolUse",
            );
        }
    }
}
