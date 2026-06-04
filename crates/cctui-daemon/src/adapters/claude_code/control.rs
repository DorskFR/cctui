//! Driver for the `claude daemon` control-socket adapter path (CCT-83).
//!
//! Polls `list` every `poll_interval`, diffs against the previous roster
//! to emit `SessionStarted` / `SessionEnded`, and merges identity fields
//! from `~/.claude/jobs/<short>/state.json` to produce `Status` events.
//!
//! Per-session `subscribe` streams and the transcript tail land in
//! Phase 3 (CCT-84) — `list` already gives us state/tempo/detail at the
//! 2s poll cadence.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use cctui_proto::adapter::{AdapterCommand, AdapterEvent, EndReason, JobShort, SessionMeta};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::backfill::{self, BackfillConfig, CursorFile, default_cursor_path};
use super::discovery::Discovery;
use super::kickstart::Kickstarter;
use super::state::{StateJson, default_jobs_root};
use super::transcript::{self, OffsetStore, default_projects_root};
use super::{SessionMap, socket};

/// Config knobs read from `adapters_enabled.config`.
#[derive(Debug, Clone)]
pub struct DriverConfig {
    pub poll_interval: Duration,
    pub jobs_root: PathBuf,
    pub projects_root: PathBuf,
    /// Override the discovery base for tests / non-standard layouts.
    pub discovery: Discovery,
    /// Optional override for the transcript-offsets store path. `None`
    /// uses the default `$XDG_CONFIG_HOME/cctui/transcript-offsets.json`.
    pub offsets_path: Option<PathBuf>,
    /// Optional override for the backfill cursor path. `None` uses the
    /// default `$XDG_CONFIG_HOME/cctui/backfill.json`.
    pub backfill_cursor_path: Option<PathBuf>,
    /// Skip the startup backfill pass. Default: false (backfill runs).
    pub skip_backfill: bool,
    /// Binary used for the `claude rm <short>` removal invoked by
    /// [`AdapterCommand::Remove`]. Defaults to `claude` (resolved on `PATH`).
    pub claude_bin: String,
    /// Local socket the `AskUserQuestion` hook (CCT-167) delivers to. Shared
    /// with the listener spawned in [`super::ClaudeCodeAdapter::start`] so the
    /// injected `--settings` hook command targets the same path the daemon
    /// binds.
    pub hook_socket_path: PathBuf,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(2),
            jobs_root: default_jobs_root(),
            projects_root: default_projects_root(),
            discovery: Discovery::for_current_user(),
            offsets_path: None,
            backfill_cursor_path: None,
            skip_backfill: false,
            claude_bin: "claude".to_string(),
            hook_socket_path: super::resolve_legacy_socket_path(&serde_json::Value::Null),
        }
    }
}

impl DriverConfig {
    pub fn from_value(v: &serde_json::Value) -> Self {
        let mut cfg = Self::default();
        if let Some(ms) = v.get("poll_interval_ms").and_then(serde_json::Value::as_u64) {
            cfg.poll_interval = Duration::from_millis(ms);
        }
        if let Some(p) = v.get("jobs_root").and_then(serde_json::Value::as_str) {
            cfg.jobs_root = PathBuf::from(p);
        }
        if let Some(p) = v.get("projects_root").and_then(serde_json::Value::as_str) {
            cfg.projects_root = PathBuf::from(p);
        }
        if let Some(p) = v.get("discovery_base").and_then(serde_json::Value::as_str) {
            cfg.discovery = Discovery::with_base(PathBuf::from(p));
        }
        if let Some(p) = v.get("offsets_path").and_then(serde_json::Value::as_str) {
            cfg.offsets_path = Some(PathBuf::from(p));
        }
        if let Some(p) = v.get("backfill_cursor_path").and_then(serde_json::Value::as_str) {
            cfg.backfill_cursor_path = Some(PathBuf::from(p));
        }
        if let Some(b) = v.get("skip_backfill").and_then(serde_json::Value::as_bool) {
            cfg.skip_backfill = b;
        }
        if let Some(s) = v.get("claude_bin").and_then(serde_json::Value::as_str) {
            cfg.claude_bin = s.to_string();
        }
        cfg.hook_socket_path = super::resolve_legacy_socket_path(v);
        cfg
    }
}

/// The `list` op returns `{ok: true, op: "list", jobs: [LiveSnapshot]}`.
#[derive(Debug, Deserialize)]
struct ListResponse {
    #[serde(default)]
    jobs: Vec<LiveSnapshot>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct LiveSnapshot {
    pub short: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default, alias = "sessionId")]
    pub session_id_camel: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub tempo: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    /// Set by the claude daemon when the worker is awaiting a decision; for a
    /// tool-permission prompt it reads e.g. `"approve Bash: touch /tmp/x"`
    /// (CCT-211). Empty/absent when nothing is pending.
    #[serde(default)]
    pub needs: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub dying: bool,
    #[serde(default, alias = "cliVersion")]
    pub cli_version: Option<String>,
}

impl LiveSnapshot {
    fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref().or(self.session_id_camel.as_deref())
    }

    /// §7.2 of the protocol doc: skip spares and dying workers.
    fn is_user_visible(&self) -> bool {
        !self.dying && self.source.as_deref() != Some("spare")
    }
}

pub struct Driver {
    cfg: DriverConfig,
    events: mpsc::Sender<AdapterEvent>,
    /// Inbound: commands routed from server → daemon → adapter.
    commands: mpsc::Receiver<AdapterCommand>,
    shutdown: CancellationToken,
    roster: HashSet<String>,
    last_status: HashMap<String, StatusSnapshot>,
    /// Shared `session_id → stable local_id` map. Populated as transcripts are
    /// pinned (incl. across `/clear` rotations) and read by the ask-hook
    /// listener so a hook's live `session_id` resolves to the `local_id` the
    /// server keys on (CCT-167).
    session_to_local: SessionMap,
    /// Reverse lookup: `local_id` (`session_id`) → worker `short`. Built
    /// from list snapshots so command dispatch can target the right
    /// worker even though the server identifies sessions by their
    /// `session_id`.
    short_by_session: HashMap<String, String>,
    /// Per-session transcript byte offsets, persisted across daemon
    /// restarts to avoid replay.
    offsets: OffsetStore,
    /// Cache: short → (cwd, `session_id`) so we can locate the transcript
    /// without re-reading `state.json` on every tick.
    transcript_locations: HashMap<String, TranscriptLocation>,
    /// Task-tool subagents currently tracked, keyed by `agentId` (CCT-141).
    /// Observe-only nested sessions discovered by scanning each parent's
    /// `subagents/` transcript directory.
    subagents: HashMap<String, SubagentState>,
    /// `agentId`s already ended (via quiescence). Prevents a finished
    /// subagent's still-present transcript from being rediscovered and
    /// re-announced on the next poll.
    ended_subagents: HashSet<String>,
    /// Self-heals the on-demand `claude daemon`: when the control socket is
    /// missing (idle shutdown, sleep, teardown) this boots it via `claude
    /// agents --json` so polling/dispatch stop failing with "no claude daemon
    /// socket present" (CCT-194).
    kickstarter: Kickstarter,
    /// Holds a persistent headless `attach` open per live session so the
    /// dispatched worker actually wakes (focus-in seed) and is kept off the
    /// 60s idle-retire path. Without this, dispatched/replied sessions sit in
    /// limbo until a human opens them in `claude agents` (CCT-209).
    attach: super::attach::AttachManager,
    /// Tool-permission prompts currently pending, keyed by worker `short`
    /// (CCT-211). Derived from the snapshot's `tempo:"blocked"`/`needs` signal:
    /// a fresh/changed `needs` emits a `PermissionRequest`, and clearing it
    /// emits `PermissionResolved`. Dedups so a still-pending prompt isn't
    /// re-emitted on every poll.
    pending_perms: HashMap<String, PendingPerm>,
    /// Monotonic counter minting synthesized permission `request_id`s. Claude's
    /// control socket exposes no id for an interactive prompt (the `needs`
    /// string is all we get), so we mint our own purely as a correlation token
    /// the server/clients echo back; the answer is keyed on the worker `short`.
    perm_seq: u64,
    /// Sessions with an `AskUserQuestion` form currently up in the PTY,
    /// maintained by the ask-hook listener (CCT-219). A `reply` injected while
    /// the form is up would just confirm the highlighted option — the reply
    /// path dismisses the form (attach+ESC) first so the user's actual text is
    /// what claude receives.
    pending_asks: super::PendingAsks,
}

#[derive(Debug, Clone)]
struct PendingPerm {
    /// Synthesized id echoed back via `AdapterCommand::PermissionResponse`.
    request_id: String,
    /// Stable session `local_id` the request was emitted under.
    local_id: String,
    /// The raw `needs` string this request was emitted for. A change means a
    /// new prompt (the previous one resolved), so we re-emit.
    needs: String,
}

/// How many consecutive idle polls mark a subagent's transcript as done.
/// Subagents run to completion without waiting for input, so a quiescent
/// transcript reliably signals the subagent has finished (~30s at the 2s
/// default poll). Lifecycle end never arrives over the control socket
/// (subagents aren't `list` jobs), so quiescence is the primary signal,
/// with parent-session end as a backstop.
const SUBAGENT_IDLE_TICKS_TO_END: u32 = 15;

#[derive(Debug, Clone)]
struct TranscriptLocation {
    path: PathBuf,
    local_id: String,
    /// Working directory of the parent session — reused as the subagents'
    /// `working_dir` and to locate their `subagents/` transcript dir.
    cwd: String,
    /// Key into the offset store. Stable across daemon restarts.
    offset_key: String,
}

#[derive(Debug, Clone)]
struct SubagentState {
    /// The parent session id (== parent's `local_id` / DB row id).
    parent_local_id: String,
    /// Consecutive polls during which the transcript did not grow.
    idle_ticks: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusSnapshot {
    tempo: Option<String>,
    state: Option<String>,
    detail: Option<String>,
    name: Option<String>,
    activity: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

impl Driver {
    pub fn new(
        cfg: DriverConfig,
        events: mpsc::Sender<AdapterEvent>,
        commands: mpsc::Receiver<AdapterCommand>,
        shutdown: CancellationToken,
    ) -> Self {
        // Offsets are kept in-memory only in production (CCT-92): the
        // transcript-tail offset could otherwise advance + persist past
        // events that hadn't yet shipped over the WS, losing them on a
        // disconnect. With server-side idempotency on `stream_events`
        // we can safely re-tail from 0 on every adapter (re)start.
        // Tests still pass an explicit path so they can verify the file
        // I/O path itself.
        let offsets = cfg
            .offsets_path
            .clone()
            .map_or_else(|| OffsetStore::open(None), |p| OffsetStore::open(Some(p)));
        let kickstarter = Kickstarter::new(cfg.claude_bin.clone());
        let attach = super::attach::AttachManager::new(cfg.discovery.clone(), shutdown.clone());
        Self {
            cfg,
            events,
            commands,
            shutdown,
            roster: HashSet::new(),
            last_status: HashMap::new(),
            session_to_local: Arc::new(Mutex::new(HashMap::new())),
            offsets,
            transcript_locations: HashMap::new(),
            short_by_session: HashMap::new(),
            subagents: HashMap::new(),
            ended_subagents: HashSet::new(),
            kickstarter,
            attach,
            pending_perms: HashMap::new(),
            perm_seq: 0,
            pending_asks: super::PendingAsks::default(),
        }
    }

    /// Clone handle to the shared `session_id → local_id` map, for the
    /// ask-hook listener to translate live `session_id`s (CCT-167).
    pub fn session_map(&self) -> SessionMap {
        self.session_to_local.clone()
    }

    /// Clone handle to the shared pending-ask set, for the ask-hook listener
    /// to maintain (CCT-219).
    pub fn pending_asks(&self) -> super::PendingAsks {
        self.pending_asks.clone()
    }

    #[allow(clippy::cognitive_complexity)]
    pub async fn run(mut self) -> anyhow::Result<()> {
        if !self.cfg.skip_backfill {
            self.run_backfill().await;
        }
        let mut tick = tokio::time::interval(self.cfg.poll_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => return Ok(()),
                _ = tick.tick() => {
                    if let Err(err) = self.poll_once().await {
                        tracing::debug!(%err, "claude daemon poll failed (will retry)");
                    }
                }
                Some(cmd) = self.commands.recv() => {
                    // Capture the correlation id before `cmd` is moved so we can
                    // report the outcome back to the originating client (CCT-131).
                    let command_id = match &cmd {
                        AdapterCommand::Spawn { command_id, .. } => *command_id,
                        _ => None,
                    };
                    let res = self.handle_command(cmd).await;
                    if let Some(command_id) = command_id {
                        let (ok, error) = match &res {
                            Ok(()) => (true, None),
                            Err(err) => (false, Some(err.to_string())),
                        };
                        let _ = self
                            .events
                            .send(AdapterEvent::CommandResult { command_id, ok, error })
                            .await;
                    }
                    if let Err(err) = res {
                        tracing::warn!(%err, "command dispatch failed");
                    }
                }
            }
        }
    }

    async fn run_backfill(&mut self) {
        let cfg = BackfillConfig {
            jobs_root: self.cfg.jobs_root.clone(),
            projects_root: self.cfg.projects_root.clone(),
            cursor_path: self.cfg.backfill_cursor_path.clone().or_else(default_cursor_path),
        };
        let mut cursor = cfg
            .cursor_path
            .clone()
            .map_or_else(CursorFile::open_default, |p| CursorFile::open(Some(p)));
        match backfill::run_once(&cfg, &self.events, &mut cursor, &mut self.offsets).await {
            Ok(n) if n > 0 => {
                tracing::info!(backfilled = n, "claude-code backfill pass complete");
                self.offsets.flush();
            }
            Ok(_) => tracing::debug!("no historical sessions to backfill"),
            Err(err) => tracing::warn!(%err, "backfill pass failed"),
        }
    }

    /// Deliver a user message to a worker, handling a pending `AskUserQuestion`
    /// form. With structured `ask_picks` and the hook-captured questions we
    /// answer the form *natively* — keystrokes on the real form, so claude
    /// records a genuine `tool_result` with the selected labels (CCT-226).
    /// Otherwise (free-text answer, missing questions, keystroke failure) fall
    /// back to dismiss-then-reply: attach+ESC the form away, then `reply` the
    /// text (CCT-219; claude records the ask as declined and reads the text as
    /// a new user turn).
    async fn deliver_reply(
        &self,
        sock: &std::path::Path,
        local_id: &str,
        text: &str,
        ask_picks: Option<Vec<Vec<usize>>>,
    ) -> anyhow::Result<()> {
        // Hibernated sessions (worker exited, job state still on disk)
        // have left `short_by_session`, so fall back to deriving the
        // short from the session id — same as the removal path.
        let short =
            self.resolve_short(local_id).or_else(|_| self.resolve_short_for_removal(local_id))?;
        // Resume-on-reply (CCT-228): a reply to an exited worker is
        // ENOJOB'd by the claude daemon and silently lost. Revive it
        // first via a resume `dispatch`, then deliver as normal. Live
        // workers take the existing path with zero extra ops.
        self.resume_if_hibernated(sock, &short).await?;
        // If an AskUserQuestion form is up in the worker's PTY, a bare
        // `reply` just presses Enter on the highlighted option — claude
        // records option 1 ("Proceed"-style) and the user's text is
        // swallowed (CCT-219).
        let pending_ask = self.pending_asks.lock().ok().and_then(|mut m| m.remove(local_id));
        if let Some(questions) = pending_ask {
            // Native answer first (CCT-226): drive the real form.
            if let Some(picks) = ask_picks
                && let Some(chunks) = questions.as_ref().and_then(|q| ask_keystrokes(q, &picks))
            {
                match socket::attach_answer_keys(sock, &short, &chunks).await {
                    Ok(()) => {
                        tracing::info!(%short, "answered ask form natively via keystrokes");
                        // PostToolUse fires for the real answer and emits
                        // `resolved`, but synthesize one too so the live card
                        // drops immediately (it's idempotent client-side).
                        let _ = self
                            .events
                            .send(AdapterEvent::AskResolved { local_id: local_id.to_owned() })
                            .await;
                        return Ok(());
                    }
                    Err(err) => {
                        tracing::warn!(%err, %short, "native ask answer failed; falling back to dismiss+reply");
                    }
                }
            }
            // Fallback: dismiss the form (attach+ESC, the same mechanism as
            // interrupt/permission-deny), then deliver the text so claude
            // reads the user's actual answer.
            if let Err(err) = socket::attach_interrupt(sock, &short).await {
                tracing::warn!(%err, %short, "failed to dismiss pending ask form");
            } else {
                tracing::info!(%short, "dismissed pending ask form before reply");
                // PostToolUse never fires for a cancelled ask, so the
                // hook won't emit `resolved` — synthesize it so the
                // server/clients drop the live question card.
                let _ = self
                    .events
                    .send(AdapterEvent::AskResolved { local_id: local_id.to_owned() })
                    .await;
                // Give the TUI a beat to settle after the ESC before
                // the reply lands.
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
        }
        let resp =
            socket::one_shot(sock, &json!({"proto":1,"op":"reply","short":short,"text":text}))
                .await?;
        tracing::debug!(?resp, %short, "reply ack");
        Ok(())
    }

    #[allow(clippy::cognitive_complexity)]
    async fn handle_command(&self, cmd: AdapterCommand) -> anyhow::Result<()> {
        // A command (spawn/reply/kill/…) needs a live control socket. If the
        // on-demand claude daemon has shut down, boot it and wait briefly for
        // the socket rather than failing the command outright (CCT-194).
        let sock = self.ensure_socket().await?;
        match cmd {
            AdapterCommand::SendMessage { local_id, text } => {
                self.deliver_reply(&sock, &local_id, &text, None).await?;
            }
            AdapterCommand::Reply { local_id, text, ask_picks } => {
                self.deliver_reply(&sock, &local_id, &text, ask_picks).await?;
            }
            AdapterCommand::Kill { local_id, signal } => {
                let short = self.resolve_short(&local_id)?;
                let mut req = json!({"proto":1,"op":"kill","short":short});
                if let Some(s) = signal {
                    // Claude's control-socket `kill` op validates `signal`
                    // against the string enum ["SIGTERM","SIGKILL"] (zod). A
                    // numeric signal (e.g. the interrupt route's `15`) fails
                    // that validation and the whole op is rejected, so the
                    // request silently no-op'd — this was why "interrupt" never
                    // actually interrupted a claude session (CCT-169). The
                    // control socket exposes no in-place turn-interrupt op, so
                    // the best we can do for a headless worker is terminate it;
                    // map to the enum name the daemon accepts.
                    req["signal"] = serde_json::Value::String(kill_signal_name(s).to_owned());
                }
                let resp = socket::one_shot(&sock, &req).await?;
                tracing::debug!(?resp, %short, "kill ack");
            }
            AdapterCommand::Interrupt { local_id } => {
                // Keep-alive turn interrupt (CCT-210): the control socket has
                // no turn-interrupt op, so attach to the worker PTY and inject
                // an ESC keystroke — the same key that aborts a turn in the
                // TUI. Unlike `Kill`, the worker stays live and resumable.
                let short = self.resolve_short(&local_id)?;
                socket::attach_interrupt(&sock, &short).await?;
                tracing::info!(%short, "interrupted in-flight turn via attach+ESC");
            }
            AdapterCommand::PermissionResponse { local_id, request_id, allow } => {
                // The control socket's `permission-response` op is a no-op stub
                // in current claude (acks ok:true but never resolves the
                // prompt). Answer the way a human does instead: attach to the
                // PTY and inject `1`+Enter (approve) or ESC (deny) — the same
                // mechanism the interrupt path uses (CCT-210/CCT-211).
                let short = self.resolve_short(&local_id)?;
                socket::attach_permission_response(&sock, &short, allow).await?;
                tracing::info!(%short, %request_id, allow, "answered permission prompt via attach");
            }
            AdapterCommand::Remove { local_id } => {
                let short = self.resolve_short_for_removal(&local_id)?;
                // Imitate the agent-view Ctrl+X (CCT-132): there is no
                // control-socket removal op, so (1) stop the worker if it is
                // still live, (2) wait for it to actually exit, then (3) let
                // `claude rm` delete the on-disk job metadata + worktree. This
                // clears the session from Claude Code's own `claude agents`
                // view as well as our discovery; the transcript is preserved.
                let _ =
                    socket::one_shot(&sock, &json!({"proto":1,"op":"kill","short":short})).await;
                Self::await_worker_exit(&sock, &short).await;
                self.claude_rm(&short).await?;
            }
            AdapterCommand::Spawn { spec, .. } => {
                self.spawn(&sock, &spec).await?;
            }
            AdapterCommand::Rename { local_id, name } => {
                let short = self.resolve_short(&local_id)?;
                // No control-socket rename op exists; persist to the on-disk
                // state.json the status poll reads from. The next poll re-emits
                // Status with the new name (the server also updates its DB row
                // synchronously in the PATCH route).
                StateJson::write_name(&self.cfg.jobs_root, &short, &name)
                    .with_context(|| format!("rename session {short} -> {name}"))?;
                tracing::info!(%short, %name, "renamed session via state.json");
            }
            _ => {
                // AdapterCommand is #[non_exhaustive]; tolerate unknown
                // future variants by logging.
                tracing::warn!("unhandled AdapterCommand variant");
            }
        }
        Ok(())
    }

    fn resolve_short(&self, local_id: &str) -> anyhow::Result<String> {
        self.short_by_session
            .get(local_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown session {local_id}"))
    }

    /// Resolve a short for removal, tolerating sessions that have already left
    /// the live roster. Removal (CCT-132) most often targets *completed*
    /// sessions, but `short_by_session` only holds live ones — it's cleared
    /// when a session exits. Claude-code's short is the first group of the
    /// session UUID (state.json `daemonShort`), so fall back to deriving it.
    /// A wrong guess just makes `claude rm` a no-op (ENOJOB), so this stays
    /// best-effort rather than erroring.
    fn resolve_short_for_removal(&self, local_id: &str) -> anyhow::Result<String> {
        if let Some(short) = self.short_by_session.get(local_id) {
            return Ok(short.clone());
        }
        let candidate = local_id.split('-').next().unwrap_or(local_id);
        JobShort::parse(candidate)
            .map(|j| j.as_str().to_string())
            .ok_or_else(|| anyhow::anyhow!("cannot resolve short for {local_id}"))
    }

    /// Poll the `has` op until the worker is no longer alive (or we give up).
    /// `claude rm` is documented to work on already-exited sessions; racing it
    /// against a still-live worker is undefined, so we drain the kill first.
    /// Best-effort: a socket error or timeout just falls through to `claude rm`.
    async fn await_worker_exit(sock: &std::path::Path, short: &str) {
        for _ in 0..20 {
            match socket::one_shot(sock, &json!({"proto":1,"op":"has","short":short})).await {
                Ok(resp) => {
                    let alive =
                        resp.get("alive").and_then(serde_json::Value::as_bool).unwrap_or(false);
                    if !alive {
                        return;
                    }
                }
                // Socket gone / op failed — nothing more to wait on.
                Err(_) => return,
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tracing::warn!(%short, "worker still live 2s after kill; proceeding to `claude rm`");
    }

    /// Resume-on-reply (CCT-228): if `short` has no live worker, revive it
    /// before a reply is delivered. The claude control socket cannot wake an
    /// exited job itself — `attach`/`reply` both return ENOJOB; the picker's
    /// "enter to resume" is client-side. What does work (probed against
    /// claude v2.1.162) is a `dispatch` that reuses the dead job's identity:
    /// same `short`, the `resumeSessionId` from its on-disk `state.json`, and
    /// `--resume <id>` as the launch argv — the daemon spawns a fresh worker
    /// bound to the saved conversation, original transcript re-pinned.
    ///
    /// No-op (one cheap `has` round-trip) when the worker is alive.
    async fn resume_if_hibernated(
        &self,
        sock: &std::path::Path,
        short: &str,
    ) -> anyhow::Result<()> {
        let alive = |resp: &serde_json::Value| {
            resp.get("alive").and_then(serde_json::Value::as_bool).unwrap_or(false)
        };
        let has = socket::one_shot(sock, &json!({"proto":1,"op":"has","short":short})).await?;
        if alive(&has) {
            return Ok(());
        }

        let st = StateJson::read(&self.cfg.jobs_root, short).ok_or_else(|| {
            anyhow::anyhow!(
                "session {short} has exited and no job state remains on disk to resume from"
            )
        })?;
        // `/clear`/`/compact` rotate the live conversation into the id recorded
        // in `resumeSessionId`; resuming the stale spawn id would fork the
        // conversation back at the pre-reset state (CCT-160).
        let session_id = st
            .resume_session_id
            .clone()
            .or_else(|| st.session_id.clone())
            .ok_or_else(|| anyhow::anyhow!("state.json for {short} has no session id"))?;
        let cwd =
            st.cwd.clone().ok_or_else(|| anyhow::anyhow!("state.json for {short} has no cwd"))?;

        let agent = "claude";
        let nonce: String = uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect();
        let created_at = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        )
        .unwrap_or(0);
        let req = json!({
            "proto": 1,
            "op": "dispatch",
            "timeoutMs": 15000,
            "d": {
                "proto": 1,
                "short": short,
                "nonce": nonce,
                "sessionId": session_id,
                "createdAt": created_at,
                "source": "fleet",
                "cwd": cwd,
                "launch": { "mode": "prompt", "args": ["--resume", &session_id, "--agent", agent] },
                "env": {},
                "isolation": "none",
                "respawnFlags": ["--agent", agent],
                "agent": agent,
                // `state.json` already exists for this short; the daemon keeps
                // its identity fields, so the seed is just protocol filler.
                "seed": { "intent": st.intent.clone().unwrap_or_default() },
                "cols": 120,
                "rows": 40,
            }
        });
        let resp: serde_json::Value = socket::call(sock, &req)
            .await
            .with_context(|| format!("resume dispatch for hibernated session {short}"))?;
        tracing::info!(?resp, %short, %session_id, "resumed hibernated session via dispatch");

        // Wait (bounded) for the revived worker to report alive, then give the
        // PTY a moment to finish booting so the reply isn't swallowed by a
        // half-started claude. The next poll tick re-adds the short to the
        // roster and the AttachManager's persistent attach keeps it awake.
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if let Ok(resp) =
                socket::one_shot(sock, &json!({"proto":1,"op":"has","short":short})).await
                && alive(&resp)
            {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                return Ok(());
            }
        }
        anyhow::bail!("resumed session {short} did not come alive within 10s");
    }

    /// Run `claude rm <short>` to delete the job metadata + Claude-created
    /// worktree. Best-effort: a worktree with uncommitted changes makes the CLI
    /// refuse and print the path — we log that but do not fail the archive, so
    /// the cctui-side state still moves to `archived`.
    async fn claude_rm(&self, short: &str) -> anyhow::Result<()> {
        let out = tokio::process::Command::new(&self.cfg.claude_bin)
            .arg("rm")
            .arg(short)
            // `claude` lives in `~/.local/bin`, off launchd's minimal PATH
            // (CCT-138) — give the child an augmented PATH so exec succeeds.
            .env("PATH", crate::childenv::child_path())
            .output()
            .await
            .with_context(|| format!("spawning `{} rm {short}`", self.cfg.claude_bin))?;
        if out.status.success() {
            tracing::info!(%short, "removed claude job via `claude rm`");
        } else {
            tracing::warn!(
                %short,
                code = ?out.status.code(),
                stderr = %String::from_utf8_lossy(&out.stderr).trim(),
                "`claude rm` did not complete cleanly (worktree changes?)"
            );
        }
        Ok(())
    }

    /// Spawn a fresh claude session via the `dispatch` op on the `claude
    /// daemon` control socket — the same primitive claude's own `FleetView`
    /// uses (`source:"fleet"`). The new session surfaces in the next `list`
    /// poll and goes through the normal observe path; there is no separate
    /// ACK beyond the dispatch reply.
    ///
    /// The payload shape is the daemon's private, proto-gated dispatch
    /// record. It is NOT `{op,cwd,prompt}` — that older guess was rejected
    /// outright (`malformed request`) and the rejection was swallowed,
    /// producing a silent no-op (CCT-131). We mint the session id / short /
    /// nonce client-side exactly as claude does and hand the worker its
    /// launch argv.
    #[allow(clippy::too_many_lines)]
    async fn spawn(
        &self,
        sock: &std::path::Path,
        spec: &cctui_proto::adapter::SessionSpec,
    ) -> anyhow::Result<()> {
        let cwd = spec
            .working_dir
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("spawn: working_dir required"))?;
        let cwd_path = std::path::Path::new(cwd);
        if !cwd_path.is_dir() {
            anyhow::bail!("spawn: working_dir does not exist or is not a directory: {cwd}");
        }

        let agent = "claude";
        let session_id = uuid::Uuid::new_v4().to_string();
        // `short` is the first uuid group (8 hex chars); `nonce` is 8 fresh
        // hex chars. Both satisfy the daemon's /^[a-f0-9]{8}$/ validator.
        let short = &session_id[..8];
        let nonce: String = uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect();
        let created_at = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        )
        .unwrap_or(0);

        // Worker launch argv, mirroring claude's own fleet dispatch:
        // `--session-id <id> --agent claude [--name <name>] [-- <prompt>]`.
        let mut args = vec![
            "--session-id".to_owned(),
            session_id.clone(),
            "--agent".to_owned(),
            agent.to_owned(),
        ];
        if let Some(name) = &spec.name {
            args.push("--name".to_owned());
            args.push(name.clone());
        }
        // Per-spawn permission posture (CCT-149). `None` inherits whatever
        // the claude daemon was launched with (the user's global default).
        if let Some(mode) = spec.permission_mode {
            use cctui_proto::adapter::PermissionMode;
            let claude_mode = match mode {
                PermissionMode::Yolo => "bypassPermissions",
                PermissionMode::Auto => "acceptEdits",
                PermissionMode::Ask => "default",
            };
            args.push("--permission-mode".to_owned());
            args.push(claude_mode.to_owned());
        }
        // Inject the managed `AskUserQuestion` hook settings (CCT-167), scoped
        // to this fleet-spawned worker only — the user's hand-run `claude` is
        // untouched. `--settings` merges over the resolved hierarchy, so it
        // only ADDS the hook. Goes into `respawnFlags` too so it survives the
        // `/clear`/`/compact` relaunch the claude daemon drives off them.
        let mut respawn_flags = vec!["--agent".to_owned(), agent.to_owned()];
        // Reasoning effort (claude `--effort`: low/medium/high/xhigh/max).
        // Goes into `respawnFlags` too so it survives the `/clear`/`/compact`
        // relaunch and round-trips through `state.json` for display.
        if let Some(effort) = spec.effort.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
            args.push("--effort".to_owned());
            args.push(effort.to_owned());
            respawn_flags.push("--effort".to_owned());
            respawn_flags.push(effort.to_owned());
        }
        if let Some(settings) = ensure_hook_settings(&self.cfg.hook_socket_path) {
            let settings = settings.to_string_lossy().into_owned();
            args.push("--settings".to_owned());
            args.push(settings.clone());
            respawn_flags.push("--settings".to_owned());
            respawn_flags.push(settings);
        }
        // Stage any uploaded files under /tmp/cctui-uploads/<session-id>/ and
        // prepend their absolute paths to the prompt so the worker reads them
        // (CCT-203). A staging failure is fatal to the spawn — silently dropping
        // an attachment the user expects the worker to read would be worse.
        let staged = stage_uploads(&session_id, &spec.bootstrap)?;
        let launch_prompt = if staged.is_empty() {
            spec.prompt.clone()
        } else {
            let mut header = String::from("Attached files:\n");
            for p in &staged {
                header.push_str("- ");
                header.push_str(p);
                header.push('\n');
            }
            Some(match spec.prompt.as_deref().map(str::trim) {
                Some(b) if !b.is_empty() => format!("{header}\n{b}"),
                _ => header,
            })
        };
        if let Some(prompt) = &launch_prompt {
            args.push("--".to_owned());
            args.push(prompt.clone());
        }
        // Keep the display intent the user's original prompt/name — the staged
        // paths live in the launch arg, not the session label.
        let intent = spec.prompt.clone().or_else(|| spec.name.clone()).unwrap_or_default();

        // The daemon's seed schema is `{intent, name?, nameSource?, …}` and
        // its state.json writer reads `name`/`intent` off the seeded roster
        // entry. Seeding only `intent` (as we did before) left dispatched
        // sessions with no display name. Seed `name` + `nameSource:"user"`
        // when the caller provided one (CCT-135).
        let mut seed = serde_json::Map::new();
        seed.insert("intent".to_owned(), json!(intent));
        if let Some(name) = &spec.name {
            seed.insert("name".to_owned(), json!(name));
            seed.insert("nameSource".to_owned(), json!("user"));
        }

        // Environment secrets (CCT-202): merged on top of the spare's baseline
        // env in the worker process. Mirror into `reattachEnv` so they survive
        // the respawn/reattach the claude daemon drives after a CLI upgrade.
        // These values are NOT placed in `seed`/`intent`/`launch.args`, so they
        // never reach the transcript, timeline, or `state.json`.
        let env_json: serde_json::Map<String, serde_json::Value> =
            spec.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();

        let req = json!({
            "proto": 1,
            "op": "dispatch",
            "timeoutMs": 15000,
            "d": {
                "proto": 1,
                "short": short,
                "nonce": nonce,
                "sessionId": session_id,
                "createdAt": created_at,
                "source": "fleet",
                "cwd": cwd,
                "launch": { "mode": "prompt", "args": args },
                "env": env_json,
                "reattachEnv": env_json,
                "isolation": "none",
                "respawnFlags": respawn_flags,
                "agent": agent,
                "seed": seed,
                "cols": 120,
                "rows": 40,
            }
        });

        // `call` (not `one_shot`) so an `ok:false` reply becomes an Err that
        // propagates back to the client instead of being logged as success.
        let resp: serde_json::Value =
            socket::call(sock, &req).await.with_context(|| format!("dispatch spawn in {cwd}"))?;
        tracing::info!(?resp, %cwd, %session_id, "spawn dispatched via control socket");
        Ok(())
    }

    /// Locate the control socket, booting the on-demand claude daemon if it's
    /// missing and waiting (up to ~12s) for the socket to appear. Used on the
    /// command path, where failing to find a socket means a dropped spawn/
    /// reply rather than just a skipped poll (CCT-194). `claude daemon run`
    /// needs a few seconds to start the supervisor and bind the socket, so the
    /// window is generous.
    async fn ensure_socket(&self) -> anyhow::Result<PathBuf> {
        if let Some(sock) = self.cfg.discovery.locate_live().await {
            return Ok(sock);
        }
        self.kickstarter.kick(true);
        for _ in 0..120 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Some(sock) = self.cfg.discovery.locate_live().await {
                return Ok(sock);
            }
        }
        anyhow::bail!("no claude daemon socket present (kickstart did not bring it up in time)");
    }

    async fn poll_once(&mut self) -> anyhow::Result<()> {
        let Some(sock) = self.cfg.discovery.locate_live().await else {
            // Daemon isn't running. Boot it (rate-limited) so it self-heals
            // before the next dispatch (CCT-194), and treat any sessions we
            // previously knew about as ended.
            self.kickstarter.kick(false);
            self.flush_roster(EndReason::Other { detail: "daemon gone".into() }).await;
            return Ok(());
        };

        let resp: ListResponse = socket::call(&sock, &json!({"proto": 1, "op": "list"})).await?;
        self.apply_snapshot(resp.jobs).await;
        Ok(())
    }

    #[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
    async fn apply_snapshot(&mut self, jobs: Vec<LiveSnapshot>) {
        let visible: Vec<LiveSnapshot> =
            jobs.into_iter().filter(LiveSnapshot::is_user_visible).collect();
        let now_shorts: HashSet<String> = visible.iter().map(|j| j.short.clone()).collect();

        // Newly started.
        for job in &visible {
            if !self.roster.contains(&job.short) {
                let session_id = job.session_id().map_or_else(|| job.short.clone(), str::to_owned);
                self.short_by_session.insert(session_id.clone(), job.short.clone());
                self.emit(AdapterEvent::SessionStarted {
                    local_id: session_id,
                    meta: SessionMeta {
                        working_dir: job.cwd.clone(),
                        extra: json!({
                            "short": job.short,
                            "cli_version": job.cli_version,
                        }),
                        ..SessionMeta::default()
                    },
                })
                .await;
            }
        }

        // Status updates (live snapshot + on-disk state.json reconciliation).
        for job in &visible {
            // The emitted `local_id` is STABLE for a worker's whole life. Once a
            // transcript is pinned we keep reusing its `local_id` even when the
            // session id rotates in place (`/clear`, `/compact`), so every
            // message lands in the one session the server already knows. Only
            // the very first pin derives the id from the live `session_id`.
            let local_id = self
                .transcript_locations
                .get(&job.short)
                .map(|loc| loc.local_id.clone())
                .or_else(|| job.session_id().map(str::to_owned))
                .unwrap_or_else(|| job.short.clone());
            let on_disk = StateJson::read(&self.cfg.jobs_root, &job.short);

            // Surface (or clear) a tool-permission prompt from the live
            // `tempo`/`needs` signal (CCT-211), before the Status emit below.
            self.reconcile_permission(
                &job.short,
                &local_id,
                job.tempo.as_deref(),
                job.needs.as_deref(),
            )
            .await;

            // Pin (or re-pin) the transcript location. A resume or an in-process
            // reset (`/clear`, `/compact`) changes the session's `sessionId` and
            // starts a NEW transcript file (`<newId>.jsonl`); if we kept tailing
            // the original file the message stream would silently stop while
            // `list`/Status polls kept the heartbeat fresh (CCT-128). So re-pin
            // whenever the live `session_id` differs from the one we cached,
            // following the transcript to the new file.
            //
            // CCT-158: a reset keeps the same worker `short`, so the "Newly
            // started" branch never fires for the new id. We deliberately keep
            // emitting under the ORIGINAL `local_id` (set on the first pin, kept
            // in `loc.local_id`) and only move `path`/`offset_key` to the new
            // file — so the post-reset transcript appends to the one session the
            // server already knows. Splitting it into a second session would be
            // worse: archive is worker-scoped (`claude rm <short>`), so a single
            // archive would wipe both conversations at once. Instead we inject a
            // `context_reset` boundary marker so the cut is visible in the UI.
            // CCT-160: `/clear` rotates the live session into a new transcript
            // file but the control socket's `list` keeps reporting the stale
            // spawn `sessionId` (it's the immutable `--session-id` launch arg in
            // `roster.json`). The rotated id only surfaces in `state.json`'s
            // `resumeSessionId`, so prefer that; fall back to the snapshot id
            // when no reset has happened. Without this the rotation check below
            // never fires for `/clear` and the message stream silently stops.
            let live_session = on_disk
                .as_ref()
                .and_then(|s| s.resume_session_id.as_deref())
                .or_else(|| job.session_id());
            if let (Some(cwd), Some(sess)) = (job.cwd.as_deref(), live_session) {
                let rotated = self
                    .transcript_locations
                    .get(&job.short)
                    .is_some_and(|loc| loc.offset_key != sess);
                let first_pin = !self.transcript_locations.contains_key(&job.short);
                let path = transcript::transcript_path(&self.cfg.projects_root, cwd, sess);
                if first_pin {
                    self.short_by_session.insert(sess.to_owned(), job.short.clone());
                    self.map_session(sess, &local_id);
                    self.transcript_locations.insert(
                        job.short.clone(),
                        TranscriptLocation {
                            path,
                            local_id: local_id.clone(),
                            cwd: cwd.to_owned(),
                            offset_key: sess.to_owned(),
                        },
                    );
                } else if rotated {
                    // Follow the file, keep the stable `local_id`. The new
                    // `sess` is mapped to the same `short` too so command
                    // dispatch keeps working if a snapshot ever reports the new
                    // id directly. The rotated id maps to the unchanged stable
                    // `local_id` so a hook firing post-`/clear` still resolves
                    // to the session the server knows (CCT-167).
                    self.short_by_session.insert(sess.to_owned(), job.short.clone());
                    self.map_session(sess, &local_id);
                    if let Some(loc) = self.transcript_locations.get_mut(&job.short) {
                        loc.path = path;
                        sess.clone_into(&mut loc.offset_key);
                    }
                    self.emit(AdapterEvent::Message {
                        local_id: local_id.clone(),
                        payload: json!({
                            "role": "context_reset",
                            "text": "context reset (/clear · /compact)",
                            // The new session id keys this marker uniquely so a
                            // second reset isn't collapsed by the server's
                            // content-hash dedup (identical text would hash the
                            // same).
                            "session_id": sess,
                        }),
                    })
                    .await;
                }
            }

            let name = on_disk.as_ref().and_then(|s| s.name.clone()).or_else(|| job.name.clone());
            let intent =
                on_disk.as_ref().and_then(|s| s.intent.clone()).or_else(|| job.intent.clone());
            let activity = on_disk.as_ref().and_then(|s| s.activity.clone());
            let model = on_disk.as_ref().and_then(|s| s.model.clone());
            let effort = on_disk.as_ref().and_then(|s| s.effort.clone());
            let children = on_disk.as_ref().map(StateJson::proto_children).unwrap_or_default();

            // NB: live `AskUserQuestion` surfacing is NOT derived from status
            // here. The earlier `blocked`+`detail` heuristic (CCT-164) was wrong
            // in both directions — it missed real questions (which report
            // `state:"done"`, not `blocked`) and fired on any other `blocked`
            // state (e.g. a background "needs input" status, whose `detail` is a
            // headline, not a question). The `AskUserQuestion` PreToolUse hook
            // delivers the real prompt over the daemon socket instead (CCT-167).

            let snap = StatusSnapshot {
                tempo: job.tempo.clone(),
                state: job.state.clone(),
                detail: job.detail.clone(),
                name: name.clone(),
                activity: activity.clone(),
                model: model.clone(),
                effort: effort.clone(),
            };
            let changed = self.last_status.get(&job.short) != Some(&snap);
            if changed {
                self.last_status.insert(job.short.clone(), snap);
                self.emit(AdapterEvent::Status {
                    local_id,
                    tempo: job.tempo.clone(),
                    state: job.state.clone(),
                    detail: job.detail.clone(),
                    activity,
                    name,
                    intent,
                    model,
                    effort,
                    children,
                })
                .await;
            }
        }

        // Tail transcripts for every visible session and emit new events.
        // Done after Status updates so the UI's identity fields land
        // before the message stream they describe.
        let mut dirty_offsets = false;
        let locations: Vec<TranscriptLocation> =
            self.transcript_locations.values().cloned().collect();
        for loc in locations {
            let off = self.offsets.get(&loc.offset_key);
            match transcript::tail_once(&loc.path, &loc.local_id, off) {
                Ok((events, new_off)) => {
                    if new_off != off {
                        self.offsets.set(loc.offset_key.clone(), new_off);
                        dirty_offsets = true;
                    }
                    for evt in events {
                        self.emit(evt).await;
                    }
                }
                Err(err) => {
                    tracing::debug!(%err, path = %loc.path.display(), "transcript tail failed");
                }
            }
        }
        // Discover + tail Task-tool subagents nested under each live parent
        // (CCT-141). Runs after the parent tail so a subagent's parent row
        // exists before its own SessionStarted references it.
        self.scan_subagents(&mut dirty_offsets).await;

        if dirty_offsets {
            self.offsets.flush();
        }

        // Ended sessions.
        let gone: Vec<String> = self.roster.difference(&now_shorts).cloned().collect();
        for short in &gone {
            self.last_status.remove(short);
            self.clear_permission(short).await;
            if let Some(loc) = self.transcript_locations.remove(short) {
                // Hibernated, not gone (CCT-228): the worker process exited but
                // its job state survives on disk, so a reply will revive it
                // (resume-on-reply above). Mark the session so the UI can show
                // the claude-style "exited, will resume on reply" red dot
                // instead of a plain dead one. Carried in `tempo` (not
                // `agent_state`) so the bucket classifier still sees the final
                // state (`done` → Completed); a revived worker's next live
                // snapshot overwrites it.
                if StateJson::read(&self.cfg.jobs_root, short).is_some() {
                    self.emit(AdapterEvent::Status {
                        local_id: loc.local_id.clone(),
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
                    .await;
                }
                self.short_by_session.remove(&loc.local_id);
                self.session_to_local
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .retain(|_, v| v != &loc.local_id);
                self.end_subagents_of(&loc.local_id).await;
            }
            // We don't retain the session_id mapping after roster removal,
            // so fall back to the short as the local_id. Sessions on the
            // server side are indexed by (machine_id, adapter_id,
            // local_id), and the prior SessionStarted carried the real
            // session_id; the server reconciles on the running row.
            self.emit(AdapterEvent::SessionEnded {
                local_id: short.clone(),
                reason: EndReason::Completed,
            })
            .await;
        }

        // Keep a headless `attach` open for every live session so the worker
        // stays focused/awake and `reply` actually drives its PTY (CCT-209).
        self.attach.reconcile(now_shorts.iter().map(String::as_str));

        self.roster = now_shorts;
    }

    /// Discover and tail Task-tool subagents for every live parent session
    /// (CCT-141). Each subagent transcript lives at
    /// `<encoded-cwd>/<parent-session-id>/subagents/agent-<agentId>.jsonl`
    /// and reuses the standard transcript parser. Subagents are observe-only
    /// (no worker `short` → no command dispatch); lifecycle end is inferred
    /// from transcript quiescence.
    async fn scan_subagents(&mut self, dirty_offsets: &mut bool) {
        // Snapshot parent locations to avoid borrowing `self` across the
        // `emit`/offset mutations below.
        let parents: Vec<(String, PathBuf, String)> = self
            .transcript_locations
            .values()
            .map(|loc| (loc.offset_key.clone(), loc.path.clone(), loc.cwd.clone()))
            .collect();

        for (parent_id, parent_path, cwd) in parents {
            let dir = transcript::subagents_dir(&parent_path);
            for (agent_id, path) in transcript::discover_subagents(&dir) {
                if self.ended_subagents.contains(&agent_id) {
                    continue;
                }
                if !self.subagents.contains_key(&agent_id) {
                    self.subagents.insert(
                        agent_id.clone(),
                        SubagentState { parent_local_id: parent_id.clone(), idle_ticks: 0 },
                    );
                    self.emit(AdapterEvent::SessionStarted {
                        local_id: agent_id.clone(),
                        meta: SessionMeta {
                            working_dir: Some(cwd.clone()),
                            parent_local_id: Some(parent_id.clone()),
                            extra: json!({ "subagent": true, "agent_id": agent_id }),
                        },
                    })
                    .await;
                }

                let off = self.offsets.get(&agent_id);
                match transcript::tail_once(&path, &agent_id, off) {
                    Ok((events, new_off)) => {
                        let grew = new_off != off;
                        if grew {
                            self.offsets.set(agent_id.clone(), new_off);
                            *dirty_offsets = true;
                        }
                        for evt in events {
                            self.emit(evt).await;
                        }
                        if let Some(st) = self.subagents.get_mut(&agent_id) {
                            st.idle_ticks = if grew { 0 } else { st.idle_ticks + 1 };
                        }
                    }
                    Err(err) => {
                        tracing::debug!(%err, path = %path.display(), "subagent tail failed");
                    }
                }
            }
        }

        // Quiescence-based end: a subagent whose transcript has not grown for
        // SUBAGENT_IDLE_TICKS_TO_END consecutive polls has finished.
        let done: Vec<String> = self
            .subagents
            .iter()
            .filter(|(_, st)| st.idle_ticks >= SUBAGENT_IDLE_TICKS_TO_END)
            .map(|(id, _)| id.clone())
            .collect();
        for agent_id in done {
            self.subagents.remove(&agent_id);
            self.ended_subagents.insert(agent_id.clone());
            self.emit(AdapterEvent::SessionEnded {
                local_id: agent_id,
                reason: EndReason::Completed,
            })
            .await;
        }
    }

    /// End any still-tracked subagents whose parent has left the roster — a
    /// backstop for the quiescence heuristic so children never outlive their
    /// parent.
    async fn end_subagents_of(&mut self, parent_local_id: &str) {
        let orphans: Vec<String> = self
            .subagents
            .iter()
            .filter(|(_, st)| st.parent_local_id == parent_local_id)
            .map(|(id, _)| id.clone())
            .collect();
        for agent_id in orphans {
            self.subagents.remove(&agent_id);
            self.ended_subagents.insert(agent_id.clone());
            self.emit(AdapterEvent::SessionEnded {
                local_id: agent_id,
                reason: EndReason::Completed,
            })
            .await;
        }
    }

    async fn flush_roster(&mut self, reason: EndReason) {
        // The daemon/socket is gone — stop dialing it from every attach task.
        self.attach.cancel_all();
        let shorts: Vec<String> = self.roster.drain().collect();
        self.last_status.clear();
        for short in shorts {
            self.clear_permission(&short).await;
            self.emit(AdapterEvent::SessionEnded { local_id: short, reason: reason.clone() }).await;
        }
    }

    /// Record `session_id → local_id` in the shared map the ask-hook listener
    /// reads. Lock poisoning is non-fatal here (the map is best-effort routing
    /// metadata), so we recover the guard rather than panic (CCT-167).
    fn map_session(&self, session_id: &str, local_id: &str) {
        let mut guard =
            self.session_to_local.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.insert(session_id.to_owned(), local_id.to_owned());
    }

    /// Reconcile the pending tool-permission prompt for one worker against the
    /// live snapshot (CCT-211). A `needs` of `"approve <Tool>: <detail>"` (set
    /// while the worker is `tempo:"blocked"`) is a permission prompt; a fresh or
    /// changed one emits `PermissionRequest`, and clearing it emits
    /// `PermissionResolved`. Deduped so an unchanged prompt isn't re-emitted on
    /// every 2s poll.
    async fn reconcile_permission(
        &mut self,
        short: &str,
        local_id: &str,
        tempo: Option<&str>,
        needs: Option<&str>,
    ) {
        let pending_needs = match needs.map(str::trim) {
            // `tempo:"blocked"` + an `approve …` need is the interactive
            // tool-permission prompt. Other `needs`/blocked states (e.g. a
            // background "needs input") are not permission prompts.
            Some(n) if tempo == Some("blocked") && n.starts_with("approve ") => Some(n.to_owned()),
            _ => None,
        };

        match pending_needs {
            Some(n) => {
                // Already surfaced this exact prompt? Nothing to do.
                if self.pending_perms.get(short).is_some_and(|p| p.needs == n) {
                    return;
                }
                // A changed `needs` means the prior prompt was superseded —
                // resolve it before emitting the new one so no stale card lingers.
                if let Some(prev) = self.pending_perms.remove(short) {
                    self.emit(AdapterEvent::PermissionResolved {
                        local_id: prev.local_id,
                        request_id: prev.request_id,
                    })
                    .await;
                }
                self.perm_seq += 1;
                let request_id = format!("{short}#perm{}", self.perm_seq);
                let (tool, description) = parse_permission_needs(&n);
                self.pending_perms.insert(
                    short.to_owned(),
                    PendingPerm {
                        request_id: request_id.clone(),
                        local_id: local_id.to_owned(),
                        needs: n.clone(),
                    },
                );
                self.emit(AdapterEvent::PermissionRequest {
                    local_id: local_id.to_owned(),
                    request_id,
                    tool,
                    input: json!({ "description": description, "needs": n }),
                })
                .await;
            }
            None => {
                if let Some(prev) = self.pending_perms.remove(short) {
                    self.emit(AdapterEvent::PermissionResolved {
                        local_id: prev.local_id,
                        request_id: prev.request_id,
                    })
                    .await;
                }
            }
        }
    }

    /// Drop any pending permission for a worker that left the roster, emitting a
    /// `PermissionResolved` so clients dismiss a prompt whose session is gone.
    async fn clear_permission(&mut self, short: &str) {
        if let Some(prev) = self.pending_perms.remove(short) {
            self.emit(AdapterEvent::PermissionResolved {
                local_id: prev.local_id,
                request_id: prev.request_id,
            })
            .await;
        }
    }

    async fn emit(&self, evt: AdapterEvent) {
        let _ = self.events.send(evt).await;
    }
}

/// Parse a permission `needs` string (`"approve <Tool>: <detail>"`) into a
/// `(tool, description)` pair. Falls back to the whole remainder as both tool
/// and description when there's no `": "` separator.
fn parse_permission_needs(needs: &str) -> (String, String) {
    let rest = needs.strip_prefix("approve ").unwrap_or(needs).trim();
    match rest.split_once(": ") {
        Some((tool, detail)) => (tool.trim().to_owned(), detail.trim().to_owned()),
        None => (rest.to_owned(), rest.to_owned()),
    }
}

/// Path of the managed hook settings file: `$XDG_CONFIG_HOME/cctui/
/// ask-hook-settings.json` (falling back to `~/.config`).
/// Map a numeric kill signal to the string name Claude's control-socket `kill`
/// op accepts. The op validates `signal` against the zod enum
/// `["SIGTERM","SIGKILL"]`, so a numeric value is rejected outright (CCT-169).
/// Only `SIGKILL` (9) maps to a hard kill; everything else (notably the
/// interrupt route's `15`) maps to the graceful `SIGTERM`.
const fn kill_signal_name(signal: i32) -> &'static str {
    if signal == 9 { "SIGKILL" } else { "SIGTERM" }
}

/// Decode + stage `bootstrap` file uploads (CCT-203) under
/// `/tmp/cctui-uploads/<session-id>/`, returning their absolute paths in upload
/// order. Files are written 0600 with sanitized bare names; an empty/null
/// bootstrap yields an empty vec. Errors (bad base64, unwritable dir) abort the
/// spawn so the user learns the attachment didn't land rather than the worker
/// silently starting without it.
fn stage_uploads(session_id: &str, bootstrap: &serde_json::Value) -> anyhow::Result<Vec<String>> {
    use base64::Engine;

    if bootstrap.is_null() {
        return Ok(Vec::new());
    }
    let parsed: cctui_proto::adapter::BootstrapUploads =
        serde_json::from_value(bootstrap.clone()).context("decoding bootstrap uploads")?;
    if parsed.uploads.is_empty() {
        return Ok(Vec::new());
    }
    let dir = std::path::Path::new("/tmp/cctui-uploads").join(session_id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating upload dir {}", dir.display()))?;
    let mut paths = Vec::with_capacity(parsed.uploads.len());
    for file in &parsed.uploads {
        // Defensive re-sanitize: the server already strips path separators, but
        // never trust a wire-supplied name when it becomes a filesystem path.
        let name = std::path::Path::new(&file.name)
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|n| !n.is_empty() && *n != ".." && *n != ".")
            .ok_or_else(|| anyhow::anyhow!("unsafe upload filename: {:?}", file.name))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(file.content_b64.as_bytes())
            .with_context(|| format!("base64-decoding upload {name}"))?;
        let path = dir.join(name);
        std::fs::write(&path, &bytes)
            .with_context(|| format!("writing upload {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("chmod 0600 {}", path.display()))?;
        }
        paths.push(path.to_string_lossy().into_owned());
    }
    tracing::info!(%session_id, count = paths.len(), "staged uploaded files for spawn");
    Ok(paths)
}

fn hook_settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("cctui").join("ask-hook-settings.json"))
}

/// Write (idempotently, on every spawn so it tracks binary upgrades) the
/// managed Claude Code settings file that registers the `AskUserQuestion`
/// PreToolUse/PostToolUse hooks, pointing at this daemon binary and the given
/// delivery socket (CCT-167). Returns the file path to inject via `--settings`,
/// or `None` if we can't locate the binary / config dir (in which case spawning
/// proceeds without the hook rather than failing).
fn ensure_hook_settings(sock: &std::path::Path) -> Option<PathBuf> {
    let path = hook_settings_path()?;
    let exe = std::env::current_exe()
        .map_err(|err| tracing::warn!(%err, "ask-hook: cannot resolve current_exe"))
        .ok()?;
    let exe = exe.to_string_lossy();
    let sock = sock.to_string_lossy();
    let hook = |event: &str| {
        json!([{
            "matcher": "AskUserQuestion",
            "hooks": [{
                "type": "command",
                "command": format!("{exe} ask-hook --event {event} --sock {sock}"),
                "timeout": 5,
            }],
        }])
    };
    let settings = json!({
        "hooks": { "PreToolUse": hook("pre"), "PostToolUse": hook("post") },
    });
    if let Some(Err(err)) = path.parent().map(std::fs::create_dir_all) {
        tracing::warn!(%err, "ask-hook: cannot create settings dir");
        return None;
    }
    match std::fs::write(&path, serde_json::to_vec_pretty(&settings).ok()?) {
        Ok(()) => Some(path),
        Err(err) => {
            tracing::warn!(%err, path = %path.display(), "ask-hook: cannot write settings");
            None
        }
    }
}

/// Translate a structured ask answer into the keystroke chunks that drive the
/// real `AskUserQuestion` form (CCT-226). `questions` is the raw
/// `tool_input.questions` array captured by the ask-hook; `picks` is one list
/// of 0-based option indices per question, in question order.
///
/// Form grammar (verified live against claude 2.1.162):
///   - single-select: the option digit (`1`-`9`) selects and auto-advances
///   - multiSelect: digits toggle options; `Tab` advances to the next question
///   - every form except a lone single-select question ends on a "Review your
///     answers" screen whose option 1 is "Submit answers" → final `1` submits
///
/// Returns `None` when the answer can't be expressed as form keystrokes
/// (count mismatch, out-of-range/duplicate picks, empty pick on a question,
/// several picks on a single-select) — the caller then falls back to the
/// dismiss-then-reply path, which handles free-text answers too.
fn ask_keystrokes(questions: &serde_json::Value, picks: &[Vec<usize>]) -> Option<Vec<Vec<u8>>> {
    let qs = questions.as_array()?;
    if qs.is_empty() || qs.len() != picks.len() {
        return None;
    }
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut any_multi = false;
    for (q, p) in qs.iter().zip(picks) {
        let n_opts = q.get("options").and_then(serde_json::Value::as_array)?.len();
        // Digits only address rows 1-9; real forms have ≤4 options, so >9 means
        // we're misreading the payload — bail to the fallback.
        if n_opts == 0 || n_opts > 9 || p.is_empty() || p.iter().any(|&i| i >= n_opts) {
            return None;
        }
        if q.get("multiSelect").and_then(serde_json::Value::as_bool).unwrap_or(false) {
            any_multi = true;
            let mut sorted = p.clone();
            sorted.sort_unstable();
            sorted.dedup();
            if sorted.len() != p.len() {
                return None; // duplicate picks — toggling twice would deselect
            }
            for &i in &sorted {
                chunks.push(vec![b'1' + u8::try_from(i).ok()?]);
            }
            chunks.push(vec![b'\t']); // advance to the next question / review
        } else {
            if p.len() != 1 {
                return None;
            }
            chunks.push(vec![b'1' + u8::try_from(p[0]).ok()?]);
        }
    }
    // The review screen ("1. Submit answers") shows for every form except a
    // lone single-select question, which submits straight from its digit.
    if qs.len() > 1 || any_multi {
        chunks.push(vec![b'1']);
    }
    Some(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_keystrokes_single_question_single_select() {
        // One single-select question: the digit submits directly, no review.
        let qs =
            json!([{ "question": "Red or blue?", "options": [{"label":"Red"},{"label":"Blue"}] }]);
        assert_eq!(ask_keystrokes(&qs, &[vec![1]]), Some(vec![b"2".to_vec()]));
    }

    #[test]
    fn ask_keystrokes_multiselect_tabs_then_submits() {
        // One multiSelect question: toggle digits, Tab to review, 1 submits.
        let qs = json!([{ "question": "Which?", "multiSelect": true,
            "options": [{"label":"A"},{"label":"B"},{"label":"C"}] }]);
        assert_eq!(
            ask_keystrokes(&qs, &[vec![2, 0]]), // unsorted on purpose
            Some(vec![b"1".to_vec(), b"3".to_vec(), b"\t".to_vec(), b"1".to_vec()])
        );
    }

    #[test]
    fn ask_keystrokes_multi_question_ends_on_review() {
        // multiSelect then single-select: toggles+Tab, digit, then review `1`.
        let qs = json!([
            { "question": "Fruits?", "multiSelect": true,
              "options": [{"label":"Apple"},{"label":"Banana"},{"label":"Cherry"}] },
            { "question": "Drink?", "options": [{"label":"Tea"},{"label":"Coffee"}] },
        ]);
        assert_eq!(
            ask_keystrokes(&qs, &[vec![0, 2], vec![1]]),
            Some(vec![b"1".to_vec(), b"3".to_vec(), b"\t".to_vec(), b"2".to_vec(), b"1".to_vec()])
        );
    }

    #[test]
    fn ask_keystrokes_rejects_unanswerable_shapes() {
        let qs = json!([{ "question": "Q", "options": [{"label":"A"},{"label":"B"}] }]);
        // count mismatch / empty pick / out of range / multi-pick on single-select
        assert_eq!(ask_keystrokes(&qs, &[]), None);
        assert_eq!(ask_keystrokes(&qs, &[vec![]]), None);
        assert_eq!(ask_keystrokes(&qs, &[vec![2]]), None);
        assert_eq!(ask_keystrokes(&qs, &[vec![0, 1]]), None);
        // duplicate toggles on multiSelect would cancel out
        let mq = json!([{ "question": "Q", "multiSelect": true,
            "options": [{"label":"A"},{"label":"B"}] }]);
        assert_eq!(ask_keystrokes(&mq, &[vec![0, 0]]), None);
        // not an array at all
        assert_eq!(ask_keystrokes(&json!({}), &[vec![0]]), None);
    }

    #[test]
    fn kill_signal_name_maps_to_claude_enum() {
        // The interrupt route sends 15; kill_session sends None (handled at the
        // call site). Anything that is not SIGKILL must map to SIGTERM so it
        // satisfies claude's `["SIGTERM","SIGKILL"]` enum (CCT-169 regression:
        // a numeric signal was rejected, making interrupt a silent no-op).
        assert_eq!(kill_signal_name(15), "SIGTERM");
        assert_eq!(kill_signal_name(9), "SIGKILL");
        assert_eq!(kill_signal_name(2), "SIGTERM");
    }

    fn snap(short: &str, state: &str, name: Option<&str>) -> LiveSnapshot {
        LiveSnapshot {
            short: short.into(),
            session_id: Some(format!("{short}-uuid")),
            session_id_camel: None,
            cwd: Some("/tmp".into()),
            tempo: Some("active".into()),
            state: Some(state.into()),
            detail: None,
            needs: None,
            name: name.map(String::from),
            intent: None,
            source: Some("shell".into()),
            dying: false,
            cli_version: Some("2.1.145".into()),
        }
    }

    #[test]
    fn stage_uploads_writes_sanitized_0600_files() {
        use base64::Engine;
        use std::os::unix::fs::PermissionsExt;

        let session_id = format!("test-{}", uuid::Uuid::new_v4());
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
        // A normal name and a traversal attempt that must collapse to its basename.
        let bootstrap = json!({
            "uploads": [
                { "name": "notes.txt", "content_b64": b64("hello world") },
                { "name": "../../etc/evil", "content_b64": b64("nope") },
            ]
        });

        let paths = stage_uploads(&session_id, &bootstrap).expect("stage ok");
        assert_eq!(paths.len(), 2);
        let dir = std::path::Path::new("/tmp/cctui-uploads").join(&session_id);

        let notes = dir.join("notes.txt");
        assert!(paths.contains(&notes.to_string_lossy().into_owned()));
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "hello world");
        let mode = std::fs::metadata(&notes).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "uploaded file must be 0600");

        // Traversal collapsed to the bare basename inside the staging dir.
        let evil = dir.join("evil");
        assert!(evil.exists(), "traversal name must be reduced to a basename in-dir");
        assert!(!std::path::Path::new("/tmp/cctui-uploads").join("../../etc/evil").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_uploads_null_bootstrap_is_empty() {
        assert!(stage_uploads("sid", &serde_json::Value::Null).unwrap().is_empty());
    }

    fn driver() -> (Driver, mpsc::Receiver<AdapterEvent>) {
        let (tx, rx) = mpsc::channel(64);
        let (_cmd_tx, cmd_rx) = mpsc::channel(64);
        let tmp = tempfile::tempdir().unwrap().keep();
        let cfg = DriverConfig {
            poll_interval: Duration::from_millis(50),
            jobs_root: tmp.join("jobs"),
            projects_root: tmp.join("projects"),
            discovery: Discovery::with_base(tmp.join("daemon")),
            offsets_path: Some(tmp.join("offsets.json")),
            backfill_cursor_path: Some(tmp.join("backfill.json")),
            skip_backfill: true,
            claude_bin: "claude".to_string(),
            hook_socket_path: tmp.join("hook.sock"),
        };
        (Driver::new(cfg, tx, cmd_rx, CancellationToken::new()), rx)
    }

    #[tokio::test]
    async fn snapshot_emits_started_and_status() {
        let (mut d, mut rx) = driver();
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, AdapterEvent::SessionStarted { .. }));
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, AdapterEvent::Status { .. }));
    }

    #[tokio::test]
    async fn snapshot_filters_spare_and_dying() {
        let (mut d, mut rx) = driver();
        let mut spare = snap("11111111", "working", None);
        spare.source = Some("spare".into());
        let mut dying = snap("22222222", "working", None);
        dying.dying = true;
        d.apply_snapshot(vec![spare, dying]).await;
        assert!(rx.try_recv().is_err(), "filtered jobs should emit nothing");
    }

    #[tokio::test]
    async fn snapshot_emits_ended_when_short_disappears() {
        let (mut d, mut rx) = driver();
        d.apply_snapshot(vec![snap("aaaa0001", "working", None)]).await;
        // Drain Started + Status.
        rx.recv().await.unwrap();
        rx.recv().await.unwrap();
        d.apply_snapshot(vec![]).await;
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, AdapterEvent::SessionEnded { .. }));
    }

    #[tokio::test]
    async fn resolve_short_for_removal_uses_live_map_then_derives() {
        // CCT-132: removal targets completed sessions, which have already
        // dropped out of the live roster. Prefer the live reverse map, but
        // fall back to the session UUID's first group (== the short).
        let (mut d, _rx) = driver();
        let mut s = snap("deadbeef", "working", None);
        s.session_id = Some("deadbeef-1111-2222-3333-444455556666".into());
        d.apply_snapshot(vec![s]).await;
        // Live: resolved from the map.
        assert_eq!(
            d.resolve_short_for_removal("deadbeef-1111-2222-3333-444455556666").unwrap(),
            "deadbeef"
        );
        // Exited (not in the map): derived from the UUID's first group.
        assert_eq!(
            d.resolve_short_for_removal("c0ffee00-9999-8888-7777-666655554444").unwrap(),
            "c0ffee00"
        );
        // Non-hex / malformed first group: refuse rather than guess.
        assert!(d.resolve_short_for_removal("zzzzzzzz-0000").is_err());
    }

    #[tokio::test]
    async fn transcript_repins_when_session_id_changes_on_reset() {
        // CCT-128 + CCT-158: an in-process reset (`/clear`, `/compact`) or a
        // resume keeps the same `short` but gets a new `sessionId` (and a new
        // transcript file). We must follow the file to the new id, but keep
        // emitting under the ORIGINAL `local_id` so the post-reset transcript
        // appends to the one session the server already knows (splitting it
        // would let a worker-scoped `claude rm` archive wipe both at once).
        let (mut d, _rx) = driver();
        let mut s1 = snap("deadbeef", "working", None);
        s1.session_id = Some("sess-1".into());
        d.apply_snapshot(vec![s1]).await;
        let loc1 = d.transcript_locations.get("deadbeef").expect("pinned");
        assert_eq!(loc1.offset_key, "sess-1");
        assert_eq!(loc1.local_id, "sess-1");
        let path1 = loc1.path.clone();
        assert_eq!(d.short_by_session.get("sess-1").map(String::as_str), Some("deadbeef"));

        let mut s2 = snap("deadbeef", "working", None);
        s2.session_id = Some("sess-2".into());
        d.apply_snapshot(vec![s2]).await;
        let loc2 = d.transcript_locations.get("deadbeef").expect("re-pinned");
        assert_eq!(loc2.offset_key, "sess-2", "should follow the reset transcript");
        assert_ne!(loc2.path, path1, "transcript path should move to the new session id");
        assert_eq!(loc2.local_id, "sess-1", "local_id stays stable across the reset");
        // Both ids resolve to the worker for command dispatch.
        assert_eq!(d.short_by_session.get("sess-2").map(String::as_str), Some("deadbeef"));
        assert_eq!(d.short_by_session.get("sess-1").map(String::as_str), Some("deadbeef"));
    }

    #[tokio::test]
    async fn reset_emits_boundary_marker_under_original_session() {
        // CCT-158: a reset must not start/end a session — it injects a single
        // `context_reset` marker under the original `local_id` so the cut is
        // visible while the stream stays in one session.
        let (mut d, mut rx) = driver();
        let mut s1 = snap("deadbeef", "working", None);
        s1.session_id = Some("sess-1".into());
        d.apply_snapshot(vec![s1]).await;
        // Drain the first SessionStarted + Status.
        while rx.try_recv().is_ok() {}

        let mut s2 = snap("deadbeef", "working", None);
        s2.session_id = Some("sess-2".into());
        d.apply_snapshot(vec![s2]).await;

        let mut marker: Option<serde_json::Value> = None;
        while let Ok(evt) = rx.try_recv() {
            match evt {
                AdapterEvent::SessionStarted { .. } | AdapterEvent::SessionEnded { .. } => {
                    panic!("a reset must not start or end a session");
                }
                AdapterEvent::Message { local_id, payload }
                    if payload.get("role").and_then(|r| r.as_str()) == Some("context_reset") =>
                {
                    assert_eq!(local_id, "sess-1", "marker rides the original session");
                    marker = Some(payload);
                }
                _ => {}
            }
        }
        let payload = marker.expect("a context_reset marker should be emitted");
        // The new session id keys the marker so a second reset isn't deduped.
        assert_eq!(payload.get("session_id").and_then(|s| s.as_str()), Some("sess-2"));
    }

    /// Write a subagent transcript under the parent's `subagents/` dir so a
    /// poll discovers it. Returns the agent file path.
    fn write_subagent(d: &Driver, parent_short: &str, agent_id: &str, lines: &[&str]) {
        use std::io::Write;
        let sess = format!("{parent_short}-uuid");
        let parent_path = transcript::transcript_path(&d.cfg.projects_root, "/tmp", &sess);
        let dir = transcript::subagents_dir(&parent_path);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = std::fs::File::create(dir.join(format!("agent-{agent_id}.jsonl"))).unwrap();
        for l in lines {
            f.write_all(l.as_bytes()).unwrap();
            f.write_all(b"\n").unwrap();
        }
    }

    #[tokio::test]
    async fn subagent_discovered_as_nested_session() {
        let (mut d, mut rx) = driver();
        // Pre-create the subagent transcript so the first poll finds it.
        write_subagent(
            &d,
            "abcd1234",
            "a8412884de5cc5396",
            &[
                r#"{"type":"assistant","isSidechain":true,"agentId":"a8412884de5cc5396","message":{"content":[{"type":"text","text":"sub work"}]}}"#,
            ],
        );
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;

        let mut started_parent = false;
        let mut started_child = false;
        let mut child_text = false;
        while let Ok(evt) = rx.try_recv() {
            match evt {
                AdapterEvent::SessionStarted { local_id, meta } => {
                    if local_id == "a8412884de5cc5396" {
                        started_child = true;
                        assert_eq!(
                            meta.parent_local_id.as_deref(),
                            Some("abcd1234-uuid"),
                            "subagent must link to its parent session id"
                        );
                        assert_eq!(meta.working_dir.as_deref(), Some("/tmp"));
                    } else {
                        started_parent = true;
                    }
                }
                AdapterEvent::Message { local_id, .. } if local_id == "a8412884de5cc5396" => {
                    child_text = true;
                }
                _ => {}
            }
        }
        assert!(started_parent, "parent SessionStarted expected");
        assert!(started_child, "subagent SessionStarted expected");
        assert!(child_text, "subagent transcript should stream through");
    }

    #[tokio::test]
    async fn subagent_announced_once_then_ends_on_quiescence() {
        let (mut d, mut rx) = driver();
        write_subagent(
            &d,
            "abcd1234",
            "deadbeefcafe00001",
            &[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#],
        );
        // First poll: discover + announce.
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        // Drain.
        while rx.try_recv().is_ok() {}

        // Subsequent idle polls must NOT re-announce, and after the idle
        // threshold the subagent ends exactly once.
        let mut started_again = 0;
        let mut ended = 0;
        for _ in 0..(SUBAGENT_IDLE_TICKS_TO_END + 2) {
            d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
            while let Ok(evt) = rx.try_recv() {
                match evt {
                    AdapterEvent::SessionStarted { local_id, .. }
                        if local_id == "deadbeefcafe00001" =>
                    {
                        started_again += 1;
                    }
                    AdapterEvent::SessionEnded { local_id, .. }
                        if local_id == "deadbeefcafe00001" =>
                    {
                        ended += 1;
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(started_again, 0, "quiescent subagent must not be re-announced");
        assert_eq!(ended, 1, "subagent should end exactly once on quiescence");
    }

    #[tokio::test]
    async fn subagent_ends_when_parent_leaves_roster() {
        let (mut d, mut rx) = driver();
        write_subagent(
            &d,
            "abcd1234",
            "facefeed00001111a",
            &[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#],
        );
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        while rx.try_recv().is_ok() {}
        // Parent disappears → its subagent must end too.
        d.apply_snapshot(vec![]).await;
        let mut child_ended = false;
        while let Ok(evt) = rx.try_recv() {
            if matches!(&evt, AdapterEvent::SessionEnded { local_id, .. } if local_id == "facefeed00001111a")
            {
                child_ended = true;
            }
        }
        assert!(child_ended, "subagent should end when its parent leaves the roster");
    }

    #[tokio::test]
    async fn blocked_state_does_not_emit_phantom_ask_question() {
        // CCT-167: the old `blocked`+`detail` heuristic (CCT-164) broadcast any
        // blocked session's status `detail` as an AskQuestion — firing phantom
        // prompts for non-question states (e.g. a background "needs input"
        // status). Status no longer drives AskQuestion at all; the real prompt
        // arrives via the PreToolUse hook. A blocked snapshot must emit Status,
        // never AskQuestion.
        let (mut d, mut rx) = driver();
        let mut blocked = snap("abcd1234", "blocked", None);
        blocked.detail = Some("needs go-ahead to build & ship".into());
        d.apply_snapshot(vec![blocked]).await;

        while let Ok(evt) = rx.try_recv() {
            assert!(
                !matches!(evt, AdapterEvent::AskQuestion { .. } | AdapterEvent::AskResolved { .. }),
                "blocked status must not synthesize an Ask event"
            );
        }
    }

    #[test]
    fn parse_permission_needs_splits_tool_and_detail() {
        assert_eq!(
            parse_permission_needs("approve Bash: touch /tmp/x"),
            ("Bash".to_owned(), "touch /tmp/x".to_owned())
        );
        // No "approve " prefix, no separator → whole string as both.
        assert_eq!(parse_permission_needs("Edit"), ("Edit".to_owned(), "Edit".to_owned()));
        // Prefix but no separator.
        assert_eq!(
            parse_permission_needs("approve WebFetch"),
            ("WebFetch".to_owned(), "WebFetch".to_owned())
        );
    }

    #[tokio::test]
    async fn blocked_approve_emits_permission_request_then_resolves() {
        // CCT-211: a `tempo:"blocked"` snapshot whose `needs` reads
        // "approve <Tool>: <detail>" surfaces a PermissionRequest; clearing the
        // block (next poll) emits PermissionResolved exactly once.
        let (mut d, mut rx) = driver();
        let mut blocked = snap("abcd1234", "running", None);
        blocked.tempo = Some("blocked".into());
        blocked.needs = Some("approve Bash: touch /tmp/x".into());
        d.apply_snapshot(vec![blocked]).await;

        let mut request: Option<(String, String, String)> = None;
        while let Ok(evt) = rx.try_recv() {
            if let AdapterEvent::PermissionRequest { local_id, request_id, tool, input } = evt {
                assert_eq!(tool, "Bash");
                assert_eq!(input.get("description").and_then(|d| d.as_str()), Some("touch /tmp/x"));
                request = Some((local_id, request_id, tool));
            }
        }
        let (_, request_id, _) = request.expect("PermissionRequest expected for blocked+approve");

        // Re-poll while still blocked on the SAME prompt: no duplicate emit.
        let mut still = snap("abcd1234", "running", None);
        still.tempo = Some("blocked".into());
        still.needs = Some("approve Bash: touch /tmp/x".into());
        d.apply_snapshot(vec![still]).await;
        while let Ok(evt) = rx.try_recv() {
            assert!(
                !matches!(evt, AdapterEvent::PermissionRequest { .. }),
                "an unchanged prompt must not re-emit PermissionRequest"
            );
        }

        // Prompt clears (answered / tempo back to active) → resolve once.
        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;
        let mut resolved = 0;
        while let Ok(evt) = rx.try_recv() {
            if let AdapterEvent::PermissionResolved { request_id: rid, .. } = evt {
                assert_eq!(rid, request_id, "resolved id matches the emitted request");
                resolved += 1;
            }
        }
        assert_eq!(resolved, 1, "clearing the prompt resolves it exactly once");
    }

    #[tokio::test]
    async fn permission_resolved_when_session_ends_while_blocked() {
        // A worker that disappears mid-prompt must not leave a stale card.
        let (mut d, mut rx) = driver();
        let mut blocked = snap("abcd1234", "running", None);
        blocked.tempo = Some("blocked".into());
        blocked.needs = Some("approve Bash: rm -rf /tmp/x".into());
        d.apply_snapshot(vec![blocked]).await;
        while rx.try_recv().is_ok() {}
        d.apply_snapshot(vec![]).await;
        let mut resolved = false;
        while let Ok(evt) = rx.try_recv() {
            if matches!(evt, AdapterEvent::PermissionResolved { .. }) {
                resolved = true;
            }
        }
        assert!(resolved, "a vanished blocked session emits PermissionResolved");
    }

    #[tokio::test]
    async fn pinning_records_session_to_local_map() {
        // CCT-167: the ask-hook listener resolves a hook's live `session_id`
        // through this map. First pin maps the id to itself; a `/clear`
        // rotation maps the NEW id to the stable original `local_id`.
        let (mut d, _rx) = driver();
        let mut s1 = snap("deadbeef", "working", None);
        s1.session_id = Some("sess-1".into());
        d.apply_snapshot(vec![s1]).await;
        assert_eq!(
            d.session_map().lock().unwrap().get("sess-1").map(String::as_str),
            Some("sess-1")
        );

        let mut s2 = snap("deadbeef", "working", None);
        s2.session_id = Some("sess-2".into());
        d.apply_snapshot(vec![s2]).await;
        assert_eq!(
            d.session_map().lock().unwrap().get("sess-2").map(String::as_str),
            Some("sess-1"),
            "rotated id resolves to the stable local_id"
        );
    }

    #[tokio::test]
    async fn status_dedup_skips_unchanged_polls() {
        let (mut d, mut rx) = driver();
        d.apply_snapshot(vec![snap("c0ffee00", "working", Some("ours"))]).await;
        rx.recv().await.unwrap(); // started
        rx.recv().await.unwrap(); // status
        d.apply_snapshot(vec![snap("c0ffee00", "working", Some("ours"))]).await;
        assert!(rx.try_recv().is_err(), "identical poll should emit nothing");
    }
}
