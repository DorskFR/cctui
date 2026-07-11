//! Codex `app-server` driver (CCT-98).
//!
//! Drives sessions that cctui *spawns*, as opposed to the log-tail
//! ([`super::log_tail`]) which passively observes sessions started outside
//! cctui (e.g. the Codex TUI). The two coexist: session identity is the
//! rollout id (`UUIDv7`), so an app-server-driven session and its on-disk
//! rollout file refer to the same `local_id`.
//!
//! `codex app-server` speaks newline-delimited JSON-RPC 2.0 over stdio
//! (stderr is logs). The handshake is `initialize` (declaring client
//! capabilities) → `initialized` notification → `thread/start { cwd }`
//! → `turn/start { threadId, input }`. The pinned/minimum supported Codex
//! version and the retained JSON Schema live in [`super::contract`]. A stale
//! cctui-owned thread is revived
//! with `thread/resume { threadId }` before the next `turn/start` (CCT-229).
//! Streaming arrives as id-less
//! notifications (`item/completed`, `turn/completed`, …); tool approvals
//! arrive as server→client *requests* (they carry both `method` and `id`)
//! that block until we reply with a `decision`.
//!
//! This module is split into a pure protocol layer (request builders +
//! [`classify`]) that is unit-tested with fixtures, and an async driver
//! ([`CodexSession`]) that owns the subprocess and pumps IO.

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cctui_proto::adapter::{AdapterEvent, EndReason, SessionMeta};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::contract;

/// Outbound request id seeds. The handshake uses fixed ids so the driver
/// can recognise the responses it is waiting for; everything after is
/// monotonic from [`Self::RUN_BASE`].
const ID_INITIALIZE: i64 = 1;
const ID_THREAD_START: i64 = 2;
const RUN_BASE: i64 = 100;

/// How many trailing `codex app-server` stderr lines to retain for crash
/// diagnostics. The app-server logs to stderr (CCT-98); when it dies
/// unexpectedly these lines are the only clue why, so they are folded into
/// the [`EndReason::Crashed`] detail instead of being discarded to
/// `/dev/null`.
const STDERR_RING: usize = 40;

const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// `thread/resume` of a long transcript can legitimately exceed the normal
/// RPC deadline, so handshake requests get a longer one.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// Pure protocol layer
// ---------------------------------------------------------------------------

/// Which `decision` vocabulary an approval reply must use. Codex uses two
/// distinct enums depending on the approval method (verified against the
/// app-server JSON schema, codex-cli 0.134):
///
/// - command-execution and file-change approvals →
///   `"accept"` / `"decline"`.
/// - apply-patch and exec-command approvals (legacy `ReviewDecision`) →
///   `"approved"` / `"denied"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalKind {
    AcceptDecline,
    ApprovedDenied,
}

impl ApprovalKind {
    const fn decision(self, allow: bool) -> &'static str {
        match (self, allow) {
            (Self::AcceptDecline, true) => "accept",
            (Self::AcceptDecline, false) => "decline",
            (Self::ApprovedDenied, true) => "approved",
            (Self::ApprovedDenied, false) => "denied",
        }
    }
}

/// Classification of a single inbound JSON-RPC object.
#[derive(Debug)]
pub enum Incoming {
    /// Reply to one of our requests: has `id`, no `method`.
    Response { id: i64, value: Value },
    /// Server→client request that blocks on a decision (tool/patch
    /// approval): carries both `method` and `id`. `rpc_id` is echoed back
    /// verbatim in the reply; `request_id` is the stable id surfaced to the
    /// TUI via [`AdapterEvent::PermissionRequest`].
    Approval { rpc_id: Value, request_id: String, tool: String, kind: ApprovalKind, input: Value },
    /// A notification we mapped onto an adapter event.
    Event(AdapterEvent),
    /// Anything we don't act on (turn lifecycle, unknown notifications).
    Ignored,
}

/// Classify one parsed JSON-RPC object. `local_id` is the thread/session id
/// (only meaningful once the handshake has completed; during the handshake
/// only [`Incoming::Response`] values are acted upon).
#[must_use]
pub fn classify(local_id: &str, v: &Value) -> Incoming {
    let has_id = v.get("id").is_some();
    let method = v.get("method").and_then(Value::as_str);
    match (method, has_id) {
        (Some(m), true) => classify_server_request(m, v),
        (Some(m), false) => map_notification(local_id, m, v),
        (None, true) => {
            let id = v.get("id").and_then(Value::as_i64).unwrap_or(-1);
            Incoming::Response { id, value: v.clone() }
        }
        (None, false) => Incoming::Ignored,
    }
}

fn classify_server_request(method: &str, v: &Value) -> Incoming {
    // Map the approval method to its decision vocabulary + a tool label.
    // `item/permissions/requestApproval` (sandbox-permission elevation) is
    // deliberately not relayed: its reply is a permission-profile grant,
    // not a simple allow/deny — see the module-level note / CCT-98.
    let (kind, tool) = match method {
        "item/commandExecution/requestApproval" => (ApprovalKind::AcceptDecline, "shell"),
        "item/fileChange/requestApproval" => (ApprovalKind::AcceptDecline, "file_change"),
        "applyPatchApproval" => (ApprovalKind::ApprovedDenied, "apply_patch"),
        "execCommandApproval" => (ApprovalKind::ApprovedDenied, "shell"),
        _ => return Incoming::Ignored,
    };
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    let rpc_id = v.get("id").cloned().unwrap_or(Value::Null);
    let item_id = params.get("itemId").and_then(Value::as_str);
    let request_id = item_id
        .map_or_else(|| format!("codex-approval-{rpc_id}"), std::string::ToString::to_string);
    Incoming::Approval { rpc_id, request_id, tool: tool.to_string(), kind, input: params }
}

fn map_notification(local_id: &str, method: &str, v: &Value) -> Incoming {
    match method {
        // We emit on `item/completed` only (final state) to avoid duplicate
        // events from the matching `item/started`.
        "item/completed" => map_item_completed(local_id, v),
        // Thread liveness/attention → Status (drives the dots + ✋, CCT-124).
        "thread/status/changed" => map_status(local_id, v),
        // Per-turn token usage → TokenUsage.
        "thread/tokenUsage/updated" => map_token_usage(local_id, v),
        // Thread rename → Status carrying just the name (display gated on CCT-113).
        "thread/name/updated" => map_name(local_id, v),
        // Structured turn errors → failed Status (CCT-631).
        "error" => map_error_notification(local_id, v),
        "turn/completed" => map_turn_completed(local_id, v),
        _ => Incoming::Ignored,
    }
}

/// Map the structured `error` notification → [`AdapterEvent::Status`].
/// `willRetry: true` means codex is retrying the turn itself, so only the
/// detail is surfaced; a non-retried error marks the session failed.
fn map_error_notification(local_id: &str, v: &Value) -> Incoming {
    let Some(message) = v.pointer("/params/error/message").and_then(Value::as_str) else {
        return Incoming::Ignored;
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
        children: vec![],
    })
}

/// Map `turn/completed` whose `turn.status == "failed"` → failed
/// [`AdapterEvent::Status`] carrying the turn error message (CCT-631).
/// Successful turns stay ignored: idle status arrives via
/// `thread/status/changed`.
fn map_turn_completed(local_id: &str, v: &Value) -> Incoming {
    if v.pointer("/params/turn/status").and_then(Value::as_str) != Some("failed") {
        return Incoming::Ignored;
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
        children: vec![],
    })
}

fn map_item_completed(local_id: &str, v: &Value) -> Incoming {
    let Some(item) = v.pointer("/params/item") else { return Incoming::Ignored };
    let ty = item.get("type").and_then(Value::as_str).unwrap_or("");
    let payload = item.clone();
    let evt = match ty {
        "commandExecution" | "fileChange" | "mcpToolCall" => {
            AdapterEvent::ToolUse { local_id: local_id.to_owned(), payload }
        }
        _ => AdapterEvent::Message { local_id: local_id.to_owned(), payload },
    };
    Incoming::Event(evt)
}

/// Map `thread/status/changed` → [`AdapterEvent::Status`]. The codex
/// `ThreadStatus` (`active` / `idle` / `systemError`) plus `activeFlags`
/// (`waitingOnApproval` / `waitingOnUserInput`) project onto the same
/// `tempo`/`state`/`activity` the classifier consumes: a waiting flag means
/// `tempo = "blocked"` (the ✋ "needs input" signal).
fn map_status(local_id: &str, v: &Value) -> Incoming {
    let Some(status) = v.pointer("/params/status") else { return Incoming::Ignored };
    let ty = status.get("type").and_then(Value::as_str).unwrap_or("");
    let waiting = status.get("activeFlags").and_then(Value::as_array).is_some_and(|flags| {
        flags
            .iter()
            .filter_map(Value::as_str)
            .any(|f| f == "waitingOnApproval" || f == "waitingOnUserInput")
    });
    let (tempo, state, activity) = match ty {
        "active" if waiting => (Some("blocked"), Some("working"), None),
        "active" => (Some("active"), Some("working"), None),
        "idle" => (None, Some("idle"), None),
        "systemError" => (None, Some("failed"), Some("failure")),
        // `notLoaded` / unknown — nothing actionable.
        _ => return Incoming::Ignored,
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
        return Incoming::Ignored;
    }
    let Some(last) = v.pointer("/params/tokenUsage/last") else { return Incoming::Ignored };
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
/// tracked separately (CCT-113).
fn map_name(local_id: &str, v: &Value) -> Incoming {
    let Some(name) = v.pointer("/params/name").and_then(Value::as_str) else {
        return Incoming::Ignored;
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
        children: vec![],
    })
}

/// Thread identity extracted from a `thread/start` response.
#[derive(Debug, Clone)]
pub struct ThreadInfo {
    pub thread_id: String,
    pub cwd: Option<String>,
    pub rollout_path: Option<String>,
}

/// Pull thread identity out of a `thread/start` response `result` object.
#[must_use]
pub fn thread_info(result: &Value) -> Option<ThreadInfo> {
    let t = result.get("thread")?;
    let thread_id = t
        .get("sessionId")
        .or_else(|| t.get("id"))
        .and_then(Value::as_str)
        .map(std::string::ToString::to_string)?;
    Some(ThreadInfo {
        thread_id,
        cwd: t.get("cwd").and_then(Value::as_str).map(std::string::ToString::to_string),
        rollout_path: t.get("path").and_then(Value::as_str).map(std::string::ToString::to_string),
    })
}

/// Build the documented `initialize` request (CCT-630). Capabilities are
/// declared explicitly rather than left to defaults so a protocol change that
/// flips a default is visible here: cctui speaks the stable (non-experimental)
/// API and does not participate in upstream attestation.
fn initialize_req() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": ID_INITIALIZE,
        "method": "initialize",
        "params": {
            "clientInfo": {"name": "cctui", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {
                "experimentalApi": false,
                "requestAttestation": false,
            },
        },
    })
}

/// The `initialized` notification that completes the handshake. Codex expects
/// it after the client has processed the `initialize` response; only then is
/// the server fully ready for `thread/*` requests.
fn initialized_notification() -> Value {
    json!({"jsonrpc": "2.0", "method": "initialized"})
}

/// Pull the Codex version out of an `initialize` response and log a diagnostic:
/// info when supported, a loud warning when the server is below
/// [`contract::CODEX_MIN_VERSION`] (the protocol shapes cctui relies on are not
/// guaranteed there). The version is returned so it can ride on the
/// [`AdapterEvent::SessionStarted`] meta for downstream diagnose reports.
fn record_codex_version(response: &Value) -> Option<String> {
    let user_agent = response.pointer("/result/userAgent").and_then(Value::as_str);
    let version = user_agent.and_then(contract::version_from_user_agent);
    match &version {
        Some(v) if contract::version_supported(v) => {
            tracing::info!(
                codex_version = %v,
                pinned = contract::CODEX_PINNED_VERSION,
                "codex app-server handshake: supported version",
            );
        }
        Some(v) => {
            tracing::warn!(
                codex_version = %v,
                min = contract::CODEX_MIN_VERSION,
                pinned = contract::CODEX_PINNED_VERSION,
                "codex app-server is below the minimum supported version; protocol may drift",
            );
        }
        None => {
            tracing::warn!(
                user_agent = user_agent.unwrap_or("<missing>"),
                "codex app-server initialize response had no parseable version",
            );
        }
    }
    version
}

fn thread_start_req(cwd: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": ID_THREAD_START, "method": "thread/start", "params": {"cwd": cwd}})
}

fn thread_resume_req(thread_id: &str, cwd: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": ID_THREAD_START,
        "method": "thread/resume",
        "params": {"threadId": thread_id, "cwd": cwd},
    })
}

/// Fork an existing thread into a brand-new one seeded from its history
/// (CCT-302). The app-server returns a fresh `thread` (its own id) just like
/// `thread/start`, so the response is parsed through the same `ID_THREAD_START`
/// path. Model/effort overrides ride on the subprocess `-c` flags (set in the
/// command pump), mirroring the spawn path, so they apply to the forked thread.
fn thread_fork_req(parent_thread_id: &str, cwd: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": ID_THREAD_START,
        "method": "thread/fork",
        "params": {"threadId": parent_thread_id, "cwd": cwd},
    })
}

fn thread_name_set_req(id: i64, thread_id: &str, name: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "thread/name/set",
        "params": {"threadId": thread_id, "name": name},
    })
}

/// Build the `input` array for a turn (CCT-636). Staged image attachments ride
/// as native `localImage` items so codex feeds the picture to the model; every
/// other staged file keeps the path/text semantics — its absolute path is
/// listed in the text item, matching the adapter-neutral mid-chat injection.
/// The array is never empty: a turn with only images still carries a text item
/// so an image-only prompt is valid.
fn turn_input_items(text: &str, attachments: &[String]) -> Vec<Value> {
    use std::fmt::Write as _;

    let mut body = text.to_owned();
    let non_images: Vec<&str> = attachments
        .iter()
        .map(String::as_str)
        .filter(|p| !crate::adapters::uploads::is_image_path(p))
        .collect();
    if !non_images.is_empty() {
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str("Attached files:");
        for p in non_images {
            let _ = write!(body, "\n  - {p}");
        }
    }

    let mut items = Vec::new();
    if !body.is_empty() {
        items.push(json!({"type": "text", "text": body}));
    }
    for p in attachments.iter().filter(|p| crate::adapters::uploads::is_image_path(p)) {
        items.push(json!({"type": "localImage", "path": p}));
    }
    if items.is_empty() {
        items.push(json!({"type": "text", "text": ""}));
    }
    items
}

/// Build a `turn/start`. An in-place model/effort change (CCT-635) rides here
/// as a per-turn override that codex promotes to the later default — the stable
/// alternative to the `experimentalApi`-gated `thread/settings/update`. Only
/// set fields are sent so an unchanged setting keeps codex's own default.
/// Staged attachments (CCT-636) become native image / path-in-text inputs.
fn turn_start_req(
    id: i64,
    thread_id: &str,
    text: &str,
    attachments: &[String],
    model: Option<&str>,
    effort: Option<&str>,
) -> Value {
    let mut params = serde_json::Map::new();
    params.insert("threadId".to_owned(), json!(thread_id));
    params.insert("input".to_owned(), json!(turn_input_items(text, attachments)));
    if let Some(model) = model {
        params.insert("model".to_owned(), json!(model));
    }
    if let Some(effort) = effort {
        params.insert("effort".to_owned(), json!(effort));
    }
    json!({"jsonrpc": "2.0", "id": id, "method": "turn/start", "params": params})
}

/// Steer a user message into the currently active turn (CCT-634). Unlike
/// `turn/start` — which codex rejects while a turn is in flight — `turn/steer`
/// appends the input to the running turn. `expectedTurnId` is a precondition:
/// the request fails if it no longer matches the active turn (it just ended),
/// which the driver recovers from by falling back to `turn/start`. Attachments
/// (CCT-636) build the same native image / path-in-text inputs as a start.
fn turn_steer_req(
    id: i64,
    thread_id: &str,
    expected_turn_id: &str,
    text: &str,
    attachments: &[String],
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "turn/steer",
        "params": {
            "threadId": thread_id,
            "expectedTurnId": expected_turn_id,
            "input": turn_input_items(text, attachments),
        },
    })
}

fn turn_interrupt_req(id: i64, thread_id: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "turn/interrupt", "params": {"threadId": thread_id}})
}

/// A turn lifecycle transition parsed from a `turn/started` or `turn/completed`
/// notification (CCT-634). The driver tracks the active turn id from these so a
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

/// Tracks the session's in-flight turn (CCT-634). `turn/started` sets the
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

/// How a user message is delivered given the current active turn (CCT-634):
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

/// How to recover from a `turn/steer` failure (CCT-634). A turn that just ended
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

/// Reply to a server-issued approval request. `rpc_id` must be the exact
/// `id` value from the request; `kind` selects the decision vocabulary.
fn approval_reply(rpc_id: &Value, kind: ApprovalKind, allow: bool) -> Value {
    json!({"jsonrpc": "2.0", "id": rpc_id, "result": {"decision": kind.decision(allow)}})
}

/// One outstanding outbound JSON-RPC request (CCT-631).
#[derive(Debug)]
pub struct PendingRpc {
    pub method: String,
    /// Server-minted correlation id: when set, the request's outcome is
    /// reported back as an [`AdapterEvent::CommandResult`].
    pub command_id: Option<Uuid>,
    pub deadline: Instant,
}

impl PendingRpc {
    /// Whether this request is part of the session-establishing handshake —
    /// its failure means the session cannot run at all.
    #[must_use]
    pub fn is_handshake(&self) -> bool {
        matches!(
            self.method.as_str(),
            "initialize" | "thread/start" | "thread/resume" | "thread/fork"
        )
    }
}

/// Correlation table for outbound JSON-RPC requests, keyed by request id
/// (CCT-631). The driver inserts before each write, resolves on the matching
/// response (propagating `error` objects as failures), expires entries past
/// their deadline, and drains everything when the app-server process exits.
#[derive(Debug, Default)]
pub struct PendingRpcs {
    inner: HashMap<i64, PendingRpc>,
}

impl PendingRpcs {
    pub fn insert(&mut self, id: i64, method: &str, command_id: Option<Uuid>, deadline: Instant) {
        self.inner.insert(id, PendingRpc { method: method.to_owned(), command_id, deadline });
    }

    /// Resolve the pending request matching a response `id`. Returns the
    /// entry plus the parsed outcome; `None` for an unknown id.
    pub fn resolve(
        &mut self,
        id: i64,
        response: &Value,
    ) -> Option<(PendingRpc, Result<Value, String>)> {
        let pending = self.inner.remove(&id)?;
        Some((pending, response_outcome(response)))
    }

    /// Remove and return every request whose deadline has passed.
    pub fn expire(&mut self, now: Instant) -> Vec<(i64, PendingRpc)> {
        self.inner.extract_if(|_, p| p.deadline <= now).collect()
    }

    /// Remove and return everything — the process is gone, nothing pending
    /// can ever resolve.
    pub fn drain(&mut self) -> Vec<(i64, PendingRpc)> {
        self.inner.drain().collect()
    }

    #[cfg(test)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Parse a JSON-RPC response into success (`result`) or failure (the `error`
/// object rendered as a message).
fn response_outcome(v: &Value) -> Result<Value, String> {
    let Some(err) = v.get("error").filter(|e| !e.is_null()) else {
        return Ok(v.get("result").cloned().unwrap_or(Value::Null));
    };
    let message = err.get("message").and_then(Value::as_str).unwrap_or("unknown error");
    let mut out = err.get("code").and_then(Value::as_i64).map_or_else(
        || format!("codex app-server error: {message}"),
        |code| format!("codex app-server error {code}: {message}"),
    );
    if let Some(data) = err.get("data").filter(|d| !d.is_null()) {
        use std::fmt::Write as _;
        let _ = write!(out, " ({data})");
    }
    Err(out)
}

// ---------------------------------------------------------------------------
// Async driver
// ---------------------------------------------------------------------------

/// Holds the spawn/fork `command_id` until the launch outcome is known: the
/// success ack is deferred to `thread/start`/`thread/resume`/`thread/fork`
/// succeeding, and every failure path (JSON-RPC error, timeout, process exit,
/// spawn error) resolves it as a failure instead (CCT-631). One-shot: the
/// first resolution wins, later calls are no-ops.
struct SpawnAck {
    command_id: Option<Uuid>,
    events: mpsc::Sender<AdapterEvent>,
}

impl SpawnAck {
    async fn ok(&mut self) {
        if let Some(command_id) = self.command_id.take() {
            let _ = self
                .events
                .send(AdapterEvent::CommandResult { command_id, ok: true, error: None })
                .await;
        }
    }

    async fn fail(&mut self, error: &str) {
        if let Some(command_id) = self.command_id.take() {
            let _ = self
                .events
                .send(AdapterEvent::CommandResult {
                    command_id,
                    ok: false,
                    error: Some(error.to_owned()),
                })
                .await;
        }
    }
}

/// Per-session commands routed from the adapter-level command pump.
#[derive(Debug, Clone)]
pub enum SessionCommand {
    /// Answer a pending approval (`request_id` came from the emitted
    /// `PermissionRequest`).
    Permission { request_id: String, allow: bool },
    /// Start a new turn with user text.
    Send { text: String },
    /// Persist the display name into Codex's thread metadata.
    Rename { name: String },
    /// Interrupt the in-flight turn and terminate the session. `signal` is
    /// the requested POSIX signal: `Some(15)` (SIGTERM) for a graceful stop
    /// that lets codex flush its rollout file; anything else (incl. `None`)
    /// falls back to an immediate SIGKILL.
    Kill { signal: Option<i32> },
    /// Interrupt the in-flight turn but KEEP the session alive (CCT-210):
    /// sends `turn/interrupt` WITHOUT terminating the app-server, so the
    /// thread stays resumable. Distinct from `Kill`, which interrupts *and*
    /// terminates the child. `command_id` correlates the `turn/interrupt`
    /// JSON-RPC outcome back to an [`AdapterEvent::CommandResult`] (CCT-631).
    Interrupt { command_id: Option<Uuid> },
    /// Change the model and/or reasoning effort of the running thread in place
    /// (CCT-303): records the override so the next `turn/start` carries it (a
    /// stable per-turn override codex promotes to the later default, CCT-635),
    /// and echoes the resolved values back via [`AdapterEvent::Status`] so the
    /// webui chip updates live. `command_id` correlates the outcome back as an
    /// [`AdapterEvent::CommandResult`].
    SetModel { model: Option<String>, effort: Option<String>, command_id: Option<Uuid> },
}

impl SessionCommand {
    #[must_use]
    pub const fn is_resumable(&self) -> bool {
        matches!(self, Self::Send { .. } | Self::Rename { .. } | Self::SetModel { .. })
    }
}

/// Live command registry: `local_id` → command sender for the owning app-server
/// task. Senders disappear when the app-server exits; the durable
/// [`SessionRegistry`] below stays so a later reply can revive the thread.
pub type LiveSessionRegistry = Arc<Mutex<HashMap<String, mpsc::Sender<SessionCommand>>>>;

/// Durable-in-daemon metadata for cctui-owned Codex threads. This is not a
/// process handle; it is the minimum launch context needed to call
/// `thread/resume` after a clean app-server exit (CCT-229). The log-tail also
/// uses this map as the ownership set so it does not double-ingest these
/// rollout files while they are hibernated.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub cfg: AppServerConfig,
    pub cwd: String,
    pub name: Option<String>,
    /// Resolved launch-time env (CCT-461) — chiefly the gateway-routing
    /// credential pulled from the server's durable `sessions.account_id`
    /// binding. Stored so a resume relaunches the codex app-server with the
    /// same gateway env instead of starting env-less and 401ing (the codex
    /// analogue of the claude CCT-460 cold-launch bug).
    pub env: std::collections::BTreeMap<String, String>,
}

/// `local_id` → cctui-owned Codex thread metadata.
pub type SessionRegistry = Arc<Mutex<HashMap<String, SessionRecord>>>;

// `Resume` carries a full `SessionRecord` (now incl. the CCT-461 launch env);
// the size gap to the unit `Delivered`/`Missing` variants is intrinsic and the
// value is short-lived (built, matched, dropped per command), so boxing it
// would add an allocation for no real benefit.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum RouteAction {
    Delivered,
    Resume { record: SessionRecord, command: SessionCommand },
    Missing,
}

/// Try the live sender first. If it is gone or closed, fall back to the
/// durable Codex thread record so the caller can spawn a resume driver.
pub async fn route_or_prepare_resume(
    live: &LiveSessionRegistry,
    sessions: &SessionRegistry,
    local_id: &str,
    command: SessionCommand,
) -> RouteAction {
    let sender = live.lock().await.get(local_id).cloned();
    if let Some(tx) = sender {
        if tx.send(command.clone()).await.is_ok() {
            return RouteAction::Delivered;
        }
        live.lock().await.remove(local_id);
        tracing::warn!(%local_id, "codex: live session command channel closed");
    }

    sessions
        .lock()
        .await
        .get(local_id)
        .cloned()
        .map_or(RouteAction::Missing, |record| RouteAction::Resume { record, command })
}

/// Configuration for spawning the `codex app-server` subprocess.
#[derive(Debug, Clone)]
pub struct AppServerConfig {
    /// Binary to invoke (default `"codex"`).
    pub bin: String,
    /// Approval policy passed via `-c approval_policy=...`. `"untrusted"`
    /// (the default) makes Codex ask for approval on commands so the relay
    /// has something to forward; `"never"` disables prompts.
    pub approval_policy: String,
    /// Sandbox mode passed via `-c sandbox_mode=...` (CCT-139). `"read-only"`
    /// and `"workspace-write"` wrap commands in bubblewrap; on a host whose
    /// kernel forbids unprivileged user namespaces those fail to launch, so a
    /// per-host default of `"danger-full-access"` (no sandbox) is required
    /// there. Overridable per-spawn via the full-access toggle.
    pub sandbox_mode: String,
    /// Reasoning effort passed via `-c model_reasoning_effort=...`
    /// (codex: `minimal`/`low`/`medium`/`high`). `None` keeps the codex
    /// default. Set per-spawn from the spawn request.
    pub reasoning_effort: Option<String>,
    /// Model passed via `-c model="…"` (CCT-274). `None` keeps the codex
    /// default. Set per-spawn from the spawn request.
    pub model: Option<String>,
}

impl Default for AppServerConfig {
    fn default() -> Self {
        Self {
            bin: "codex".to_string(),
            approval_policy: "untrusted".to_string(),
            sandbox_mode: "workspace-write".to_string(),
            reasoning_effort: None,
            model: None,
        }
    }
}

impl AppServerConfig {
    /// The `-c key="value"` overrides passed to `codex app-server` for a spawn.
    /// This is the COMPLETE set of config knobs cctui sets — kept as a single
    /// function so the "Fast mode is never silently enabled" guarantee (CCT-339)
    /// is testable. Codex's "Fast mode" is a separate per-thread setting; cctui
    /// never sets it here (no `fast`/`model_fast`/`reasoning_fast` key), so a
    /// spawned session always uses the user's normal model/effort, never the
    /// degraded fast path. Reasoning effort and model are the only opt-in
    /// quality knobs, both surfaced explicitly in the spawn picker (CCT-299/303).
    #[must_use]
    pub fn config_overrides(&self) -> Vec<(String, String)> {
        let mut args = vec![
            ("approval_policy".to_owned(), self.approval_policy.clone()),
            ("sandbox_mode".to_owned(), self.sandbox_mode.clone()),
        ];
        if let Some(effort) = self.reasoning_effort.as_deref() {
            args.push(("model_reasoning_effort".to_owned(), effort.to_owned()));
        }
        if let Some(model) = self.model.as_deref() {
            args.push(("model".to_owned(), model.to_owned()));
        }
        args
    }

    pub fn from_value(v: &Value) -> Self {
        let mut cfg = Self::default();
        if let Some(b) = v.get("codex_bin").and_then(Value::as_str) {
            cfg.bin = b.to_string();
        }
        if let Some(p) = v.get("approval_policy").and_then(Value::as_str) {
            cfg.approval_policy = p.to_string();
        }
        if let Some(s) = v.get("sandbox_mode").and_then(Value::as_str) {
            cfg.sandbox_mode = s.to_string();
        }
        if let Some(e) = v.get("model_reasoning_effort").and_then(Value::as_str) {
            cfg.reasoning_effort = Some(e.to_string());
        }
        if let Some(m) = v.get("model").and_then(Value::as_str) {
            cfg.model = Some(m.to_string());
        }
        cfg
    }
}

#[derive(Debug, Clone)]
enum SessionLaunch {
    Fresh {
        prompt: Option<String>,
        name: Option<String>,
        /// Staged spawn-attachment paths (CCT-636), fed into the first turn.
        attachments: Vec<String>,
    },
    Resume {
        thread_id: String,
        initial_commands: Vec<SessionCommand>,
    },
    /// Fork a parent thread into a new one seeded from its history (CCT-302).
    /// Post-fork it behaves like `Fresh` (optional name + first turn), but the
    /// start handshake sends `thread/fork { threadId }` and the resulting
    /// `SessionStarted` carries `parent_local_id` for discoverability.
    Fork {
        parent_thread_id: String,
        prompt: Option<String>,
        name: Option<String>,
        attachments: Vec<String>,
    },
}

/// One spawned Codex session: owns a `codex app-server` subprocess and a
/// single thread within it.
pub struct CodexSession {
    cfg: AppServerConfig,
    cwd: String,
    /// Launch-time env merged onto the `codex app-server` child process
    /// (CCT-461). Holds the gateway-routing credential resolved at spawn /
    /// fork / resume; see [`SessionRecord::env`].
    env: std::collections::BTreeMap<String, String>,
    launch: SessionLaunch,
    /// Spawn/fork correlation id (CCT-631): resolved as an
    /// [`AdapterEvent::CommandResult`] only once the launch outcome is known.
    command_id: Option<Uuid>,
    events: mpsc::Sender<AdapterEvent>,
    live: LiveSessionRegistry,
    registry: SessionRegistry,
    shutdown: CancellationToken,
}

impl CodexSession {
    #[allow(clippy::too_many_arguments)]
    pub const fn new_fresh(
        cfg: AppServerConfig,
        cwd: String,
        env: std::collections::BTreeMap<String, String>,
        prompt: Option<String>,
        name: Option<String>,
        attachments: Vec<String>,
        command_id: Option<Uuid>,
        events: mpsc::Sender<AdapterEvent>,
        live: LiveSessionRegistry,
        registry: SessionRegistry,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            cfg,
            cwd,
            env,
            launch: SessionLaunch::Fresh { prompt, name, attachments },
            command_id,
            events,
            live,
            registry,
            shutdown,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub const fn new_fork(
        cfg: AppServerConfig,
        cwd: String,
        env: std::collections::BTreeMap<String, String>,
        parent_thread_id: String,
        prompt: Option<String>,
        name: Option<String>,
        attachments: Vec<String>,
        command_id: Option<Uuid>,
        events: mpsc::Sender<AdapterEvent>,
        live: LiveSessionRegistry,
        registry: SessionRegistry,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            cfg,
            cwd,
            env,
            launch: SessionLaunch::Fork { parent_thread_id, prompt, name, attachments },
            command_id,
            events,
            live,
            registry,
            shutdown,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub const fn new_resume(
        cfg: AppServerConfig,
        cwd: String,
        env: std::collections::BTreeMap<String, String>,
        thread_id: String,
        initial_commands: Vec<SessionCommand>,
        events: mpsc::Sender<AdapterEvent>,
        live: LiveSessionRegistry,
        registry: SessionRegistry,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            cfg,
            cwd,
            env,
            launch: SessionLaunch::Resume { thread_id, initial_commands },
            command_id: None,
            events,
            live,
            registry,
            shutdown,
        }
    }

    /// Spawn the subprocess, complete the handshake, then pump IO until the
    /// process exits, the session is killed, or the daemon shuts down.
    /// The spawn/fork `command_id` (when present) is resolved exactly once:
    /// `ok` after the thread request succeeds, failure on any other outcome.
    pub async fn run(mut self) -> Result<()> {
        let mut ack = SpawnAck { command_id: self.command_id.take(), events: self.events.clone() };
        let res = self.run_inner(&mut ack).await;
        match &res {
            Err(err) => ack.fail(&err.to_string()).await,
            Ok(()) => ack.fail("codex app-server exited before the thread was started").await,
        }
        res
    }

    #[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
    async fn run_inner(&self, ack: &mut SpawnAck) -> Result<()> {
        let cwd_path = std::path::Path::new(&self.cwd);
        if !cwd_path.is_dir() {
            anyhow::bail!("spawn: working_dir does not exist or is not a directory: {}", self.cwd);
        }

        let mut cmd = Command::new(&self.cfg.bin);
        cmd.arg("app-server");
        for (key, value) in self.cfg.config_overrides() {
            cmd.arg("-c").arg(format!("{key}=\"{value}\""));
        }
        // Forward the resolved launch env (CCT-461) — chiefly the gateway
        // credential pulled from the server's `sessions.account_id` binding —
        // onto the app-server child, so a session bound to a named gateway
        // account routes through it instead of hitting the default upstream and
        // 401ing. Applied before `PATH` below so the launchd PATH fix wins even
        // if the resolved env carried a `PATH` of its own. The fail-closed
        // contract (refuse an account-bound launch with empty gateway env) is
        // enforced upstream in the adapter command pump; see CCT-460.
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        let mut child = cmd
            .current_dir(cwd_path)
            // launchd strips `PATH` down to a minimal set that omits
            // `/opt/homebrew/bin`, so a bare `codex` fails ENOENT (CCT-138).
            .env("PATH", crate::childenv::child_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Capture stderr (the app-server's log stream) rather than
            // discarding it — it is the only diagnostic when codex dies
            // unexpectedly (CCT macOS "randomly dies" report).
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn `{} app-server`", self.cfg.bin))?;

        let mut stdin = child.stdin.take().context("child stdin missing")?;
        let stdout = child.stdout.take().context("child stdout missing")?;
        let mut lines = BufReader::new(stdout).lines();

        // Drain stderr into a bounded ring buffer in the background. Each
        // line is also logged at debug; the retained tail is surfaced in the
        // crash detail below.
        let stderr_ring: Arc<Mutex<VecDeque<String>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_RING)));
        if let Some(stderr) = child.stderr.take() {
            let ring = stderr_ring.clone();
            tokio::spawn(async move {
                let mut err_lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = err_lines.next_line().await {
                    tracing::debug!(target: "codex_app_server_stderr", "{line}");
                    let mut guard = ring.lock().await;
                    if guard.len() == STDERR_RING {
                        guard.pop_front();
                    }
                    guard.push_back(line);
                }
            });
        }

        // Handshake: initialize → thread/start or thread/resume.
        let mut pending_rpcs = PendingRpcs::default();
        pending_rpcs.insert(ID_INITIALIZE, "initialize", None, Instant::now() + RPC_TIMEOUT);
        write_json(&mut stdin, &initialize_req()).await?;
        let mut local_id = String::new();
        let mut codex_version: Option<String> = None;
        let mut next_id = RUN_BASE;
        // request_id (surfaced to TUI) → (rpc_id echoed to codex, decision kind).
        let mut pending_approvals: HashMap<String, (Value, ApprovalKind)> = HashMap::new();
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<SessionCommand>(32);
        let mut registered = false;
        // Set when the session is terminated on purpose (daemon shutdown or a
        // Kill command) so the epilogue reports `Killed` rather than treating
        // the non-zero exit as a crash.
        let mut killed = false;
        let mut retry_after_hibernate: Option<SessionCommand> = None;
        let mut active_turn = ActiveTurn::default();
        // In-place model/effort override (CCT-635). A SetModel records it here;
        // every subsequent `turn/start` carries it so codex adopts it as the
        // later default. Left `None` at launch — the spawn-time `-c model=`/
        // `-c model_reasoning_effort=` flags already seed the initial turns.
        let mut override_model: Option<String> = None;
        let mut override_effort: Option<String> = None;
        let mut steer_texts: HashMap<i64, String> = HashMap::new();
        let mut sweep = tokio::time::interval(Duration::from_secs(1));
        sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => {
                    let _ = child.start_kill();
                    killed = true;
                    break;
                }
                _ = sweep.tick() => {
                    let mut handshake_dead = false;
                    for (id, pending) in pending_rpcs.expire(Instant::now()) {
                        tracing::warn!(rpc_id = id, method = %pending.method, "codex: JSON-RPC request timed out");
                        if let Some(command_id) = pending.command_id {
                            let _ = self.events
                                .send(AdapterEvent::CommandResult {
                                    command_id,
                                    ok: false,
                                    error: Some(format!("codex {} timed out", pending.method)),
                                })
                                .await;
                        }
                        if pending.is_handshake() {
                            ack.fail(&format!("codex {} timed out", pending.method)).await;
                            handshake_dead = true;
                        }
                    }
                    if handshake_dead {
                        let _ = child.start_kill();
                        break;
                    }
                }
                cmd = cmd_rx.recv(), if registered => {
                    match cmd {
                        Some(SessionCommand::Permission { request_id, allow }) => {
                            if let Some((rpc_id, kind)) = pending_approvals.remove(&request_id) {
                                if let Err(e) =
                                    write_json(&mut stdin, &approval_reply(&rpc_id, kind, allow)).await
                                {
                                    tracing::warn!(%e, "codex: approval write failed; ending session");
                                    break;
                                }
                            } else {
                                tracing::warn!(%request_id, "codex: no pending approval for response");
                            }
                        }
                        Some(SessionCommand::Send { text }) => {
                            let (req, method) = match prompt_dispatch(&active_turn) {
                                PromptDispatch::Steer { turn_id } => {
                                    steer_texts.insert(next_id, text.clone());
                                    (turn_steer_req(next_id, &local_id, &turn_id, &text, &[]), "turn/steer")
                                }
                                PromptDispatch::Start => {
                                    (
                                        turn_start_req(
                                            next_id,
                                            &local_id,
                                            &text,
                                            &[],
                                            override_model.as_deref(),
                                            override_effort.as_deref(),
                                        ),
                                        "turn/start",
                                    )
                                }
                            };
                            pending_rpcs.insert(next_id, method, None, Instant::now() + RPC_TIMEOUT);
                            next_id += 1;
                            // A write failure here means the app-server is gone
                            // — remember the turn and let the epilogue revive
                            // the thread if this was a clean hibernation exit.
                            if let Err(e) = write_json(&mut stdin, &req).await {
                                tracing::warn!(%e, "codex: turn dispatch write failed; ending session");
                                steer_texts.remove(&(next_id - 1));
                                retry_after_hibernate = Some(SessionCommand::Send { text });
                                break;
                            }
                        }
                        Some(SessionCommand::Rename { name }) => {
                            if let Err(e) = set_thread_name(
                                &mut stdin,
                                &mut next_id,
                                &mut pending_rpcs,
                                &local_id,
                                &name,
                                &self.events,
                                &self.registry,
                            )
                            .await
                            {
                                tracing::warn!(%e, "codex: thread/name/set write failed; ending session");
                                retry_after_hibernate = Some(SessionCommand::Rename { name });
                                break;
                            }
                        }
                        Some(SessionCommand::Kill { signal }) => {
                            let req = turn_interrupt_req(next_id, &local_id);
                            let _ = write_json(&mut stdin, &req).await;
                            terminate_child(&mut child, signal);
                            killed = true;
                            break;
                        }
                        Some(SessionCommand::Interrupt { command_id }) => {
                            // Keep-alive interrupt (CCT-210): abort the turn but
                            // leave the app-server running so the session keeps
                            // going — unlike Kill, we do NOT terminate the child.
                            let req = turn_interrupt_req(next_id, &local_id);
                            pending_rpcs.insert(next_id, "turn/interrupt", command_id, Instant::now() + RPC_TIMEOUT);
                            next_id += 1;
                            if let Err(e) = write_json(&mut stdin, &req).await {
                                tracing::warn!(%e, "codex: turn/interrupt write failed; ending session");
                                break;
                            }
                        }
                        Some(SessionCommand::SetModel { model, effort, command_id }) => {
                            record_model_override(
                                &mut override_model,
                                &mut override_effort,
                                model.as_deref(),
                                effort.as_deref(),
                                &local_id,
                                &self.events,
                                &self.registry,
                                command_id,
                            )
                            .await;
                        }
                        None => break,
                    }
                }
                line = lines.next_line() => {
                    // EOF (`Ok(None)`) or a read error both mean the app-server
                    // is gone; break and let the epilogue classify the exit.
                    let line = match line {
                        Ok(Some(line)) => line,
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!(%e, "codex: stdout read error; ending session");
                            break;
                        }
                    };
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                        tracing::debug!(line = %trimmed, "codex: non-JSON line");
                        continue;
                    };
                    if let Some(ev) = turn_lifecycle(&value) {
                        active_turn.apply(&ev);
                    }
                    match classify(&local_id, &value) {
                        Incoming::Response { id, value } => {
                            let Some((pending, outcome)) = pending_rpcs.resolve(id, &value) else {
                                tracing::debug!(rpc_id = id, "codex: response for unknown request id");
                                continue;
                            };
                            match (pending.method.as_str(), outcome) {
                        ("initialize", Ok(_)) => {
                            codex_version = record_codex_version(&value);
                            // Complete the documented handshake before any
                            // thread request (CCT-630): the server treats
                            // `thread/*` sent before `initialized` as premature.
                            write_json(&mut stdin, &initialized_notification()).await?;
                            let (req, method) = match &self.launch {
                                SessionLaunch::Fresh { .. } => {
                                    (thread_start_req(&self.cwd), "thread/start")
                                }
                                SessionLaunch::Resume { thread_id, .. } => {
                                    (thread_resume_req(thread_id, &self.cwd), "thread/resume")
                                }
                                SessionLaunch::Fork { parent_thread_id, .. } => {
                                    (thread_fork_req(parent_thread_id, &self.cwd), "thread/fork")
                                }
                            };
                            pending_rpcs.insert(
                                ID_THREAD_START,
                                method,
                                None,
                                Instant::now() + HANDSHAKE_TIMEOUT,
                            );
                            write_json(&mut stdin, &req).await?;
                        }
                        ("thread/start" | "thread/resume" | "thread/fork", Ok(result)) => {
                            let Some(info) = thread_info(&result) else {
                                anyhow::bail!("codex thread/start response missing thread id");
                            };
                            local_id.clone_from(&info.thread_id);
                            // Link a forked thread back to its parent (CCT-302)
                            // so the server resolves `parent_id`.
                            let parent_local_id = match &self.launch {
                                SessionLaunch::Fork { parent_thread_id, .. } => {
                                    Some(parent_thread_id.clone())
                                }
                                _ => None,
                            };
                            self.events
                                .send(AdapterEvent::SessionStarted {
                                    local_id: local_id.clone(),
                                    meta: SessionMeta {
                                        working_dir: info.cwd.or_else(|| Some(self.cwd.clone())),
                                        parent_local_id,
                                        extra: json!({
                                            "source": "codex-app-server",
                                            "rollout_path": info.rollout_path,
                                            "codex_version": codex_version,
                                        }),
                                    },
                                })
                                .await
                                .ok();
                            let remembered_name = match &self.launch {
                                SessionLaunch::Fresh { name, .. }
                                | SessionLaunch::Fork { name, .. } => name.clone(),
                                SessionLaunch::Resume { .. } => self
                                    .registry
                                    .lock()
                                    .await
                                    .get(&local_id)
                                    .and_then(|r| r.name.clone()),
                            };
                            self.registry.lock().await.insert(
                                local_id.clone(),
                                SessionRecord {
                                    cfg: self.cfg.clone(),
                                    cwd: self.cwd.clone(),
                                    name: remembered_name.clone(),
                                    env: self.env.clone(),
                                },
                            );
                            self.live.lock().await.insert(local_id.clone(), cmd_tx.clone());
                            registered = true;
                            ack.ok().await;
                            // Surface the configured model + reasoning effort so
                            // the session list shows them (claude gets this for
                            // free via state.json; codex has no equivalent feed).
                            // Emit when either is known (CCT-299).
                            let model = self.cfg.model.clone();
                            let effort = self.cfg.reasoning_effort.clone();
                            if model.is_some() || effort.is_some() {
                                self.events
                                    .send(AdapterEvent::Status {
                                        local_id: local_id.clone(),
                                        tempo: None,
                                        state: None,
                                        detail: None,
                                        activity: None,
                                        name: None,
                                        intent: None,
                                        model,
                                        effort,
                                        children: vec![],
                                    })
                                    .await
                                    .ok();
                            }

                            let mut end_after_initial = false;
                            match &self.launch {
                                SessionLaunch::Fresh { name, prompt, attachments }
                                | SessionLaunch::Fork { name, prompt, attachments, .. } => {
                                    if let Some(name) = name.as_deref() {
                                        let result = set_thread_name(
                                            &mut stdin,
                                            &mut next_id,
                                            &mut pending_rpcs,
                                            &local_id,
                                            name,
                                            &self.events,
                                            &self.registry,
                                        )
                                        .await;
                                        if let Err(e) = result {
                                            tracing::warn!(%e, "codex: initial thread/name/set failed");
                                            retry_after_hibernate =
                                                Some(SessionCommand::Rename { name: name.to_owned() });
                                            end_after_initial = true;
                                        }
                                    }
                                    // Send the first turn when there is a prompt OR
                                    // staged attachments (CCT-636) — an image-only
                                    // spawn carries no prompt text but must still
                                    // reach codex as a `localImage` turn input.
                                    if !end_after_initial
                                        && (prompt.is_some() || !attachments.is_empty())
                                    {
                                        let prompt_text = prompt.as_deref().unwrap_or("");
                                        let req = turn_start_req(
                                            next_id,
                                            &local_id,
                                            prompt_text,
                                            attachments,
                                            override_model.as_deref(),
                                            override_effort.as_deref(),
                                        );
                                        pending_rpcs.insert(
                                            next_id,
                                            "turn/start",
                                            None,
                                            Instant::now() + RPC_TIMEOUT,
                                        );
                                        next_id += 1;
                                        if let Err(e) = write_json(&mut stdin, &req).await {
                                            tracing::warn!(%e, "codex: initial prompt write failed; ending session");
                                            retry_after_hibernate =
                                                Some(SessionCommand::Send { text: prompt_text.to_owned() });
                                            end_after_initial = true;
                                        }
                                    }
                                }
                                SessionLaunch::Resume { initial_commands, .. } => {
                                    for command in initial_commands.clone() {
                                        match command {
                                            SessionCommand::Send { text } => {
                                                let req = turn_start_req(
                                                    next_id,
                                                    &local_id,
                                                    &text,
                                                    &[],
                                                    override_model.as_deref(),
                                                    override_effort.as_deref(),
                                                );
                                                pending_rpcs.insert(
                                                    next_id,
                                                    "turn/start",
                                                    None,
                                                    Instant::now() + RPC_TIMEOUT,
                                                );
                                                next_id += 1;
                                                if let Err(e) = write_json(&mut stdin, &req).await {
                                                    tracing::warn!(%e, "codex: resumed turn/start write failed");
                                                    retry_after_hibernate =
                                                        Some(SessionCommand::Send { text });
                                                    end_after_initial = true;
                                                    break;
                                                }
                                            }
                                            SessionCommand::Rename { name } => {
                                                if let Err(e) = set_thread_name(
                                                    &mut stdin,
                                                    &mut next_id,
                                                    &mut pending_rpcs,
                                                    &local_id,
                                                    &name,
                                                    &self.events,
                                                    &self.registry,
                                                )
                                                .await
                                                {
                                                    tracing::warn!(%e, "codex: resumed thread/name/set write failed");
                                                    retry_after_hibernate =
                                                        Some(SessionCommand::Rename { name });
                                                    end_after_initial = true;
                                                    break;
                                                }
                                            }
                                            SessionCommand::SetModel { model, effort, command_id } => {
                                                record_model_override(
                                                    &mut override_model,
                                                    &mut override_effort,
                                                    model.as_deref(),
                                                    effort.as_deref(),
                                                    &local_id,
                                                    &self.events,
                                                    &self.registry,
                                                    command_id,
                                                )
                                                .await;
                                            }
                                            other => {
                                                tracing::warn!(?other, "codex: ignoring non-resumable initial command");
                                            }
                                        }
                                    }
                                }
                            }
                            if end_after_initial {
                                break;
                            }
                        }
                        (method, Err(err)) if pending.is_handshake() => {
                            tracing::error!(%err, %method, "codex: handshake request failed; ending session");
                            ack.fail(&err).await;
                            let _ = child.start_kill();
                            break;
                        }
                        ("turn/interrupt", outcome) => {
                            if let Some(command_id) = pending.command_id {
                                let (ok, error) = match &outcome {
                                    Ok(_) => (true, None),
                                    Err(e) => (false, Some(e.clone())),
                                };
                                let _ = self.events
                                    .send(AdapterEvent::CommandResult { command_id, ok, error })
                                    .await;
                            }
                            if let Err(err) = outcome {
                                tracing::warn!(%err, "codex: turn/interrupt failed");
                            }
                        }
                        ("turn/steer", Ok(_)) => {
                            steer_texts.remove(&id);
                        }
                        ("turn/steer", Err(err)) => {
                            let text = steer_texts.remove(&id);
                            match (steer_recovery(&err), text) {
                                (SteerRecovery::FallbackToStart, Some(text)) => {
                                    active_turn.clear();
                                    tracing::info!(%err, "codex: turn/steer stale; falling back to turn/start");
                                    let req = turn_start_req(
                                        next_id,
                                        &local_id,
                                        &text,
                                        &[],
                                        override_model.as_deref(),
                                        override_effort.as_deref(),
                                    );
                                    pending_rpcs.insert(next_id, "turn/start", None, Instant::now() + RPC_TIMEOUT);
                                    next_id += 1;
                                    if let Err(e) = write_json(&mut stdin, &req).await {
                                        tracing::warn!(%e, "codex: turn/start fallback write failed; ending session");
                                        retry_after_hibernate = Some(SessionCommand::Send { text });
                                        break;
                                    }
                                }
                                (recovery, _) => {
                                    tracing::warn!(%err, ?recovery, "codex: turn/steer rejected");
                                    self.events
                                        .send(AdapterEvent::Status {
                                            local_id: local_id.clone(),
                                            tempo: None,
                                            state: Some("failed".to_owned()),
                                            detail: Some(err),
                                            activity: Some("failure".to_owned()),
                                            name: None,
                                            intent: None,
                                            model: None,
                                            effort: None,
                                            children: vec![],
                                        })
                                        .await
                                        .ok();
                                }
                            }
                        }
                        (method, Err(err)) => {
                            tracing::warn!(%err, %method, "codex: JSON-RPC request failed");
                            if let Some(command_id) = pending.command_id {
                                let _ = self.events
                                    .send(AdapterEvent::CommandResult {
                                        command_id,
                                        ok: false,
                                        error: Some(err.clone()),
                                    })
                                    .await;
                            }
                            if method == "turn/start" {
                                self.events
                                    .send(AdapterEvent::Status {
                                        local_id: local_id.clone(),
                                        tempo: None,
                                        state: Some("failed".to_owned()),
                                        detail: Some(err),
                                        activity: Some("failure".to_owned()),
                                        name: None,
                                        intent: None,
                                        model: None,
                                        effort: None,
                                        children: vec![],
                                    })
                                    .await
                                    .ok();
                            }
                        }
                        (_, Ok(_)) => {}
                            }
                        }
                        Incoming::Approval { rpc_id, request_id, tool, kind, input } => {
                            pending_approvals.insert(request_id.clone(), (rpc_id, kind));
                            self.events
                                .send(AdapterEvent::PermissionRequest {
                                    local_id: local_id.clone(),
                                    request_id,
                                    tool,
                                    input,
                                })
                                .await
                                .ok();
                        }
                        Incoming::Event(evt) => {
                            self.events.send(evt).await.ok();
                        }
                        Incoming::Ignored => {}
                    }
                }
            }
        }

        for (id, pending) in pending_rpcs.drain() {
            tracing::warn!(rpc_id = id, method = %pending.method, "codex: cancelling pending request — app-server gone");
            if let Some(command_id) = pending.command_id {
                let _ = self
                    .events
                    .send(AdapterEvent::CommandResult {
                        command_id,
                        ok: false,
                        error: Some(format!(
                            "codex {}: app-server exited before responding",
                            pending.method
                        )),
                    })
                    .await;
            }
        }

        // Reap the child and classify why the session ended. An abnormal exit
        // that we did not request is surfaced as `Crashed` with the captured
        // stderr tail — the diagnostic for the macOS "randomly dies" report.
        let status = child.wait().await;
        if !local_id.is_empty() {
            self.live.lock().await.remove(&local_id);
            let reason = if killed {
                Some(EndReason::Killed)
            } else {
                match status {
                    Ok(s) if s.success() => None,
                    Ok(s) => Some(EndReason::Crashed {
                        detail: format!(
                            "codex app-server exited ({s}){}",
                            stderr_tail(&stderr_ring).await
                        ),
                    }),
                    Err(e) => Some(EndReason::Crashed {
                        detail: format!("codex app-server wait failed: {e}"),
                    }),
                }
            };
            if let Some(reason) = reason {
                if let EndReason::Crashed { detail } = &reason {
                    tracing::error!(%detail, "codex app-server session crashed");
                }
                self.registry.lock().await.remove(&local_id);
                self.events
                    .send(AdapterEvent::SessionEnded { local_id: local_id.clone(), reason })
                    .await
                    .ok();
            } else {
                self.events
                    .send(AdapterEvent::Status {
                        local_id: local_id.clone(),
                        tempo: Some("hibernated".to_owned()),
                        state: None,
                        detail: None,
                        activity: None,
                        name: None,
                        intent: None,
                        model: None,
                        effort: None,
                        children: Vec::new(),
                    })
                    .await
                    .ok();
                let retry = if let Some(command) = retry_after_hibernate {
                    let record = self.registry.lock().await.get(&local_id).cloned();
                    record.map(|record| (record, command))
                } else {
                    None
                };
                if let Some((record, command)) = retry {
                    spawn_resumed_session(
                        record,
                        local_id.clone(),
                        command,
                        self.events.clone(),
                        self.live.clone(),
                        self.registry.clone(),
                        self.shutdown.clone(),
                    );
                }
            }
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn set_thread_name<W: AsyncWriteExt + Unpin>(
    stdin: &mut W,
    next_id: &mut i64,
    pending_rpcs: &mut PendingRpcs,
    thread_id: &str,
    name: &str,
    events: &mpsc::Sender<AdapterEvent>,
    registry: &SessionRegistry,
) -> Result<()> {
    pending_rpcs.insert(*next_id, "thread/name/set", None, Instant::now() + RPC_TIMEOUT);
    write_json(stdin, &thread_name_set_req(*next_id, thread_id, name)).await?;
    *next_id += 1;
    if let Some(record) = registry.lock().await.get_mut(thread_id) {
        record.name = Some(name.to_owned());
    }
    events
        .send(AdapterEvent::Status {
            local_id: thread_id.to_owned(),
            tempo: None,
            state: None,
            detail: None,
            activity: None,
            name: Some(name.to_owned()),
            intent: None,
            model: None,
            effort: None,
            children: Vec::new(),
        })
        .await
        .ok();
    Ok(())
}

/// Record an in-place model/effort change (CCT-303, CCT-635). Stashes the
/// override in `override_model`/`override_effort` (carried on the next
/// `turn/start`, which codex promotes to the later default — the stable path,
/// vs the `experimentalApi`-gated `thread/settings/update` codex 0.144.1
/// rejects) and folds it into the durable `SessionRecord` cfg so a resume
/// relaunches with matching `-c model=`/`-c model_reasoning_effort=` flags.
/// No app-server round-trip can reject it, so the chip (`Status`) and the
/// `command_id` ack (`CommandResult`) are truthful the moment they fire here.
#[allow(clippy::too_many_arguments)]
async fn record_model_override(
    override_model: &mut Option<String>,
    override_effort: &mut Option<String>,
    model: Option<&str>,
    effort: Option<&str>,
    thread_id: &str,
    events: &mpsc::Sender<AdapterEvent>,
    registry: &SessionRegistry,
    command_id: Option<Uuid>,
) {
    if let Some(model) = model {
        *override_model = Some(model.to_owned());
    }
    if let Some(effort) = effort {
        *override_effort = Some(effort.to_owned());
    }
    if let Some(record) = registry.lock().await.get_mut(thread_id) {
        if let Some(model) = model {
            record.cfg.model = Some(model.to_owned());
        }
        if let Some(effort) = effort {
            record.cfg.reasoning_effort = Some(effort.to_owned());
        }
    }
    events
        .send(AdapterEvent::Status {
            local_id: thread_id.to_owned(),
            tempo: None,
            state: None,
            detail: None,
            activity: None,
            name: None,
            intent: None,
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
            children: Vec::new(),
        })
        .await
        .ok();
    if let Some(command_id) = command_id {
        events
            .send(AdapterEvent::CommandResult { command_id, ok: true, error: None })
            .await
            .ok();
    }
}

pub fn spawn_resumed_session(
    record: SessionRecord,
    thread_id: String,
    command: SessionCommand,
    events: mpsc::Sender<AdapterEvent>,
    live: LiveSessionRegistry,
    registry: SessionRegistry,
    shutdown: CancellationToken,
) {
    if !command.is_resumable() {
        tracing::warn!(%thread_id, ?command, "codex: command is not resumable");
        return;
    }
    let session = CodexSession::new_resume(
        record.cfg,
        record.cwd,
        record.env,
        thread_id,
        vec![command],
        events,
        live,
        registry,
        shutdown,
    );
    tokio::spawn(async move {
        if let Err(err) = session.run().await {
            tracing::error!(%err, "codex resumed app-server session ended in error");
        }
    });
}

/// Format the retained stderr tail for inclusion in a crash detail. Empty
/// when nothing was captured.
async fn stderr_tail(ring: &Arc<Mutex<VecDeque<String>>>) -> String {
    let lines: Vec<String> = { ring.lock().await.iter().cloned().collect() };
    if lines.is_empty() { String::new() } else { format!("; last stderr:\n{}", lines.join("\n")) }
}

/// SIGTERM, per POSIX. The control-plane `Kill { signal }` uses raw signal
/// numbers; 15 is the one graceful case we special-case.
const SIGTERM: i32 = 15;

/// Terminate the child with the requested signal. `Some(15)` (SIGTERM)
/// gives codex a chance to flush its rollout file; anything else (incl.
/// `None`) is an immediate SIGKILL via tokio's `start_kill`.
fn terminate_child(child: &mut tokio::process::Child, signal: Option<i32>) {
    if signal == Some(SIGTERM)
        && let Some(pid) =
            child.id().and_then(|p| i32::try_from(p).ok()).and_then(rustix::process::Pid::from_raw)
    {
        // A reaped pid just yields ESRCH, which we ignore.
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::Term);
        return;
    }
    let _ = child.start_kill();
}

async fn write_json<W: AsyncWriteExt + Unpin>(w: &mut W, v: &Value) -> Result<()> {
    let mut line = serde_json::to_string(v)?;
    line.push('\n');
    w.write_all(line.as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_response() {
        let v = json!({"id": 2, "result": {"thread": {"sessionId": "abc"}}});
        match classify("", &v) {
            Incoming::Response { id, .. } => assert_eq!(id, 2),
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn classifies_command_approval_request() {
        // Server→client request: has both method and id.
        let v = json!({
            "method": "item/commandExecution/requestApproval",
            "id": 0,
            "params": {"threadId": "t", "itemId": "call_9", "command": "rm -rf /"},
        });
        match classify("sess", &v) {
            Incoming::Approval { rpc_id, request_id, tool, kind, .. } => {
                assert_eq!(rpc_id, json!(0));
                assert_eq!(request_id, "call_9");
                assert_eq!(tool, "shell");
                assert_eq!(kind, ApprovalKind::AcceptDecline);
            }
            other => panic!("expected Approval, got {other:?}"),
        }
    }

    #[test]
    fn classifies_apply_patch_approval() {
        let v = json!({"method": "applyPatchApproval", "id": 5, "params": {"itemId": "p1"}});
        match classify("s", &v) {
            Incoming::Approval { tool, request_id, kind, .. } => {
                assert_eq!(tool, "apply_patch");
                assert_eq!(request_id, "p1");
                assert_eq!(kind, ApprovalKind::ApprovedDenied);
            }
            other => panic!("expected apply_patch Approval, got {other:?}"),
        }
    }

    #[test]
    fn file_change_and_exec_command_approvals_classify() {
        match classify(
            "s",
            &json!({"method": "item/fileChange/requestApproval", "id": 1, "params": {}}),
        ) {
            Incoming::Approval { kind, tool, .. } => {
                assert_eq!(kind, ApprovalKind::AcceptDecline);
                assert_eq!(tool, "file_change");
            }
            other => panic!("expected file-change Approval, got {other:?}"),
        }
        match classify("s", &json!({"method": "execCommandApproval", "id": 1, "params": {}})) {
            Incoming::Approval { kind, .. } => assert_eq!(kind, ApprovalKind::ApprovedDenied),
            other => panic!("expected exec-command Approval, got {other:?}"),
        }
    }

    #[test]
    fn permissions_approval_is_not_relayed() {
        // Sandbox-permission elevation has no simple allow/deny reply.
        let v = json!({"method": "item/permissions/requestApproval", "id": 1, "params": {}});
        assert!(matches!(classify("s", &v), Incoming::Ignored));
    }

    #[test]
    fn approval_without_item_id_falls_back_to_rpc_id() {
        let v = json!({"method": "item/commandExecution/requestApproval", "id": 7, "params": {}});
        match classify("s", &v) {
            Incoming::Approval { request_id, .. } => assert_eq!(request_id, "codex-approval-7"),
            other => panic!("expected Approval, got {other:?}"),
        }
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
        assert!(matches!(classify("sess", &v), Incoming::Ignored));
    }

    #[test]
    fn turn_lifecycle_is_ignored() {
        assert!(matches!(
            classify("s", &json!({"method": "turn/completed", "params": {}})),
            Incoming::Ignored
        ));
        assert!(matches!(
            classify("s", &json!({"method": "thread/started", "params": {}})),
            Incoming::Ignored
        ));
    }

    #[test]
    fn thread_info_reads_session_id_and_path() {
        let result = json!({"thread": {
            "id": "019e6628-af3f-7131",
            "sessionId": "019e6628-af3f-7131",
            "cwd": "/tmp",
            "path": "/home/u/.codex/sessions/2026/05/27/rollout-x-019e6628.jsonl",
        }});
        let info = thread_info(&result).expect("thread info");
        assert_eq!(info.thread_id, "019e6628-af3f-7131");
        assert_eq!(info.cwd.as_deref(), Some("/tmp"));
        assert!(info.rollout_path.unwrap().ends_with("019e6628.jsonl"));
    }

    #[test]
    fn approval_reply_uses_correct_decision_vocabulary() {
        // command/file-change family
        let a = approval_reply(&json!(0), ApprovalKind::AcceptDecline, true);
        assert_eq!(a["id"], json!(0));
        assert_eq!(a["result"]["decision"], "accept");
        assert_eq!(
            approval_reply(&json!(0), ApprovalKind::AcceptDecline, false)["result"]["decision"],
            "decline"
        );
        // patch/exec family (ReviewDecision)
        assert_eq!(
            approval_reply(&json!(0), ApprovalKind::ApprovedDenied, true)["result"]["decision"],
            "approved"
        );
        assert_eq!(
            approval_reply(&json!(0), ApprovalKind::ApprovedDenied, false)["result"]["decision"],
            "denied"
        );
    }

    #[test]
    fn initialize_declares_capabilities_and_handshake() {
        let init = initialize_req();
        assert_eq!(init["method"], "initialize");
        assert_eq!(init["params"]["capabilities"]["experimentalApi"], false);
        assert_eq!(init["params"]["capabilities"]["requestAttestation"], false);
        assert_eq!(init["params"]["clientInfo"]["name"], "cctui");

        let done = initialized_notification();
        assert_eq!(done["method"], "initialized");
        assert!(done.get("id").is_none(), "initialized is a notification, not a request");
    }

    #[test]
    fn record_codex_version_extracts_from_user_agent() {
        let resp = json!({
            "id": 1,
            "result": {
                "userAgent": "cctui/0.144.1 (Ubuntu 24.4.0; x86_64) xterm-256color (cctui; 0.0.0)",
                "platformOs": "linux",
            },
        });
        assert_eq!(record_codex_version(&resp).as_deref(), Some("0.144.1"));

        let missing = json!({"id": 1, "result": {"platformOs": "linux"}});
        assert_eq!(record_codex_version(&missing), None);
    }

    #[test]
    fn request_builders_shape() {
        assert_eq!(initialize_req()["method"], "initialize");
        assert_eq!(thread_start_req("/tmp")["params"]["cwd"], "/tmp");
        let resume = thread_resume_req("tid", "/repo");
        assert_eq!(resume["method"], "thread/resume");
        assert_eq!(resume["params"]["threadId"], "tid");
        assert_eq!(resume["params"]["cwd"], "/repo");
        let fork = thread_fork_req("parent-tid", "/repo");
        assert_eq!(fork["method"], "thread/fork");
        assert_eq!(fork["params"]["threadId"], "parent-tid");
        assert_eq!(fork["params"]["cwd"], "/repo");
        let rename = thread_name_set_req(101, "tid", "build fix");
        assert_eq!(rename["method"], "thread/name/set");
        assert_eq!(rename["id"], 101);
        assert_eq!(rename["params"]["threadId"], "tid");
        assert_eq!(rename["params"]["name"], "build fix");
        let turn = turn_start_req(100, "tid", "hello", &[], None, None);
        assert_eq!(turn["params"]["threadId"], "tid");
        assert_eq!(turn["params"]["input"][0]["text"], "hello");
    }

    #[test]
    fn turn_start_req_carries_only_provided_overrides() {
        // No override — model/effort keys absent so codex keeps its defaults.
        let plain = turn_start_req(100, "tid", "hi", &[], None, None);
        assert!(plain["params"].get("model").is_none());
        assert!(plain["params"].get("effort").is_none());
        // Both overrides ride the turn (CCT-635 per-turn model change).
        let both = turn_start_req(101, "tid", "hi", &[], Some("gpt-5-codex"), Some("high"));
        assert_eq!(both["method"], "turn/start");
        assert_eq!(both["params"]["model"], "gpt-5-codex");
        assert_eq!(both["params"]["effort"], "high");
        // Model only — effort key must be absent.
        let model_only = turn_start_req(102, "tid", "hi", &[], Some("gpt-5-codex"), None);
        assert_eq!(model_only["params"]["model"], "gpt-5-codex");
        assert!(model_only["params"].get("effort").is_none());
    }

    #[test]
    fn turn_input_items_sends_images_native_and_files_in_text() {
        let attachments = vec![
            "/tmp/cctui-uploads/s/diagram.png".to_owned(),
            "/tmp/cctui-uploads/s/report.pdf".to_owned(),
            "/tmp/cctui-uploads/s/photo.JPEG".to_owned(),
        ];
        let items = turn_input_items("look at these", &attachments);
        // Text item first: prompt plus a listing of the non-image file only.
        assert_eq!(items[0]["type"], "text");
        let text = items[0]["text"].as_str().unwrap();
        assert!(text.contains("look at these"));
        assert!(text.contains("report.pdf"));
        assert!(!text.contains("diagram.png"), "images are native, not text paths");
        // Both images become localImage inputs carrying their local paths.
        let images: Vec<&Value> =
            items.iter().filter(|i| i["type"] == "localImage").collect();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0]["path"], "/tmp/cctui-uploads/s/diagram.png");
        assert_eq!(images[1]["path"], "/tmp/cctui-uploads/s/photo.JPEG");
    }

    #[test]
    fn turn_input_items_image_only_prompt_has_no_empty_text_gap() {
        // An image-only spawn (no prompt text) still yields a valid input array:
        // just the localImage item, no stray empty text item.
        let items = turn_input_items("", &["/tmp/s/shot.png".to_owned()]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "localImage");
        // No attachments and no text falls back to a single empty text item.
        let empty = turn_input_items("", &[]);
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0]["type"], "text");
    }

    #[tokio::test]
    async fn record_model_override_updates_state_and_acks() {
        let registry = SessionRegistry::default();
        registry.lock().await.insert(
            "tid".to_owned(),
            SessionRecord {
                cfg: AppServerConfig::default(),
                cwd: "/tmp".to_owned(),
                name: None,
                env: std::collections::BTreeMap::new(),
            },
        );
        let (tx, mut rx) = mpsc::channel(8);
        let mut model = None;
        let mut effort = None;
        let command_id = Uuid::new_v4();
        record_model_override(
            &mut model,
            &mut effort,
            Some("gpt-5-codex"),
            Some("high"),
            "tid",
            &tx,
            &registry,
            Some(command_id),
        )
        .await;
        // Override recorded for the next turn/start.
        assert_eq!(model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(effort.as_deref(), Some("high"));
        // Durable cfg folded in for a later resume's `-c` flags.
        let rec = registry.lock().await.get("tid").cloned().unwrap();
        assert_eq!(rec.cfg.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(rec.cfg.reasoning_effort.as_deref(), Some("high"));
        // Chip Status then a truthful ok CommandResult.
        let status = rx.recv().await.unwrap();
        assert!(matches!(status, AdapterEvent::Status { model: Some(m), .. } if m == "gpt-5-codex"));
        let ack = rx.recv().await.unwrap();
        assert!(matches!(ack, AdapterEvent::CommandResult { ok: true, command_id: c, .. } if c == command_id));
    }

    #[test]
    fn set_model_is_resumable() {
        assert!(
            SessionCommand::SetModel { model: Some("m".into()), effort: None, command_id: None }
                .is_resumable()
        );
    }

    #[tokio::test]
    async fn route_delivers_to_live_sender() {
        let live = LiveSessionRegistry::default();
        let registry = SessionRegistry::default();
        let (tx, mut rx) = mpsc::channel(1);
        live.lock().await.insert("tid".to_owned(), tx);
        registry.lock().await.insert(
            "tid".to_owned(),
            SessionRecord {
                cfg: AppServerConfig::default(),
                cwd: "/tmp".to_owned(),
                name: Some("n".to_owned()),
                env: std::collections::BTreeMap::new(),
            },
        );

        let action = route_or_prepare_resume(
            &live,
            &registry,
            "tid",
            SessionCommand::Send { text: "hi".to_owned() },
        )
        .await;
        assert!(matches!(action, RouteAction::Delivered));
        assert!(matches!(rx.recv().await, Some(SessionCommand::Send { text }) if text == "hi"));
    }

    #[tokio::test]
    async fn route_prepares_resume_when_live_sender_is_closed() {
        let live = LiveSessionRegistry::default();
        let registry = SessionRegistry::default();
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        live.lock().await.insert("tid".to_owned(), tx);
        registry.lock().await.insert(
            "tid".to_owned(),
            SessionRecord {
                cfg: AppServerConfig::default(),
                cwd: "/repo".to_owned(),
                name: Some("stale".to_owned()),
                env: std::collections::BTreeMap::new(),
            },
        );

        let action = route_or_prepare_resume(
            &live,
            &registry,
            "tid",
            SessionCommand::Rename { name: "new".to_owned() },
        )
        .await;
        match action {
            RouteAction::Resume { record, command: SessionCommand::Rename { name } } => {
                assert_eq!(record.cwd, "/repo");
                assert_eq!(record.name.as_deref(), Some("stale"));
                assert_eq!(name, "new");
            }
            other => panic!("expected resume action, got {other:?}"),
        }
        assert!(!live.lock().await.contains_key("tid"));
    }

    #[tokio::test]
    async fn route_missing_without_durable_record() {
        let live = LiveSessionRegistry::default();
        let registry = SessionRegistry::default();
        let action = route_or_prepare_resume(
            &live,
            &registry,
            "missing",
            SessionCommand::Send { text: "hi".to_owned() },
        )
        .await;
        assert!(matches!(action, RouteAction::Missing));
    }

    #[tokio::test]
    #[ignore = "requires `codex` installed locally; run with `--ignored`"]
    async fn real_codex_handshake_emits_session_started() {
        let (tx, mut rx) = mpsc::channel(64);
        let live = LiveSessionRegistry::default();
        let registry = SessionRegistry::default();
        let shutdown = CancellationToken::new();
        let session = CodexSession::new_fresh(
            AppServerConfig::default(),
            "/tmp".to_string(),
            std::collections::BTreeMap::new(),
            None, // no prompt → no turn/start, so no model auth needed
            None,
            Vec::new(),
            None,
            tx,
            live,
            registry.clone(),
            shutdown.clone(),
        );
        let handle = tokio::spawn(session.run());
        let evt = tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv())
            .await
            .expect("timed out waiting for SessionStarted")
            .expect("event channel closed");
        match evt {
            AdapterEvent::SessionStarted { local_id, meta } => {
                assert!(!local_id.is_empty(), "session id should be the rollout uuid");
                assert_eq!(meta.working_dir.as_deref(), Some("/tmp"));
                assert!(registry.lock().await.contains_key(&local_id), "session must register");
            }
            other => panic!("expected SessionStarted, got {other:?}"),
        }
        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    #[test]
    fn config_overrides_from_value() {
        let cfg = AppServerConfig::from_value(&json!({
            "codex_bin": "/opt/codex", "approval_policy": "never",
            "sandbox_mode": "danger-full-access",
        }));
        assert_eq!(cfg.bin, "/opt/codex");
        assert_eq!(cfg.approval_policy, "never");
        assert_eq!(cfg.sandbox_mode, "danger-full-access");
        // Default sandbox_mode is the safe, sandboxed mode.
        assert_eq!(AppServerConfig::default().sandbox_mode, "workspace-write");
    }

    #[test]
    fn config_overrides_never_enable_fast_mode() {
        // CCT-339: assert the COMPLETE set of `-c` knobs cctui sets — Fast mode
        // must never sneak in, on a default spawn or with model/effort set.
        for cfg in [
            AppServerConfig::default(),
            AppServerConfig {
                reasoning_effort: Some("high".to_owned()),
                model: Some("gpt-5-codex".to_owned()),
                ..AppServerConfig::default()
            },
        ] {
            let overrides = cfg.config_overrides();
            let keys: Vec<&str> = overrides.iter().map(|(k, _)| k.as_str()).collect();
            assert!(
                !keys.iter().any(|k| k.to_lowercase().contains("fast")),
                "no fast-mode knob may be set, got {keys:?}"
            );
            // Only the four known, intentional knobs are ever set.
            for k in &keys {
                assert!(
                    matches!(
                        *k,
                        "approval_policy" | "sandbox_mode" | "model_reasoning_effort" | "model"
                    ),
                    "unexpected codex config knob {k:?}"
                );
            }
        }
    }

    #[test]
    fn config_overrides_default_and_with_quality_knobs() {
        let base = AppServerConfig::default().config_overrides();
        assert_eq!(base.len(), 2);
        assert!(base.contains(&("approval_policy".to_owned(), "untrusted".to_owned())));
        assert!(base.contains(&("sandbox_mode".to_owned(), "workspace-write".to_owned())));

        let with = AppServerConfig {
            reasoning_effort: Some("high".to_owned()),
            model: Some("gpt-5-codex".to_owned()),
            ..AppServerConfig::default()
        }
        .config_overrides();
        assert!(with.contains(&("model_reasoning_effort".to_owned(), "high".to_owned())));
        assert!(with.contains(&("model".to_owned(), "gpt-5-codex".to_owned())));
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
    fn token_usage_without_turn_id_is_ignored() {
        let v = json!({"method":"thread/tokenUsage/updated","params":{
            "tokenUsage":{"last":{"inputTokens":1}}}});
        assert!(matches!(classify("t", &v), Incoming::Ignored));
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

    // --- CCT-631: correlated JSON-RPC outcomes -----------------------------

    #[test]
    fn pending_rpcs_resolves_success_response() {
        let mut table = PendingRpcs::default();
        table.insert(100, "turn/start", None, Instant::now() + RPC_TIMEOUT);
        let resp = json!({"id": 100, "result": {"turn": {"id": "t1"}}});
        let (pending, outcome) = table.resolve(100, &resp).expect("pending entry");
        assert_eq!(pending.method, "turn/start");
        assert_eq!(outcome.unwrap().pointer("/turn/id").and_then(Value::as_str), Some("t1"));
        assert!(table.is_empty());
        assert!(table.resolve(100, &resp).is_none(), "entry is one-shot");
    }

    #[test]
    fn pending_rpcs_propagates_error_response() {
        let mut table = PendingRpcs::default();
        let cid = Uuid::new_v4();
        table.insert(2, "thread/start", Some(cid), Instant::now() + HANDSHAKE_TIMEOUT);
        let resp = json!({"id": 2, "error": {"code": -32600, "message": "bad thread", "data": {"hint": "x"}}});
        let (pending, outcome) = table.resolve(2, &resp).expect("pending entry");
        assert_eq!(pending.command_id, Some(cid));
        assert!(pending.is_handshake());
        let err = outcome.unwrap_err();
        assert!(err.contains("-32600"), "{err}");
        assert!(err.contains("bad thread"), "{err}");
        assert!(err.contains("hint"), "{err}");
    }

    #[test]
    fn pending_rpcs_expires_only_past_deadline() {
        let mut table = PendingRpcs::default();
        let now = Instant::now();
        table.insert(1, "turn/start", None, now + Duration::from_secs(5));
        table.insert(2, "turn/interrupt", Some(Uuid::new_v4()), now + Duration::from_secs(60));
        let expired = table.expire(now + Duration::from_secs(30));
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, 1);
        assert_eq!(expired[0].1.method, "turn/start");
        assert!(!table.is_empty());
        assert!(table.expire(now + Duration::from_secs(120)).len() == 1);
        assert!(table.is_empty());
    }

    #[test]
    fn pending_rpcs_drain_cancels_everything_on_process_exit() {
        let mut table = PendingRpcs::default();
        let now = Instant::now();
        table.insert(1, "initialize", None, now + RPC_TIMEOUT);
        table.insert(100, "turn/start", None, now + RPC_TIMEOUT);
        table.insert(101, "turn/interrupt", Some(Uuid::new_v4()), now + RPC_TIMEOUT);
        let drained = table.drain();
        assert_eq!(drained.len(), 3);
        assert!(table.is_empty());
        assert_eq!(drained.iter().filter(|(_, p)| p.command_id.is_some()).count(), 1);
    }

    #[test]
    fn response_outcome_shapes() {
        assert_eq!(
            response_outcome(&json!({"id": 1, "result": {"ok": 1}})).unwrap(),
            json!({"ok": 1})
        );
        assert_eq!(response_outcome(&json!({"id": 1})).unwrap(), Value::Null);
        let err = response_outcome(&json!({"id": 1, "error": {"message": "nope"}})).unwrap_err();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn handshake_methods_are_flagged() {
        for m in ["initialize", "thread/start", "thread/resume", "thread/fork"] {
            let p = PendingRpc { method: m.to_owned(), command_id: None, deadline: Instant::now() };
            assert!(p.is_handshake(), "{m}");
        }
        let p = PendingRpc {
            method: "turn/start".to_owned(),
            command_id: None,
            deadline: Instant::now(),
        };
        assert!(!p.is_handshake());
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
    fn successful_turn_completed_stays_ignored() {
        let v = json!({"method": "turn/completed", "params": {"threadId": "t", "turn": {
            "id": "u", "items": [], "status": "completed"}}});
        assert!(matches!(classify("t", &v), Incoming::Ignored));
    }

    // --- CCT-634: active-turn routing via turn/steer ------------------------

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
        assert_eq!(
            turn_lifecycle(&json!({"method": "thread/status/changed", "params": {}})),
            None
        );
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
        assert_eq!(prompt_dispatch(&active), PromptDispatch::Steer { turn_id: "turn-9".to_owned() });
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

    #[test]
    fn turn_steer_req_shape() {
        let req = turn_steer_req(100, "tid", "turn-1", "keep going", &[]);
        assert_eq!(req["method"], "turn/steer");
        assert_eq!(req["id"], 100);
        assert_eq!(req["params"]["threadId"], "tid");
        assert_eq!(req["params"]["expectedTurnId"], "turn-1");
        assert_eq!(req["params"]["input"][0]["type"], "text");
        assert_eq!(req["params"]["input"][0]["text"], "keep going");
    }
}
