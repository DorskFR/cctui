//! One-shot dispatch by adapter.
//!
//! A dispatched worker's payload names the harness in `adapter`. Claude runs
//! through the claude control socket and is observed like any session; every
//! other harness runs one fire-and-report turn here and writes the verdict
//! to `RESULT_FILE`, the envelope the entrypoint's callback trap posts. An
//! adapter the table does not know is an error, never a claude fallback.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::Context as _;
use cctui_proto::adapter::{AdapterEvent, EndReason};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::adapters::acp::rows::{self, AgentRow};

/// Default result-file path the worker entrypoint's callback trap reads
/// (`RESULT_FILE`, matching `deploy/worker-entrypoint.sh`).
pub const DEFAULT_RESULT_FILE: &str = "/tmp/cctui-result.json";

/// Hard wall-clock bound of one dispatched turn when the env sets none.
const DEFAULT_TIMEOUT_SECS: u64 = 10_800;

/// The harness a dispatch payload selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchAdapter {
    ClaudeCode,
    Codex,
    OpenCode,
    Acp(&'static AgentRow),
}

impl DispatchAdapter {
    /// The adapter an `adapter` payload value names.
    ///
    /// # Errors
    /// A name outside the harness table and the ACP agent rows.
    pub fn select(adapter: &str) -> anyhow::Result<Self> {
        let id = adapter.trim();
        match id {
            "" | "claude" | "claude-code" | "claude_code" => Ok(Self::ClaudeCode),
            "codex" | "codex-cli" => Ok(Self::Codex),
            "opencode" => Ok(Self::OpenCode),
            other => rows::row(other).map(Self::Acp).ok_or_else(|| {
                anyhow::anyhow!(
                    "dispatch payload names unknown adapter `{other}` (known: {})",
                    known_adapters().join(", ")
                )
            }),
        }
    }

    /// The adapter a whole payload selects; no `adapter` key is claude.
    ///
    /// # Errors
    /// See [`Self::select`].
    pub fn from_payload(payload: &Value) -> anyhow::Result<Self> {
        Self::select(&payload_adapter(payload))
    }

    /// The id the server and the result envelope know the adapter by.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Acp(row) => row.id,
        }
    }

    /// Whether the adapter runs as one turn here rather than through the
    /// claude control socket.
    #[must_use]
    pub const fn is_one_shot(self) -> bool {
        !matches!(self, Self::ClaudeCode)
    }
}

fn known_adapters() -> Vec<&'static str> {
    let mut out = vec!["claude-code", "codex", "opencode"];
    out.extend(rows::ROWS.iter().map(|r| r.id));
    out
}

/// The adapter string a dispatch payload selects, defaulting to `claude-code`
/// so a payload with no `adapter` key is backward-compatible.
#[must_use]
pub fn payload_adapter(payload: &Value) -> String {
    payload
        .get("adapter")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("claude-code")
        .to_owned()
}

/// Where the verdict goes: `RESULT_FILE`, else the entrypoint's default.
#[must_use]
pub fn result_file() -> String {
    std::env::var("RESULT_FILE").unwrap_or_else(|_| DEFAULT_RESULT_FILE.to_owned())
}

/// `payload.task_id`, else the `TASK_ID` env, else empty.
#[must_use]
pub fn task_id(payload: &Value) -> String {
    payload
        .get("task_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var("TASK_ID").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

/// The wall-clock bound of one dispatched turn: `CODEX_TIMEOUT` for codex,
/// `CCTUI_DISPATCH_TIMEOUT` otherwise, default 3h. The job's
/// `activeDeadlineSeconds` is the outer bound.
#[must_use]
pub fn timeout_secs(adapter: DispatchAdapter) -> u64 {
    let var = match adapter {
        DispatchAdapter::Codex => "CODEX_TIMEOUT",
        _ => "CCTUI_DISPATCH_TIMEOUT",
    };
    std::env::var(var)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
}

/// The distilled result of one dispatched turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// `success` | `failed`, matching the result-callback contract.
    pub status: String,
    /// One-line reason when `status != success`.
    pub error: Option<String>,
    /// The final assistant message text.
    pub message: String,
    /// The ACP `stopReason` or an equivalent, when the harness reports one.
    pub stop_reason: Option<String>,
    /// Token counts, when the harness reports them.
    pub usage: Option<Value>,
}

impl Outcome {
    #[must_use]
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            status: "failed".to_owned(),
            error: Some(reason.into()),
            message: String::new(),
            stop_reason: None,
            usage: None,
        }
    }

    #[must_use]
    pub fn success(message: String) -> Self {
        Self { status: "success".to_owned(), error: None, message, stop_reason: None, usage: None }
    }
}

/// The `RESULT_FILE` verdict envelope: `task_id`, `flow`, `status`, `error`,
/// plus the final `message`, the `stop_reason` and token `usage` when known.
#[must_use]
pub fn result_json(task_id: &str, flow: &str, outcome: &Outcome) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("task_id".to_owned(), json!(task_id));
    obj.insert("flow".to_owned(), json!(flow));
    obj.insert("status".to_owned(), json!(outcome.status));
    obj.insert("error".to_owned(), outcome.error.as_ref().map_or(Value::Null, |e| json!(e)));
    if !outcome.message.is_empty() {
        obj.insert("message".to_owned(), json!(outcome.message));
    }
    if let Some(reason) = &outcome.stop_reason {
        obj.insert("stop_reason".to_owned(), json!(reason));
    }
    if let Some(usage) = &outcome.usage {
        obj.insert("usage".to_owned(), usage.clone());
    }
    Value::Object(obj)
}

/// Write the verdict where the entrypoint's callback trap reads it.
///
/// # Errors
/// The file could not be written.
pub fn write_result(
    path: &str,
    task_id: &str,
    flow: &str,
    outcome: &Outcome,
) -> anyhow::Result<()> {
    let body = serde_json::to_string(&result_json(task_id, flow, outcome))
        .unwrap_or_else(|_| "{}".to_owned());
    std::fs::write(path, body).with_context(|| format!("writing result file {path}"))?;
    tracing::info!(task_id, flow, status = %outcome.status, "dispatch: wrote {path}");
    Ok(())
}

/// Fail the dispatch before any harness runs, so the pod ends on the verdict
/// instead of waiting out its boot deadline.
pub fn write_failure(payload: &Value, flow: &str, reason: &str) {
    if let Err(err) =
        write_result(&result_file(), &task_id(payload), flow, &Outcome::failed(reason))
    {
        tracing::error!(%err, "dispatch: could not write the failure verdict");
    }
}

/// The supervisor's adapter-tagged event lane, so a one-shot session's
/// transcript reaches the server like any adapter's.
pub type EventSink = mpsc::Sender<(String, AdapterEvent)>;

static EVENT_SINK: OnceLock<Mutex<Option<EventSink>>> = OnceLock::new();

fn sink_slot() -> &'static Mutex<Option<EventSink>> {
    EVENT_SINK.get_or_init(|| Mutex::new(None))
}

/// Register the lane; the last registration wins.
pub fn register_event_sink(sink: EventSink) {
    if let Ok(mut slot) = sink_slot().lock() {
        *slot = Some(sink);
    }
}

#[must_use]
pub fn event_sink() -> Option<EventSink> {
    sink_slot().lock().ok().and_then(|slot| slot.clone())
}

/// Reduce one session's event stream to an [`Outcome`], forwarding every
/// event to `sink` under `adapter_id` on the way.
///
/// The stream ends on `SessionEnded`: `Completed` is a success, any other
/// reason a failure carrying its detail. A status of `error` before the end
/// is the failure reason; the stop reason comes from the idle status detail.
pub async fn collect(
    adapter_id: &str,
    mut events: mpsc::Receiver<AdapterEvent>,
    sink: Option<EventSink>,
    timeout: Duration,
) -> Outcome {
    let mut acc = Reducer::default();
    let reduce = async {
        while let Some(evt) = events.recv().await {
            let ended = acc.observe(&evt);
            if let Some(sink) = &sink
                && sink.send((adapter_id.to_owned(), evt)).await.is_err()
            {
                tracing::warn!(adapter_id, "dispatch: event lane closed, transcript not forwarded");
            }
            if ended {
                return;
            }
        }
        acc.closed();
    };
    if tokio::time::timeout(timeout, reduce).await.is_err() {
        return Outcome::failed(format!(
            "{adapter_id} dispatch exceeded {}s timeout",
            timeout.as_secs()
        ));
    }
    acc.outcome()
}

/// What the event stream has said so far.
#[derive(Debug, Default)]
pub struct Reducer {
    message: String,
    stop_reason: Option<String>,
    usage: Option<Value>,
    error: Option<String>,
    end: Option<EndReason>,
}

impl Reducer {
    /// Feed one event; `true` once the session has ended.
    pub fn observe(&mut self, evt: &AdapterEvent) -> bool {
        match evt {
            AdapterEvent::Message { payload, .. } => {
                if payload.get("role").and_then(Value::as_str) == Some("assistant")
                    && payload.get("meta").and_then(Value::as_bool) != Some(true)
                    && let Some(text) = message_text(payload)
                {
                    text.clone_into(&mut self.message);
                }
            }
            AdapterEvent::Status { tempo: Some(tempo), detail, .. } => match tempo.as_str() {
                "idle" => self.stop_reason = Some(stop_reason_of(detail.as_deref())),
                "error" => {
                    self.error = Some(detail.clone().unwrap_or_else(|| "turn failed".to_owned()))
                }
                _ => {}
            },
            AdapterEvent::TokenUsage {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                ..
            } => {
                self.usage = Some(json!({
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens,
                    "cached_input_tokens": cache_read_tokens,
                    "cache_creation_tokens": cache_creation_tokens,
                }));
            }
            AdapterEvent::SessionEnded { reason, .. } => {
                self.end = Some(reason.clone());
                return true;
            }
            _ => {}
        }
        false
    }

    /// The stream closed without a `SessionEnded`.
    pub fn closed(&mut self) {
        if self.end.is_none() {
            self.end = Some(EndReason::Crashed {
                detail: "the session ended without a verdict".to_owned(),
            });
        }
    }

    #[must_use]
    pub fn outcome(self) -> Outcome {
        let Self { message, stop_reason, usage, error, end } = self;
        let failure = match end {
            Some(EndReason::Completed) | None => error,
            Some(EndReason::Killed) => Some("the session was killed".to_owned()),
            Some(
                EndReason::Crashed { detail }
                | EndReason::ResumeFailed { detail }
                | EndReason::SpawnFailed { detail }
                | EndReason::Other { detail },
            ) => Some(error.unwrap_or(detail)),
        };
        let mut outcome = failure.map_or_else(|| Outcome::success(String::new()), Outcome::failed);
        outcome.message = message;
        outcome.stop_reason = stop_reason;
        outcome.usage = usage;
        outcome
    }
}

fn message_text(payload: &Value) -> Option<&str> {
    ["content", "text"]
        .iter()
        .find_map(|k| payload.get(k).and_then(Value::as_str))
        .filter(|t| !t.trim().is_empty())
}

/// The ACP stop reason behind an idle status: no detail is `end_turn`, a
/// `turn stopped: <reason>` detail is that reason, `turn interrupted` is
/// `cancelled` (the inverse of `normalize::stop_detail`).
fn stop_reason_of(detail: Option<&str>) -> String {
    match detail {
        None => "end_turn".to_owned(),
        Some("turn interrupted") => "cancelled".to_owned(),
        Some("turn stopped: token limit reached") => "max_tokens".to_owned(),
        Some("turn stopped: request limit reached") => "max_turn_requests".to_owned(),
        Some("turn stopped: the model refused") => "refusal".to_owned(),
        Some(other) => other.strip_prefix("turn stopped: ").unwrap_or(other).to_owned(),
    }
}

/// Run one dispatched turn on a one-shot adapter and write its verdict.
///
/// # Errors
/// The verdict could not be written; every harness failure is still a
/// written `failed` verdict.
pub async fn run_one_shot(adapter: DispatchAdapter, payload: &Value) -> anyhow::Result<()> {
    let flow = format!("{}-dispatch", adapter.id());
    let outcome = match adapter {
        DispatchAdapter::ClaudeCode => {
            anyhow::bail!("claude-code is dispatched through the control socket, not one-shot")
        }
        DispatchAdapter::Codex => {
            let dispatch = crate::dispatch_codex::CodexDispatch::from_payload(payload)?;
            return dispatch.run().await;
        }
        DispatchAdapter::OpenCode => {
            match crate::adapters::opencode::oneshot::OneShot::from_payload(payload) {
                Ok(one) => one.run(event_sink()).await,
                Err(err) => Outcome::failed(err.to_string()),
            }
        }
        DispatchAdapter::Acp(row) => {
            match crate::adapters::acp::oneshot::OneShot::from_payload(row, payload) {
                Ok(one) => one.run(event_sink()).await,
                Err(err) => Outcome::failed(err.to_string()),
            }
        }
    };
    write_result(&result_file(), &task_id(payload), &flow, &outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_selection_covers_every_harness_and_rejects_the_rest() {
        assert_eq!(DispatchAdapter::select("").unwrap(), DispatchAdapter::ClaudeCode);
        assert_eq!(DispatchAdapter::select("claude").unwrap(), DispatchAdapter::ClaudeCode);
        assert_eq!(DispatchAdapter::select(" claude-code ").unwrap(), DispatchAdapter::ClaudeCode);
        assert_eq!(DispatchAdapter::select("codex").unwrap(), DispatchAdapter::Codex);
        assert_eq!(DispatchAdapter::select("codex-cli").unwrap(), DispatchAdapter::Codex);
        assert_eq!(DispatchAdapter::select("opencode").unwrap(), DispatchAdapter::OpenCode);
        for id in ["gemini", "qwen", "goose"] {
            let DispatchAdapter::Acp(row) = DispatchAdapter::select(id).unwrap() else {
                panic!("{id} is an ACP agent")
            };
            assert_eq!(row.id, id);
        }
        for bad in ["gpt", "claude-cli", "acp", "codex exec"] {
            let err = DispatchAdapter::select(bad).unwrap_err().to_string();
            assert!(err.contains(bad) && err.contains("unknown adapter"), "{err}");
        }
    }

    #[test]
    fn a_payload_without_an_adapter_is_claude_and_an_unknown_one_is_an_error() {
        assert_eq!(DispatchAdapter::from_payload(&json!({})).unwrap(), DispatchAdapter::ClaudeCode);
        assert_eq!(
            DispatchAdapter::from_payload(&json!({ "adapter": "" })).unwrap(),
            DispatchAdapter::ClaudeCode
        );
        assert_eq!(
            DispatchAdapter::from_payload(&json!({ "adapter": " codex " })).unwrap(),
            DispatchAdapter::Codex
        );
        assert!(DispatchAdapter::from_payload(&json!({ "adapter": "nope" })).is_err());
        assert!(!DispatchAdapter::ClaudeCode.is_one_shot());
        assert!(DispatchAdapter::select("goose").unwrap().is_one_shot());
        assert_eq!(DispatchAdapter::select("qwen").unwrap().id(), "qwen");
    }

    #[test]
    fn payload_adapter_defaults_to_claude() {
        assert_eq!(payload_adapter(&json!({})), "claude-code");
        assert_eq!(payload_adapter(&json!({"adapter": ""})), "claude-code");
        assert_eq!(payload_adapter(&json!({"adapter": "codex"})), "codex");
        assert_eq!(payload_adapter(&json!({"adapter": " codex "})), "codex");
    }

    fn status(tempo: &str, detail: Option<&str>) -> AdapterEvent {
        AdapterEvent::Status {
            local_id: "s".to_owned(),
            tempo: Some(tempo.to_owned()),
            state: Some(tempo.to_owned()),
            detail: detail.map(str::to_owned),
            activity: None,
            name: None,
            intent: None,
            model: None,
            effort: None,
            permission_mode: None,
            children: Vec::new(),
        }
    }

    fn message(role: &str, text: &str, meta: bool) -> AdapterEvent {
        AdapterEvent::Message {
            local_id: "s".to_owned(),
            payload: json!({ "type": "text", "role": role, "content": text, "meta": meta }),
            turn_id: None,
        }
    }

    fn ended(reason: EndReason) -> AdapterEvent {
        AdapterEvent::SessionEnded { local_id: "s".to_owned(), reason }
    }

    #[test]
    fn the_reducer_keeps_the_last_assistant_text_and_the_stop_reason() {
        let mut r = Reducer::default();
        assert!(!r.observe(&message("user", "hi", false)));
        assert!(!r.observe(&message("assistant", "first", false)));
        assert!(!r.observe(&message("assistant", "prompt failed: x", true)));
        assert!(!r.observe(&message("assistant", "final", false)));
        assert!(!r.observe(&status("active", None)));
        assert!(!r.observe(&status("idle", Some("turn stopped: token limit reached"))));
        assert!(!r.observe(&AdapterEvent::TokenUsage {
            local_id: "s".to_owned(),
            message_id: "m".to_owned(),
            input_tokens: 100,
            output_tokens: 30,
            cache_read_tokens: 10,
            cache_creation_tokens: 0,
        }));
        assert!(r.observe(&ended(EndReason::Completed)));
        let out = r.outcome();
        assert_eq!(out.status, "success");
        assert_eq!(out.message, "final");
        assert_eq!(out.stop_reason.as_deref(), Some("max_tokens"));
        assert_eq!(out.usage.unwrap()["input_tokens"], json!(100));
    }

    #[test]
    fn a_crash_or_an_error_status_is_a_failure_that_keeps_the_text() {
        let mut r = Reducer::default();
        r.observe(&message("assistant", "partial", false));
        assert!(r.observe(&ended(EndReason::Crashed { detail: "agent exited".to_owned() })));
        let out = r.outcome();
        assert_eq!(out.status, "failed");
        assert_eq!(out.error.as_deref(), Some("agent exited"));
        assert_eq!(out.message, "partial");

        let mut r = Reducer::default();
        r.observe(&status("error", Some("prompt failed: boom")));
        assert!(r.observe(&ended(EndReason::Completed)));
        assert_eq!(r.outcome().error.as_deref(), Some("prompt failed: boom"));

        let mut r = Reducer::default();
        r.observe(&status("idle", None));
        r.closed();
        let out = r.outcome();
        assert_eq!(out.status, "failed");
        assert_eq!(out.stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn stop_reasons_round_trip_the_idle_detail() {
        assert_eq!(stop_reason_of(None), "end_turn");
        assert_eq!(stop_reason_of(Some("turn interrupted")), "cancelled");
        assert_eq!(stop_reason_of(Some("turn stopped: the model refused")), "refusal");
        assert_eq!(stop_reason_of(Some("turn stopped: custom")), "custom");
    }

    #[test]
    fn result_json_shape() {
        let mut outcome = Outcome::success("done".to_owned());
        outcome.stop_reason = Some("end_turn".to_owned());
        let r = result_json("T-1", "gemini-dispatch", &outcome);
        assert_eq!(r["task_id"], json!("T-1"));
        assert_eq!(r["flow"], json!("gemini-dispatch"));
        assert_eq!(r["status"], json!("success"));
        assert_eq!(r["error"], Value::Null);
        assert_eq!(r["message"], json!("done"));
        assert_eq!(r["stop_reason"], json!("end_turn"));
        assert!(r.get("usage").is_none());

        let r = result_json("T-1", "qwen-dispatch", &Outcome::failed("boom"));
        assert_eq!(r["status"], json!("failed"));
        assert_eq!(r["error"], json!("boom"));
        assert!(r.get("message").is_none());
    }

    #[tokio::test]
    async fn collect_forwards_every_event_and_stops_on_the_end() {
        let (tx, rx) = mpsc::channel(8);
        let (sink, mut forwarded) = mpsc::channel(8);
        tx.send(message("assistant", "hello", false)).await.unwrap();
        tx.send(status("idle", None)).await.unwrap();
        tx.send(ended(EndReason::Completed)).await.unwrap();
        let out = collect("gemini", rx, Some(sink), Duration::from_secs(5)).await;
        assert_eq!(out.status, "success");
        assert_eq!(out.message, "hello");
        let mut seen = 0;
        while let Ok((id, _)) = forwarded.try_recv() {
            assert_eq!(id, "gemini");
            seen += 1;
        }
        assert_eq!(seen, 3);
    }

    #[tokio::test]
    async fn collect_times_out_a_silent_session() {
        let (_tx, rx) = mpsc::channel::<AdapterEvent>(1);
        let out = collect("goose", rx, None, Duration::from_millis(20)).await;
        assert_eq!(out.status, "failed");
        assert!(out.error.unwrap().contains("timeout"));
    }
}
