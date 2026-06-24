//! Codex `app-server` driver (CCT-98).
//!
//! Drives sessions that cctui *spawns*, as opposed to the log-tail
//! ([`super::log_tail`]) which passively observes sessions started outside
//! cctui (e.g. the Codex TUI). The two coexist: session identity is the
//! rollout id (`UUIDv7`), so an app-server-driven session and its on-disk
//! rollout file refer to the same `local_id`.
//!
//! `codex app-server` speaks newline-delimited JSON-RPC 2.0 over stdio
//! (stderr is logs). The handshake is `initialize` → `thread/start { cwd }`
//! → `turn/start { threadId, input }`. A stale cctui-owned thread is revived
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

use anyhow::{Context, Result};
use cctui_proto::adapter::{AdapterEvent, EndReason, SessionMeta};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

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
        _ => Incoming::Ignored,
    }
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

fn initialize_req() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": ID_INITIALIZE,
        "method": "initialize",
        "params": {"clientInfo": {"name": "cctui", "version": env!("CARGO_PKG_VERSION")}},
    })
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

/// Build a `thread/settings/update` request that persistently overrides the
/// model and/or reasoning effort for subsequent turns on `thread_id` (CCT-303).
/// Only the provided fields are sent so a partial update leaves the other
/// setting untouched.
fn thread_settings_update_req(
    id: i64,
    thread_id: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Value {
    let mut params = serde_json::Map::new();
    params.insert("threadId".to_owned(), json!(thread_id));
    if let Some(model) = model {
        params.insert("model".to_owned(), json!(model));
    }
    if let Some(effort) = effort {
        params.insert("effort".to_owned(), json!(effort));
    }
    json!({"jsonrpc": "2.0", "id": id, "method": "thread/settings/update", "params": params})
}

fn turn_start_req(id: i64, thread_id: &str, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "turn/start",
        "params": {"threadId": thread_id, "input": [{"type": "text", "text": text}]},
    })
}

fn turn_interrupt_req(id: i64, thread_id: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "turn/interrupt", "params": {"threadId": thread_id}})
}

/// Reply to a server-issued approval request. `rpc_id` must be the exact
/// `id` value from the request; `kind` selects the decision vocabulary.
fn approval_reply(rpc_id: &Value, kind: ApprovalKind, allow: bool) -> Value {
    json!({"jsonrpc": "2.0", "id": rpc_id, "result": {"decision": kind.decision(allow)}})
}

// ---------------------------------------------------------------------------
// Async driver
// ---------------------------------------------------------------------------

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
    /// terminates the child.
    Interrupt,
    /// Change the model and/or reasoning effort of the running thread in place
    /// (CCT-303): sends `thread/settings/update { threadId, model?, effort? }`
    /// so subsequent turns use the new settings, and echoes the resolved values
    /// back via [`AdapterEvent::Status`] so the webui chip updates live.
    SetModel { model: Option<String>, effort: Option<String> },
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
        events: mpsc::Sender<AdapterEvent>,
        live: LiveSessionRegistry,
        registry: SessionRegistry,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            cfg,
            cwd,
            env,
            launch: SessionLaunch::Fresh { prompt, name },
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
        events: mpsc::Sender<AdapterEvent>,
        live: LiveSessionRegistry,
        registry: SessionRegistry,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            cfg,
            cwd,
            env,
            launch: SessionLaunch::Fork { parent_thread_id, prompt, name },
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
            events,
            live,
            registry,
            shutdown,
        }
    }

    /// Spawn the subprocess, complete the handshake, then pump IO until the
    /// process exits, the session is killed, or the daemon shuts down.
    #[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
    pub async fn run(self) -> Result<()> {
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
        write_json(&mut stdin, &initialize_req()).await?;
        let mut local_id = String::new();
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

        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => {
                    let _ = child.start_kill();
                    killed = true;
                    break;
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
                            let req = turn_start_req(next_id, &local_id, &text);
                            next_id += 1;
                            // A write failure here means the app-server is gone
                            // — remember the turn and let the epilogue revive
                            // the thread if this was a clean hibernation exit.
                            if let Err(e) = write_json(&mut stdin, &req).await {
                                tracing::warn!(%e, "codex: turn/start write failed; ending session");
                                retry_after_hibernate = Some(SessionCommand::Send { text });
                                break;
                            }
                        }
                        Some(SessionCommand::Rename { name }) => {
                            if let Err(e) = set_thread_name(
                                &mut stdin,
                                &mut next_id,
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
                        Some(SessionCommand::Interrupt) => {
                            // Keep-alive interrupt (CCT-210): abort the turn but
                            // leave the app-server running so the session keeps
                            // going — unlike Kill, we do NOT terminate the child.
                            let req = turn_interrupt_req(next_id, &local_id);
                            next_id += 1;
                            if let Err(e) = write_json(&mut stdin, &req).await {
                                tracing::warn!(%e, "codex: turn/interrupt write failed; ending session");
                                break;
                            }
                        }
                        Some(SessionCommand::SetModel { model, effort }) => {
                            if let Err(e) = set_thread_model(
                                &mut stdin,
                                &mut next_id,
                                &local_id,
                                model.as_deref(),
                                effort.as_deref(),
                                &self.events,
                                &self.registry,
                            )
                            .await
                            {
                                tracing::warn!(%e, "codex: thread/settings/update write failed; ending session");
                                retry_after_hibernate = Some(SessionCommand::SetModel { model, effort });
                                break;
                            }
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
                    match classify(&local_id, &value) {
                        Incoming::Response { id, value } if id == ID_INITIALIZE => {
                            let _ = value;
                            match &self.launch {
                                SessionLaunch::Fresh { .. } => {
                                    write_json(&mut stdin, &thread_start_req(&self.cwd)).await?;
                                }
                                SessionLaunch::Resume { thread_id, .. } => {
                                    write_json(&mut stdin, &thread_resume_req(thread_id, &self.cwd))
                                        .await?;
                                }
                                SessionLaunch::Fork { parent_thread_id, .. } => {
                                    write_json(
                                        &mut stdin,
                                        &thread_fork_req(parent_thread_id, &self.cwd),
                                    )
                                    .await?;
                                }
                            }
                        }
                        Incoming::Response { id, value } if id == ID_THREAD_START => {
                            let result = value.get("result").cloned().unwrap_or(Value::Null);
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
                                SessionLaunch::Fresh { name, prompt }
                                | SessionLaunch::Fork { name, prompt, .. } => {
                                    if let Some(name) = name.as_deref() {
                                        let result = set_thread_name(
                                            &mut stdin,
                                            &mut next_id,
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
                                    if !end_after_initial
                                        && let Some(prompt) = prompt.as_deref()
                                    {
                                        let req = turn_start_req(next_id, &local_id, prompt);
                                        next_id += 1;
                                        if let Err(e) = write_json(&mut stdin, &req).await {
                                            tracing::warn!(%e, "codex: initial prompt write failed; ending session");
                                            retry_after_hibernate =
                                                Some(SessionCommand::Send { text: prompt.to_owned() });
                                            end_after_initial = true;
                                        }
                                    }
                                }
                                SessionLaunch::Resume { initial_commands, .. } => {
                                    for command in initial_commands.clone() {
                                        match command {
                                            SessionCommand::Send { text } => {
                                                let req = turn_start_req(next_id, &local_id, &text);
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
                                            SessionCommand::SetModel { model, effort } => {
                                                if let Err(e) = set_thread_model(
                                                    &mut stdin,
                                                    &mut next_id,
                                                    &local_id,
                                                    model.as_deref(),
                                                    effort.as_deref(),
                                                    &self.events,
                                                    &self.registry,
                                                )
                                                .await
                                                {
                                                    tracing::warn!(%e, "codex: resumed thread/settings/update write failed");
                                                    retry_after_hibernate =
                                                        Some(SessionCommand::SetModel { model, effort });
                                                    end_after_initial = true;
                                                    break;
                                                }
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
                        Incoming::Response { .. } | Incoming::Ignored => {}
                    }
                }
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

async fn set_thread_name<W: AsyncWriteExt + Unpin>(
    stdin: &mut W,
    next_id: &mut i64,
    thread_id: &str,
    name: &str,
    events: &mpsc::Sender<AdapterEvent>,
    registry: &SessionRegistry,
) -> Result<()> {
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

/// Apply an in-place model/effort change (CCT-303): send
/// `thread/settings/update` to the running thread, update the durable
/// `SessionRecord` cfg so a later resume keeps the new settings, and echo the
/// resolved values back via [`AdapterEvent::Status`] so the webui chip updates
/// live (codex emits a `thread/settings/updated` notification too, but we
/// surface the requested values directly for immediacy).
#[allow(clippy::too_many_arguments)]
async fn set_thread_model<W: AsyncWriteExt + Unpin>(
    stdin: &mut W,
    next_id: &mut i64,
    thread_id: &str,
    model: Option<&str>,
    effort: Option<&str>,
    events: &mpsc::Sender<AdapterEvent>,
    registry: &SessionRegistry,
) -> Result<()> {
    write_json(stdin, &thread_settings_update_req(*next_id, thread_id, model, effort)).await?;
    *next_id += 1;
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
    Ok(())
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
        let turn = turn_start_req(100, "tid", "hello");
        assert_eq!(turn["params"]["threadId"], "tid");
        assert_eq!(turn["params"]["input"][0]["text"], "hello");
    }

    #[test]
    fn thread_settings_update_req_sends_only_provided_fields() {
        // Both fields.
        let both = thread_settings_update_req(102, "tid", Some("gpt-5-codex"), Some("high"));
        assert_eq!(both["method"], "thread/settings/update");
        assert_eq!(both["id"], 102);
        assert_eq!(both["params"]["threadId"], "tid");
        assert_eq!(both["params"]["model"], "gpt-5-codex");
        assert_eq!(both["params"]["effort"], "high");
        // Model only — effort key must be absent so codex keeps its current effort.
        let model_only = thread_settings_update_req(103, "tid", Some("gpt-5-codex"), None);
        assert_eq!(model_only["params"]["model"], "gpt-5-codex");
        assert!(model_only["params"].get("effort").is_none());
        // Effort only.
        let effort_only = thread_settings_update_req(104, "tid", None, Some("low"));
        assert_eq!(effort_only["params"]["effort"], "low");
        assert!(effort_only["params"].get("model").is_none());
    }

    #[test]
    fn set_model_is_resumable() {
        assert!(SessionCommand::SetModel { model: Some("m".into()), effort: None }.is_resumable());
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
}
