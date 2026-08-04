//! Codex `app-server` driver.
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
//! with `thread/resume { threadId }` before the next `turn/start`.
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
use cctui_proto::codex_catalog::{CodexModel, CodexModelCatalog};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{contract, model_list};

/// Outbound request id seeds. The handshake uses fixed ids so the driver
/// can recognise the responses it is waiting for; everything after is
/// monotonic from [`Self::RUN_BASE`].
const ID_INITIALIZE: i64 = 1;
const ID_THREAD_START: i64 = 2;
const RUN_BASE: i64 = 100;

/// How many trailing `codex app-server` stderr lines to retain for crash
/// diagnostics. The app-server logs to stderr; when it dies
/// unexpectedly these lines are the only clue why, so they are folded into
/// the [`EndReason::Crashed`] detail instead of being discarded to
/// `/dev/null`.
const STDERR_RING: usize = 40;

const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// `thread/resume` of a long transcript can legitimately exceed the normal
/// RPC deadline, so handshake requests get a longer one.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_mins(2);

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
    /// `item/tool/requestUserInput`: codex's `AskUserQuestion`.
    /// `question_ids` are needed to key the [`ToolRequestUserInputResponse`].
    Question { rpc_id: Value, question: String, questions: Value, question_ids: Vec<String> },
    /// A server→client request cctui cannot fulfil. It carries an `id`, so
    /// leaving it unanswered blocks codex forever; `reply` is the decline/error
    /// to write back immediately instead.
    Decline { reply: Value },
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
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    let rpc_id = v.get("id").cloned().unwrap_or(Value::Null);
    let (kind, tool) = match method {
        "item/commandExecution/requestApproval" => (ApprovalKind::AcceptDecline, "shell"),
        "item/fileChange/requestApproval" => (ApprovalKind::AcceptDecline, "file_change"),
        "applyPatchApproval" => (ApprovalKind::ApprovedDenied, "apply_patch"),
        "execCommandApproval" => (ApprovalKind::ApprovedDenied, "shell"),
        "item/tool/requestUserInput" => return classify_user_input(rpc_id, &params),
        // Known-but-unsupported requests get their schema-correct decline reply
        // so codex isn't blocked forever; everything else (dynamic
        // tool call, token refresh, attestation, future methods) gets a generic
        // method-not-supported error.
        "mcpServer/elicitation/request" => {
            return Incoming::Decline { reply: elicitation_decline(&rpc_id) };
        }
        "item/permissions/requestApproval" => {
            return Incoming::Decline { reply: permissions_decline(&rpc_id) };
        }
        _ => return Incoming::Decline { reply: request_not_supported(&rpc_id, method) },
    };
    let request_id = params
        .get("itemId")
        .and_then(Value::as_str)
        .map_or_else(|| format!("codex-approval-{rpc_id}"), std::string::ToString::to_string);
    Incoming::Approval { rpc_id, request_id, tool: tool.to_string(), kind, input: params }
}

/// Classify an `item/tool/requestUserInput` request into a [`Incoming::Question`].
/// Flattens the per-question `header`/`question` into a single text
/// (for the flattened claude field) while passing the raw `questions` array
/// through for the interactive card, and collects the question ids the answer
/// must be keyed on.
fn classify_user_input(rpc_id: Value, params: &Value) -> Incoming {
    let questions = params.get("questions").cloned().unwrap_or_else(|| json!([]));
    let list = questions.as_array().cloned().unwrap_or_default();
    let question_ids: Vec<String> = list
        .iter()
        .filter_map(|q| q.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let question = list
        .iter()
        .filter_map(|q| {
            let text = q.get("question").and_then(Value::as_str)?;
            Some(
                q.get("header")
                    .and_then(Value::as_str)
                    .filter(|h| !h.is_empty())
                    .map_or_else(|| text.to_owned(), |header| format!("{header} — {text}")),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Incoming::Question { rpc_id, question, questions, question_ids }
}

/// Decline an MCP `elicitation/create` request. cctui does not render
/// the typed form, so it answers `decline` — the schema's neutral "user did not
/// provide input" action — rather than leaving the turn blocked.
fn elicitation_decline(rpc_id: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": rpc_id, "result": {"action": "decline"}})
}

/// Decline a sandbox-permission elevation request. Granting nothing
/// (an empty `GrantedPermissionProfile`) is the deny: codex continues the turn
/// without the extra permissions instead of waiting on a reply that never comes.
fn permissions_decline(rpc_id: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": rpc_id, "result": {"permissions": {}}})
}

/// Reject a server request method cctui does not implement with a JSON-RPC
/// method-not-found error, so codex fails the request fast instead of blocking.
fn request_not_supported(rpc_id: &Value, method: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": rpc_id,
        "error": {"code": -32601, "message": format!("cctui does not support server request {method}")},
    })
}

/// Reply to an `item/tool/requestUserInput` request. The single free
/// text answer is mapped onto every question id — requestUserInput forms are
/// single-question in practice, and codex feeds the string straight to the tool.
fn user_input_reply(rpc_id: &Value, question_ids: &[String], answer: &str) -> Value {
    let answers: serde_json::Map<String, Value> =
        question_ids.iter().map(|id| (id.clone(), json!({"answers": [answer]}))).collect();
    json!({"jsonrpc": "2.0", "id": rpc_id, "result": {"answers": answers}})
}

fn map_notification(local_id: &str, method: &str, v: &Value) -> Incoming {
    match method {
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
/// [`AdapterEvent::Status`] carrying the turn error message.
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
        "commandExecution"
        | "fileChange"
        | "mcpToolCall"
        | "dynamicToolCall"
        | "collabAgentToolCall"
        | "webSearch"
        | "imageView"
        | "imageGeneration" => AdapterEvent::ToolUse { local_id: local_id.to_owned(), payload },
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
/// tracked separately.
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

/// Build the documented `initialize` request. Capabilities are
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

/// Fork an existing thread into a brand-new one seeded from its history.
/// The app-server returns a fresh `thread` (its own id) just like
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

/// A native codex thread lifecycle operation. Each maps to a single
/// JSON-RPC method taking `{ threadId }`. Archive/unarchive are wired to the
/// CCTUI archive/reopen actions; `Delete` implements the third native op for
/// parity (no CCTUI destructive-delete action wires to it yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleOp {
    Archive,
    Unarchive,
    #[allow(dead_code)]
    Delete,
}

impl LifecycleOp {
    #[must_use]
    const fn method(self) -> &'static str {
        match self {
            Self::Archive => "thread/archive",
            Self::Unarchive => "thread/unarchive",
            Self::Delete => "thread/delete",
        }
    }
}

fn thread_lifecycle_req(id: i64, op: LifecycleOp, thread_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": op.method(),
        "params": {"threadId": thread_id},
    })
}

/// Whether a `thread/{archive,unarchive,delete}` JSON-RPC error can be treated
/// as success for idempotency: the thread is already in the target
/// state or no longer exists, so CCTUI and native lifecycle state can't wedge
/// each other. Matched on the codex error text since the app-server exposes no
/// stable machine codes for these.
#[must_use]
pub fn is_idempotent_lifecycle_error(op: LifecycleOp, err: &str) -> bool {
    let e = err.to_lowercase();
    // A missing thread makes any lifecycle op a no-op success.
    let missing = e.contains("not found")
        || e.contains("no such")
        || e.contains("does not exist")
        || e.contains("doesn't exist")
        || e.contains("unknown thread")
        || e.contains("no thread");
    // Already in the requested terminal state.
    let already = match op {
        LifecycleOp::Archive => e.contains("already archived"),
        LifecycleOp::Unarchive => e.contains("already unarchived") || e.contains("not archived"),
        LifecycleOp::Delete => e.contains("already deleted"),
    };
    missing || already
}

/// Run a native codex thread lifecycle op via a short-lived stdio
/// `codex app-server`, mirroring the one-shot pattern the
/// [`super::thread_list`] inventory poll uses. Spawns the app-server, sends
/// `initialize` → `initialized` → the lifecycle RPC, correlates the response by
/// id, and reaps the process. Idempotent: an "already in target state" /
/// "thread missing" error resolves as success ([`is_idempotent_lifecycle_error`])
/// so CCTUI and native lifecycle state can't wedge each other. No gateway env is
/// needed — no turn is started.
pub async fn run_thread_lifecycle(
    app: &AppServerConfig,
    thread_id: &str,
    op: LifecycleOp,
) -> Result<()> {
    let mut cmd = Command::new(&app.bin);
    cmd.arg("app-server")
        // No turn is started, so sandbox mode only matters because codex
        // refuses to boot when it cannot create the bwrap namespace on some
        // kernels — pass the configured (host-default) mode through.
        .arg("-c")
        .arg(format!("sandbox_mode=\"{}\"", app.sandbox_mode))
        .env("PATH", crate::childenv::child_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::childenv::ScrubChildEnv::scrub_child_env(&mut cmd);
    let mut child = cmd.spawn()?;
    let mut stdin = child.stdin.take().context("codex app-server stdin unavailable")?;
    let stdout = child.stdout.take().context("codex app-server stdout unavailable")?;

    let req_id = RUN_BASE;
    let outcome = tokio::time::timeout(RPC_TIMEOUT, async {
        let mut lines = BufReader::new(stdout).lines();
        write_json(&mut stdin, &initialize_req()).await?;
        write_json(&mut stdin, &initialized_notification()).await?;
        write_json(&mut stdin, &thread_lifecycle_req(req_id, op, thread_id)).await?;
        while let Some(line) = lines.next_line().await? {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(trimmed) else { continue };
            if v.get("id").and_then(Value::as_i64) == Some(req_id) {
                return anyhow::Ok(response_outcome(&v));
            }
        }
        anyhow::bail!("codex {} response not received before EOF", op.method())
    })
    .await;

    // Close stdin and reap regardless of how the read went.
    drop(stdin);
    let _ = child.start_kill();
    let _ = child.wait().await;

    match outcome {
        Err(_) => anyhow::bail!("codex {} timed out", op.method()),
        Ok(Err(e)) => Err(e),
        Ok(Ok(Ok(_))) => Ok(()),
        Ok(Ok(Err(msg))) => {
            if is_idempotent_lifecycle_error(op, &msg) {
                tracing::info!(
                    %thread_id,
                    op = op.method(),
                    "codex lifecycle op idempotent no-op: {msg}"
                );
                Ok(())
            } else {
                Err(anyhow::anyhow!(msg))
            }
        }
    }
}

/// Build the `input` array for a turn. Staged image attachments ride
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

/// Build a `turn/start`. An in-place model/effort change rides here
/// as a per-turn override that codex promotes to the later default — the stable
/// alternative to the `experimentalApi`-gated `thread/settings/update`. Only
/// set fields are sent so an unchanged setting keeps codex's own default.
/// Staged attachments become native image / path-in-text inputs.
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

/// Steer a user message into the currently active turn. Unlike
/// `turn/start` — which codex rejects while a turn is in flight — `turn/steer`
/// appends the input to the running turn. `expectedTurnId` is a precondition:
/// the request fails if it no longer matches the active turn (it just ended),
/// which the driver recovers from by falling back to `turn/start`. Attachments
/// build the same native image / path-in-text inputs as a start.
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

/// Reply to a server-issued approval request. `rpc_id` must be the exact
/// `id` value from the request; `kind` selects the decision vocabulary.
fn approval_reply(rpc_id: &Value, kind: ApprovalKind, allow: bool) -> Value {
    json!({"jsonrpc": "2.0", "id": rpc_id, "result": {"decision": kind.decision(allow)}})
}

/// One outstanding outbound JSON-RPC request.
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

/// Correlation table for outbound JSON-RPC requests, keyed by request id.
/// The driver inserts before each write, resolves on the matching
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

    /// Methods of every outstanding request (diagnostics).
    #[must_use]
    pub fn pending_methods(&self) -> Vec<String> {
        self.inner.values().map(|p| p.method.clone()).collect()
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
/// spawn error) resolves it as a failure instead. One-shot: the
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
    /// Interrupt the in-flight turn but KEEP the session alive:
    /// sends `turn/interrupt` WITHOUT terminating the app-server, so the
    /// thread stays resumable. Distinct from `Kill`, which interrupts *and*
    /// terminates the child. `command_id` correlates the `turn/interrupt`
    /// JSON-RPC outcome back to an [`AdapterEvent::CommandResult`].
    Interrupt { command_id: Option<Uuid> },
    /// Change the model and/or reasoning effort of the running thread in place:
    /// records the override so the next `turn/start` carries it (a
    /// stable per-turn override codex promotes to the later default),
    /// and echoes the resolved values back via [`AdapterEvent::Status`] so the
    /// webui chip updates live. `command_id` correlates the outcome back as an
    /// [`AdapterEvent::CommandResult`].
    SetModel { model: Option<String>, effort: Option<String>, command_id: Option<Uuid> },
    /// Gather a point-in-time snapshot of the live driver's internal state for
    /// the adapter-neutral diagnose report and return it on `reply`.
    Diagnose { reply: mpsc::Sender<CodexLiveSnapshot> },
}

impl SessionCommand {
    #[must_use]
    pub const fn is_resumable(&self) -> bool {
        matches!(self, Self::Send { .. } | Self::Rename { .. } | Self::SetModel { .. })
    }
}

/// Point-in-time snapshot of a live codex session's internal driver state,
/// gathered on demand for the diagnose report.
#[derive(Debug, Clone, Default)]
pub struct CodexLiveSnapshot {
    pub codex_version: Option<String>,
    pub pid: Option<u32>,
    pub active_turn_id: Option<String>,
    pub pending_rpc_methods: Vec<String>,
    pub last_protocol_error: Option<String>,
    pub rollout_path: Option<String>,
    pub rollout_size_bytes: Option<u64>,
}

/// Live command registry: `local_id` → command sender for the owning app-server
/// task. Senders disappear when the app-server exits; the durable
/// [`SessionRegistry`] below stays so a later reply can revive the thread.
pub type LiveSessionRegistry = Arc<Mutex<HashMap<String, mpsc::Sender<SessionCommand>>>>;

/// Durable-in-daemon metadata for cctui-owned Codex threads. This is not a
/// process handle; it is the minimum launch context needed to call
/// `thread/resume` after a clean app-server exit. The log-tail also
/// uses this map as the ownership set so it does not double-ingest these
/// rollout files while they are hibernated.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub cfg: AppServerConfig,
    pub cwd: String,
    pub name: Option<String>,
    /// Resolved launch-time env — chiefly the gateway-routing
    /// credential pulled from the server's durable `sessions.account_id`
    /// binding. Stored so a resume relaunches the codex app-server with the
    /// same gateway env instead of starting env-less and 401ing (the codex
    /// analogue of the claude cold-launch bug).
    pub env: std::collections::BTreeMap<String, String>,
}

/// `local_id` → cctui-owned Codex thread metadata.
pub type SessionRegistry = Arc<Mutex<HashMap<String, SessionRecord>>>;

// `Resume` carries a full `SessionRecord` (now incl. the launch env);
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
    /// Sandbox mode passed via `-c sandbox_mode=...`. `"read-only"`
    /// and `"workspace-write"` wrap commands in bubblewrap; on a host whose
    /// kernel forbids unprivileged user namespaces those fail to launch, so a
    /// per-host default of `"danger-full-access"` (no sandbox) is required
    /// there. Overridable per-spawn via the full-access toggle.
    pub sandbox_mode: String,
    /// Reasoning effort passed via `-c model_reasoning_effort=...`
    /// (codex: `minimal`/`low`/`medium`/`high`). `None` keeps the codex
    /// default. Set per-spawn from the spawn request.
    pub reasoning_effort: Option<String>,
    /// Model passed via `-c model="…"`. `None` keeps the codex
    /// default. Set per-spawn from the spawn request.
    pub model: Option<String>,
    /// Whether to refresh the codex model catalog on session start
    /// by issuing `model/list` over this session's authenticated app-server
    /// connection. `false` (`model_catalog = false`) disables the refresh.
    pub model_catalog: bool,
}

impl Default for AppServerConfig {
    fn default() -> Self {
        Self {
            bin: "codex".to_string(),
            approval_policy: "untrusted".to_string(),
            sandbox_mode: "workspace-write".to_string(),
            reasoning_effort: None,
            model: None,
            model_catalog: true,
        }
    }
}

impl AppServerConfig {
    /// The `-c key="value"` overrides passed to `codex app-server` for a spawn.
    /// This is the COMPLETE set of config knobs cctui sets — kept as a single
    /// function so the "Fast mode is never silently enabled" guarantee
    /// is testable. Codex's "Fast mode" is a separate per-thread setting; cctui
    /// never sets it here (no `fast`/`model_fast`/`reasoning_fast` key), so a
    /// spawned session always uses the user's normal model/effort, never the
    /// degraded fast path. Reasoning effort and model are the only opt-in
    /// quality knobs, both surfaced explicitly in the spawn picker (303).
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
        cfg.model_catalog = model_list::catalog_enabled(v);
        cfg
    }
}

/// The `-c` overrides that route codex's model provider through the cctui
/// gateway. Codex does NOT honor `OPENAI_BASE_URL`/`OPENAI_API_KEY`
/// from the environment alone: launched with only those env vars it POSTs to
/// api.openai.com with no Authorization header and 401s. It reads them solely
/// through a `model_providers` entry — `base_url` inlined here, the bearer via
/// `env_key` from the launch env at request time. Mirrors the worker
/// entrypoint's `phase_codex_config`, which fixed the same failure
/// for k8s workers by writing this block into config.toml. Empty when either
/// var is absent (an unbound session keeps codex's default provider).
#[must_use]
pub fn gateway_provider_overrides(
    env: &std::collections::BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let (Some(base_url), Some(_key)) = (env.get("OPENAI_BASE_URL"), env.get("OPENAI_API_KEY"))
    else {
        return Vec::new();
    };
    vec![
        ("model_provider".to_owned(), "cctui".to_owned()),
        ("model_providers.cctui.name".to_owned(), "cctui-gateway".to_owned()),
        ("model_providers.cctui.base_url".to_owned(), base_url.clone()),
        ("model_providers.cctui.env_key".to_owned(), "OPENAI_API_KEY".to_owned()),
        ("model_providers.cctui.wire_api".to_owned(), "responses".to_owned()),
    ]
}

#[derive(Debug, Clone)]
enum SessionLaunch {
    Fresh {
        prompt: Option<String>,
        name: Option<String>,
        /// Staged spawn-attachment paths, fed into the first turn.
        attachments: Vec<String>,
    },
    Resume {
        thread_id: String,
        initial_commands: Vec<SessionCommand>,
    },
    /// Fork a parent thread into a new one seeded from its history.
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
    /// Launch-time env merged onto the `codex app-server` child process.
    /// Holds the gateway-routing credential resolved at spawn /
    /// fork / resume; see [`SessionRecord::env`].
    env: std::collections::BTreeMap<String, String>,
    launch: SessionLaunch,
    /// Spawn/fork correlation id: resolved as an
    /// [`AdapterEvent::CommandResult`] only once the launch outcome is known.
    command_id: Option<Uuid>,
    /// Server-pre-minted session id, echoed on `SessionStarted` so childwatch
    /// can bind the thread codex mints to the `CctuiAgent` waiter.
    spawn_key: Option<String>,
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
        spawn_key: Option<String>,
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
            spawn_key,
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
            spawn_key: None,
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
            spawn_key: None,
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
        for (key, value) in gateway_provider_overrides(&self.env) {
            cmd.arg("-c").arg(format!("{key}=\"{value}\""));
        }
        // Forward the resolved launch env — chiefly the gateway
        // credential pulled from the server's `sessions.account_id` binding —
        // onto the app-server child, so a session bound to a named gateway
        // account routes through it instead of hitting the default upstream and
        // 401ing. Applied before `PATH` below so the launchd PATH fix wins even
        // if the resolved env carried a `PATH` of its own. The fail-closed
        // contract (refuse an account-bound launch with empty gateway env) is
        // enforced upstream in the adapter command pump;.
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        crate::childenv::ScrubChildEnv::scrub_child_env(&mut cmd);
        let mut child = cmd
            .current_dir(cwd_path)
            // launchd strips `PATH` down to a minimal set that omits
            // `/opt/homebrew/bin`, so a bare `codex` fails ENOENT.
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
        let mut rollout_path: Option<String> = None;
        let mut last_protocol_error: Option<String> = None;
        let mut next_id = RUN_BASE;
        // request_id (surfaced to TUI) → (rpc_id echoed to codex, decision kind).
        let mut pending_approvals: HashMap<String, (Value, ApprovalKind)> = HashMap::new();
        // Parked `item/tool/requestUserInput` requests: the next user
        // reply answers the oldest one (codex blocks the turn on it) rather than
        // starting a fresh turn.
        let mut pending_questions: VecDeque<(Value, Vec<String>)> = VecDeque::new();
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<SessionCommand>(32);
        let mut registered = false;
        // Set when the session is terminated on purpose (daemon shutdown or a
        // Kill command) so the epilogue reports `Killed` rather than treating
        // the non-zero exit as a crash.
        let mut killed = false;
        let mut retry_after_hibernate: Option<SessionCommand> = None;
        let mut active_turn = ActiveTurn::default();
        let mut items = ItemAccumulator::default();
        // In-place model/effort override. A SetModel records it here;
        // every subsequent `turn/start` carries it so codex adopts it as the
        // later default. Left `None` at launch — the spawn-time `-c model=`/
        // `-c model_reasoning_effort=` flags already seed the initial turns.
        let mut override_model: Option<String> = None;
        let mut override_effort: Option<String> = None;
        let mut steer_texts: HashMap<i64, String> = HashMap::new();
        // `model/list` pages accumulated over this session's
        // authenticated connection; the counter bounds `nextCursor` following.
        let mut model_catalog: Vec<CodexModel> = Vec::new();
        let mut model_catalog_pages: usize = 0;
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
                            if let Some((rpc_id, question_ids)) = pending_questions.pop_front() {
                                let reply = user_input_reply(&rpc_id, &question_ids, &text);
                                if let Err(e) = write_json(&mut stdin, &reply).await {
                                    tracing::warn!(%e, "codex: requestUserInput answer write failed; ending session");
                                    break;
                                }
                                self.events
                                    .send(AdapterEvent::AskResolved { local_id: local_id.clone() })
                                    .await
                                    .ok();
                                continue;
                            }
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
                            // Keep-alive interrupt: abort the turn but
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
                        Some(SessionCommand::Diagnose { reply }) => {
                            let snapshot = CodexLiveSnapshot {
                                codex_version: codex_version.clone(),
                                pid: child.id(),
                                active_turn_id: active_turn.id().map(str::to_owned),
                                pending_rpc_methods: pending_rpcs.pending_methods(),
                                last_protocol_error: last_protocol_error.clone(),
                                rollout_path: rollout_path.clone(),
                                rollout_size_bytes: rollout_path
                                    .as_ref()
                                    .and_then(|p| std::fs::metadata(p).ok())
                                    .map(|m| m.len()),
                            };
                            let _ = reply.send(snapshot).await;
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
                    items.note(&value);
                    let value = items.enrich_completed(value);
                    match classify(&local_id, &value) {
                        Incoming::Response { id, value } => {
                            let Some((pending, outcome)) = pending_rpcs.resolve(id, &value) else {
                                tracing::debug!(rpc_id = id, "codex: response for unknown request id");
                                continue;
                            };
                            if let Err(ref e) = outcome {
                                last_protocol_error = Some(format!("{}: {e}", pending.method));
                            }
                            match (pending.method.as_str(), outcome) {
                        ("initialize", Ok(_)) => {
                            codex_version = record_codex_version(&value);
                            // Complete the documented handshake before any
                            // thread request: the server treats
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
                            rollout_path.clone_from(&info.rollout_path);
                            // Link a forked thread back to its parent
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
                                            "spawn_key": self.spawn_key,
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
                            // refresh the account/machine model catalog
                            // over THIS authenticated connection (the gateway
                            // credential is in env), so gateway-only machines get
                            // the current remote list instead of a stale
                            // unauthenticated fallback. Best-effort: a failure is
                            // logged, never fatal to the session.
                            if self.cfg.model_catalog {
                                pending_rpcs.insert(
                                    next_id,
                                    "model/list",
                                    None,
                                    Instant::now() + RPC_TIMEOUT,
                                );
                                if let Err(e) =
                                    write_json(&mut stdin, &model_list::model_list_req(next_id, None))
                                        .await
                                {
                                    tracing::debug!(%e, "codex: model/list write failed");
                                    pending_rpcs.resolve(next_id, &json!({}));
                                }
                                next_id += 1;
                            }
                            // Surface the configured model + reasoning effort so
                            // the session list shows them (claude gets this for
                            // free via state.json; codex has no equivalent feed).
                            // Emit when either is known.
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
                                    // staged attachments — an image-only
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
                        ("model/list", Ok(result)) => {
                            model_catalog.extend(model_list::parse_model_list(&result));
                            model_catalog_pages += 1;
                            match model_list::page_step(model_catalog_pages, &result) {
                                model_list::PageStep::Next { cursor } => {
                                    pending_rpcs.insert(
                                        next_id,
                                        "model/list",
                                        None,
                                        Instant::now() + RPC_TIMEOUT,
                                    );
                                    if let Err(e) = write_json(
                                        &mut stdin,
                                        &model_list::model_list_req(next_id, Some(&cursor)),
                                    )
                                    .await
                                    {
                                        tracing::debug!(%e, "codex: model/list page write failed");
                                        pending_rpcs.resolve(next_id, &json!({}));
                                    }
                                    next_id += 1;
                                }
                                model_list::PageStep::Done => {
                                    let catalog = CodexModelCatalog {
                                        models: std::mem::take(&mut model_catalog),
                                    };
                                    self.events
                                        .send(AdapterEvent::CodexModels { catalog })
                                        .await
                                        .ok();
                                }
                            }
                        }
                        ("model/list", Err(err)) => {
                            tracing::debug!(%err, "codex: model/list refresh failed");
                            model_catalog.clear();
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
                        Incoming::Question { rpc_id, question, questions, question_ids } => {
                            pending_questions.push_back((rpc_id, question_ids));
                            self.events
                                .send(AdapterEvent::AskQuestion {
                                    local_id: local_id.clone(),
                                    question,
                                    questions: Some(questions),
                                    preamble: None,
                                })
                                .await
                                .ok();
                        }
                        Incoming::Decline { reply } => {
                            if let Err(e) = write_json(&mut stdin, &reply).await {
                                tracing::warn!(%e, "codex: decline write failed; ending session");
                                break;
                            }
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

/// Record an in-place model/effort change. Stashes the
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
        events.send(AdapterEvent::CommandResult { command_id, ok: true, error: None }).await.ok();
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
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
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
    fn gateway_provider_overrides_route_via_gateway_when_env_bound() {
        let env: std::collections::BTreeMap<String, String> = [
            ("OPENAI_BASE_URL".to_owned(), "https://cctui.example/gateway/openai".to_owned()),
            ("OPENAI_API_KEY".to_owned(), "cctui_s_tok".to_owned()),
            ("KEEP".to_owned(), "1".to_owned()),
        ]
        .into_iter()
        .collect();
        let got = gateway_provider_overrides(&env);
        assert_eq!(
            got,
            vec![
                ("model_provider".to_owned(), "cctui".to_owned()),
                ("model_providers.cctui.name".to_owned(), "cctui-gateway".to_owned()),
                (
                    "model_providers.cctui.base_url".to_owned(),
                    "https://cctui.example/gateway/openai".to_owned()
                ),
                ("model_providers.cctui.env_key".to_owned(), "OPENAI_API_KEY".to_owned()),
                ("model_providers.cctui.wire_api".to_owned(), "responses".to_owned()),
            ]
        );
    }

    #[test]
    fn gateway_provider_overrides_empty_without_full_gateway_env() {
        let empty = std::collections::BTreeMap::new();
        assert!(gateway_provider_overrides(&empty).is_empty());
        let url_only: std::collections::BTreeMap<String, String> =
            std::iter::once(("OPENAI_BASE_URL".to_owned(), "https://x".to_owned())).collect();
        assert!(gateway_provider_overrides(&url_only).is_empty());
        let key_only: std::collections::BTreeMap<String, String> =
            std::iter::once(("OPENAI_API_KEY".to_owned(), "tok".to_owned())).collect();
        assert!(gateway_provider_overrides(&key_only).is_empty());
    }

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
    fn permissions_approval_is_declined_not_left_blocking() {
        // Sandbox-permission elevation has no simple allow/deny reply, so it is
        // declined (empty grant) rather than left hanging.
        let v = json!({"method": "item/permissions/requestApproval", "id": 1, "params": {}});
        match classify("s", &v) {
            Incoming::Decline { reply } => {
                assert_eq!(reply["id"], json!(1));
                assert_eq!(reply["result"]["permissions"], json!({}));
            }
            other => panic!("expected Decline, got {other:?}"),
        }
    }

    #[test]
    fn mcp_elicitation_is_declined() {
        let v = json!({"method": "mcpServer/elicitation/request", "id": 3,
            "params": {"serverName": "s", "threadId": "t", "message": "pick", "mode": "form"}});
        match classify("s", &v) {
            Incoming::Decline { reply } => {
                assert_eq!(reply["id"], json!(3));
                assert_eq!(reply["result"]["action"], "decline");
            }
            other => panic!("expected Decline, got {other:?}"),
        }
    }

    #[test]
    fn unknown_server_request_is_declined_with_error() {
        // A dynamic tool call / future method cctui does not implement must not
        // hang codex: it is answered with a JSON-RPC method-not-found error.
        let v = json!({"method": "item/tool/call", "id": 9, "params": {}});
        match classify("s", &v) {
            Incoming::Decline { reply } => {
                assert_eq!(reply["id"], json!(9));
                assert_eq!(reply["error"]["code"], -32601);
                assert!(reply["error"]["message"].as_str().unwrap().contains("item/tool/call"));
            }
            other => panic!("expected Decline, got {other:?}"),
        }
    }

    #[test]
    fn request_user_input_maps_to_question() {
        let v = json!({"method": "item/tool/requestUserInput", "id": 4, "params": {
        "itemId": "call_42", "threadId": "t", "turnId": "u",
        "questions": [
            {"id": "q1", "header": "Deploy", "question": "Which env?",
             "options": [{"label": "prod", "description": "production"},
                         {"label": "staging", "description": "staging"}]},
        ]}});
        match classify("sess", &v) {
            Incoming::Question { rpc_id, question, questions, question_ids } => {
                assert_eq!(rpc_id, json!(4));
                assert_eq!(question, "Deploy — Which env?");
                assert_eq!(question_ids, vec!["q1".to_owned()]);
                assert_eq!(questions[0]["options"][0]["label"], "prod");
            }
            other => panic!("expected Question, got {other:?}"),
        }
    }

    #[test]
    fn request_user_input_with_no_question_ids_still_maps() {
        let v = json!({"method": "item/tool/requestUserInput", "id": 7,
            "params": {"threadId": "t", "turnId": "u", "questions": []}});
        match classify("s", &v) {
            Incoming::Question { rpc_id, question_ids, .. } => {
                assert_eq!(rpc_id, json!(7));
                assert!(question_ids.is_empty());
            }
            other => panic!("expected Question, got {other:?}"),
        }
    }

    #[test]
    fn user_input_reply_keys_answer_by_question_id() {
        let reply =
            user_input_reply(&json!(4), &["q1".to_owned(), "q2".to_owned()], "prod, us-east");
        assert_eq!(reply["id"], json!(4));
        assert_eq!(reply["result"]["answers"]["q1"]["answers"][0], "prod, us-east");
        assert_eq!(reply["result"]["answers"]["q2"]["answers"][0], "prod, us-east");
    }

    #[test]
    fn decline_reply_builders_shape() {
        assert_eq!(elicitation_decline(&json!(1))["result"]["action"], "decline");
        assert_eq!(permissions_decline(&json!(2))["result"]["permissions"], json!({}));
        assert_eq!(request_not_supported(&json!(3), "x/y")["error"]["code"], -32601);
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
        // Both overrides ride the turn (per-turn model change).
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
        let images: Vec<&Value> = items.iter().filter(|i| i["type"] == "localImage").collect();
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
        assert!(
            matches!(status, AdapterEvent::Status { model: Some(m), .. } if m == "gpt-5-codex")
        );
        let ack = rx.recv().await.unwrap();
        assert!(
            matches!(ack, AdapterEvent::CommandResult { ok: true, command_id: c, .. } if c == command_id)
        );
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
    fn model_catalog_toggle_defaults_on_and_reads_config() {
        assert!(AppServerConfig::default().model_catalog);
        assert!(AppServerConfig::from_value(&json!({})).model_catalog);
        assert!(!AppServerConfig::from_value(&json!({"model_catalog": false})).model_catalog);
    }

    #[test]
    fn config_overrides_never_enable_fast_mode() {
        // assert the COMPLETE set of `-c` knobs cctui sets — Fast mode
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

    // --- correlated JSON-RPC outcomes -----------------------------

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
        table.insert(2, "turn/interrupt", Some(Uuid::new_v4()), now + Duration::from_mins(1));
        let expired = table.expire(now + Duration::from_secs(30));
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, 1);
        assert_eq!(expired[0].1.method, "turn/start");
        assert!(!table.is_empty());
        assert_eq!(table.expire(now + Duration::from_mins(2)).len(), 1);
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
    fn lifecycle_request_shapes() {
        for (op, method) in [
            (LifecycleOp::Archive, "thread/archive"),
            (LifecycleOp::Unarchive, "thread/unarchive"),
            (LifecycleOp::Delete, "thread/delete"),
        ] {
            let req = thread_lifecycle_req(7, op, "thread-abc");
            assert_eq!(req["jsonrpc"], "2.0");
            assert_eq!(req["id"], 7);
            assert_eq!(req["method"], method);
            assert_eq!(req["params"]["threadId"], "thread-abc");
        }
    }

    #[test]
    fn lifecycle_idempotency_maps_already_in_state() {
        assert!(is_idempotent_lifecycle_error(
            LifecycleOp::Archive,
            "codex app-server error: thread is already archived"
        ));
        assert!(is_idempotent_lifecycle_error(LifecycleOp::Unarchive, "thread is not archived"));
        assert!(is_idempotent_lifecycle_error(LifecycleOp::Unarchive, "already unarchived"));
        assert!(is_idempotent_lifecycle_error(LifecycleOp::Delete, "already deleted"));
    }

    #[test]
    fn lifecycle_idempotency_maps_missing_thread_for_every_op() {
        for op in [LifecycleOp::Archive, LifecycleOp::Unarchive, LifecycleOp::Delete] {
            assert!(is_idempotent_lifecycle_error(op, "thread not found"));
            assert!(is_idempotent_lifecycle_error(op, "No such thread: abc"));
            assert!(is_idempotent_lifecycle_error(op, "thread does not exist"));
        }
    }

    #[test]
    fn lifecycle_idempotency_rejects_real_errors() {
        assert!(!is_idempotent_lifecycle_error(
            LifecycleOp::Archive,
            "codex app-server error 500: internal error"
        ));
        assert!(!is_idempotent_lifecycle_error(LifecycleOp::Unarchive, "permission denied"));
        // An archive-specific "already" must not mask a genuine unarchive fault.
        assert!(!is_idempotent_lifecycle_error(LifecycleOp::Unarchive, "already archived"));
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

    // --- item/started + delta accumulation, new item types ---------

    const ITEM_STREAM_FIXTURE: &str = include_str!("fixtures/item_stream.jsonl");

    fn fixture_lines() -> Vec<Value> {
        ITEM_STREAM_FIXTURE
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Value>(l).expect("fixture line is valid JSON"))
            .collect()
    }

    #[test]
    fn item_started_and_deltas_are_ignored_by_classify() {
        for v in fixture_lines() {
            let method = v.get("method").and_then(Value::as_str).unwrap_or_default();
            if method == "item/started" || method.ends_with("Delta") || method.contains("/delta") {
                assert!(
                    matches!(classify("t", &v), Incoming::Ignored),
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

    #[tokio::test]
    async fn terminate_child_sigterm_stops_the_process() {
        let mut child = tokio::process::Command::new("sleep").arg("300").spawn().unwrap();
        terminate_child(&mut child, Some(SIGTERM));
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .expect("child survived SIGTERM")
            .unwrap();
        assert!(!status.success());
    }
}
