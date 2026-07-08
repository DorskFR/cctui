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
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// Claude's per-session "process gone" flag (CCT-252). When the worker
    /// process exits while still listed (e.g. it died while the supervisor was
    /// down — "process gone while supervisor down"), claude keeps the entry in
    /// `daemon list` but marks it dead. We have no live known-dead sample of
    /// the exact wire shape, so we parse DEFENSIVELY: a boolean `gone`/`dead`
    /// flag, OR an explicit `alive: false`, OR a terminal `status` string
    /// (`gone`/`exited`/`dead`). `is_dead()` folds them together.
    #[serde(default)]
    pub gone: bool,
    #[serde(default)]
    pub dead: bool,
    /// Defensive: some builds may report liveness positively. `Some(false)`
    /// means dead; `None`/`Some(true)` mean "no signal / alive".
    #[serde(default)]
    pub alive: Option<bool>,
    /// Defensive: a free-form lifecycle string distinct from `state`/`tempo`.
    #[serde(default)]
    pub status: Option<String>,
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

    /// Whether claude reports this still-listed session as dead / "process
    /// gone" (CCT-252). Parsed DEFENSIVELY across the plausible wire shapes:
    ///   - boolean `gone` / `dead` flags,
    ///   - `alive: false`,
    ///   - a terminal `status`/`state`/`tempo` string
    ///     (`gone`/`exited`/`dead`/`process gone`),
    ///   - a `detail` that CONTAINS `process gone` — the live wire shape we
    ///     actually observed is `state:"failed"`, `tempo:"idle"`,
    ///     `detail:"process gone while supervisor was down"` (CCT-355). None of
    ///     those fields exact-match a terminal token, so the session would
    ///     otherwise linger showing that detail with a non-terminal status.
    ///
    /// `dying` is handled separately (it filters the session out entirely), so
    /// it is intentionally NOT folded in here.
    fn is_dead(&self) -> bool {
        const TERMINAL: &[&str] = &["gone", "exited", "dead", "process gone"];
        let terminal_str = |o: &Option<String>| {
            o.as_deref().is_some_and(|s| {
                let s = s.trim().to_ascii_lowercase();
                TERMINAL.contains(&s.as_str())
            })
        };
        // The "process gone" phrase arrives wrapped in a longer sentence in the
        // `detail` field (e.g. "process gone while supervisor was down"), so we
        // match it as a substring rather than an exact token.
        let detail_gone =
            self.detail.as_deref().is_some_and(|s| s.to_ascii_lowercase().contains("process gone"));
        self.gone
            || self.dead
            || self.alive == Some(false)
            || detail_gone
            || terminal_str(&self.status)
            || terminal_str(&self.state)
            || terminal_str(&self.tempo)
    }
}

/// Spawn-time `(model, effort)` pair remembered per worker `short` (CCT-299).
type SpawnModelEffort = (Option<String>, Option<String>);

pub struct Driver {
    cfg: DriverConfig,
    events: mpsc::Sender<AdapterEvent>,
    /// Inbound: commands routed from server → daemon → adapter.
    commands: mpsc::Receiver<AdapterCommand>,
    shutdown: CancellationToken,
    roster: HashSet<String>,
    last_status: HashMap<String, StatusSnapshot>,
    /// Shorts claude reports dead-but-still-listed (CCT-252). Once we emit the
    /// dead transition (hibernated or `SessionEnded`) we record the short here
    /// and suppress further live-status emits for it, so the still-present
    /// roster entry can't re-emit a non-terminal Status and re-green the dot
    /// (daemon-side sticky, mirroring the server's CCT-192 sticky terminal
    /// status). Cleared when the worker revives (reports alive again) or drops
    /// off the roster.
    dead_shorts: HashSet<String>,
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
    /// Tool-permission `PreToolUse` hooks currently parked in the ask-hook
    /// listener, long-polling for a human's decision (CCT-342). Keyed by the
    /// session's stable `local_id`. The `PermissionResponse` handler resolves
    /// the matching entry — handing the decision straight back to the blocked
    /// hook (which returns an `allow`/`deny` decision to Claude Code) — instead
    /// of attaching + injecting `1\r`/ESC keystrokes. The keystroke path is kept
    /// only as the fallback for when no hook is registered (hook timed out, or
    /// a prompt that surfaced via the legacy `tempo:"blocked"` signal).
    pending_perm_hooks: super::PendingPermHooks,
    /// When the last periodic reconciliation re-tail ran (CCT-253). The
    /// reconciler re-reads each live session's transcript from a checkpoint
    /// behind the persisted offset so a gap left by a dropped send or
    /// roster-churn re-home self-heals within one cycle; the server's
    /// content-hash dedup drops the re-emitted dups.
    last_reconcile: Instant,
    /// Set when the control socket vanished (roster flushed) so the next
    /// successful poll triggers an immediate reconciliation re-tail rather
    /// than waiting for the periodic cycle (CCT-253).
    churned: bool,
    /// Whether the first successful roster snapshot of this daemon lifetime has
    /// been grandfathered into the heal tracker (CCT-509). The in-memory
    /// `HealTracker` is empty at (re)start, so without this every session that
    /// was already alive — a self-update re-exec / sleep-wake survivor cctui
    /// launched correctly in a prior lifetime — would look like an env-less
    /// autonomous respawn and be force-killed by the proactive heal. The first
    /// snapshot's workers were brought up by a PRIOR lifetime, not by a respawn
    /// we observed, so we trust them; genuine respawns AFTER startup still arm
    /// the heal via the roster-drop `forget`. Set once on the first snapshot.
    grandfathered: bool,
    /// Spawn-time `--model`/`--effort` remembered per worker `short` (CCT-299).
    /// Used as a fallback for the Status event when `state.json` isn't on disk
    /// yet (freshly spawned) or transiently absent (`/clear` rotation), so the
    /// session list still shows the model/effort we launched the worker with.
    /// `Mutex` because `spawn` takes `&self` while the poll loop holds `&mut self`.
    spawn_model_effort: std::sync::Mutex<HashMap<String, SpawnModelEffort>>,
    /// Parent session id remembered per freshly-forked child `short` (CCT-302).
    /// `fork` dispatches a new worker but the `SessionStarted` for it is emitted
    /// later by the poll loop when the short first appears in the roster — that
    /// path has no idea it was a fork, so we stash the parent here and the
    /// roster-discovery emit reads it to set `SessionMeta::parent_local_id` (the
    /// link the server resolves into `parent_id`). `Mutex` for the same reason as
    /// `spawn_model_effort`.
    fork_parent_by_short: std::sync::Mutex<HashMap<String, String>>,
    /// Authenticated server client + machine key for the launch-time gateway-env
    /// pull (CCT-460). Every worker (re)launch resolves the session's account
    /// env here from the server's durable `sessions.account_id` binding, so
    /// routing survives a daemon / claude-daemon restart and session-id rotation
    /// instead of depending on env carried by the triggering command. `None` in
    /// tests / when no server is configured — the chokepoint then falls back to
    /// the pushed env hint.
    server: Option<crate::client::ServerClient>,
    machine_key: Option<String>,
    /// Proactive gateway-env healing for autonomously-respawned workers
    /// (CCT-462). Tracks which live workers cctui launched WITH a resolved
    /// gateway env; a live, account-bound worker missing from that set was
    /// brought up by an autonomous `claude daemon` respawn (bypassing the
    /// CCT-460 launch chokepoint) and may be env-less, so the poll loop forces
    /// a re-resume through the chokepoint. Bounded/idempotent — see
    /// [`crate::gateway_heal`]. `Mutex` because the spawn/resume chokepoint
    /// records launched-with-env under `&self` while the poll loop reads/heals
    /// under `&mut self`.
    gateway_heal: std::sync::Mutex<crate::gateway_heal::HealTracker>,
    /// Polls elapsed since the last token-validity sweep (CCT-462 finish). The
    /// sweep runs every [`TOKEN_VALIDITY_SWEEP_POLLS`] polls, probing each
    /// trusted account-bound worker's recorded token hash against the server.
    polls_since_validity_sweep: u32,
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

/// Polls between token-validity sweeps for trusted workers (CCT-462 finish).
/// At the default 2s poll cadence this probes roughly once a minute —
/// low-frequency on purpose: each sweep is one server round-trip per live
/// account-bound trusted worker, and a stale token must stay unresolvable for
/// [`crate::gateway_heal::STALE_TOKEN_STRIKES`] consecutive sweeps before the
/// (destructive) heal fires.
const TOKEN_VALIDITY_SWEEP_POLLS: u32 = 30;

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

/// Everything the launch chokepoint pulls from the server's durable binding for
/// a (re)launch: the gateway-routing `env` (CCT-460) and the per-account
/// `settings_json` (CCT-539/540) the daemon merges under its managed hook
/// settings.
#[derive(Debug, Default)]
pub(super) struct LaunchEnv {
    pub env: std::collections::BTreeMap<String, String>,
    pub settings: Option<serde_json::Value>,
}

/// Decide the launch env from a server `GatewayEnvResponse` (CCT-460), split out
/// as a pure function so the fail-closed contract is unit-testable without a
/// live server. See [`Driver::resolve_launch_env`] for the surrounding flow.
pub(super) fn launch_env_decision(
    local_id: &str,
    resp: &cctui_proto::api::GatewayEnvResponse,
    hint: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    match resp {
        // Account-bound but unmintable: refuse rather than launch a worker that
        // will silently route to the default upstream and 401.
        r if r.account_bound && r.env.is_empty() => anyhow::bail!(
            "refusing to launch {local_id}: session is account-bound but the server \
             returned no gateway env (account missing/unmintable) — launching would \
             route to the default upstream and 401 (CCT-460)"
        ),
        // Account-bound: the authoritative gateway env must win for routing, but
        // merge it OVER the pushed hint rather than replacing it, so user-supplied
        // non-gateway env (spec.env keys the gateway mint doesn't emit) survives a
        // resume / cold-resume / clear / compact / fork relaunch instead of being
        // dropped (CCT-460 follow-up). Gateway keys still override any hint of the
        // same name, so routing credentials remain authoritative.
        r if r.account_bound => {
            let mut merged = hint.clone();
            merged.extend(r.env.iter().map(|(k, v)| (k.clone(), v.clone())));
            Ok(merged)
        }
        // Not account-bound: no gateway routing required. Keep any hint (e.g.
        // user-supplied non-gateway env) but don't fail closed.
        _ => Ok(hint.clone()),
    }
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
            dead_shorts: HashSet::new(),
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
            pending_perm_hooks: super::PendingPermHooks::default(),
            last_reconcile: Instant::now(),
            churned: false,
            grandfathered: false,
            spawn_model_effort: std::sync::Mutex::new(HashMap::new()),
            fork_parent_by_short: std::sync::Mutex::new(HashMap::new()),
            server: None,
            machine_key: None,
            gateway_heal: std::sync::Mutex::new(crate::gateway_heal::HealTracker::new()),
            polls_since_validity_sweep: 0,
        }
    }

    /// Attach the authenticated server client + machine key used by the
    /// launch-time gateway-env pull (CCT-460). Builder-style so the test
    /// constructor and any future caller can omit it.
    #[must_use]
    pub fn with_server(
        mut self,
        server: Option<crate::client::ServerClient>,
        machine_key: Option<String>,
    ) -> Self {
        self.server = server;
        self.machine_key = machine_key;
        self
    }

    /// How often the periodic reconciliation re-tail runs (CCT-253). Chosen
    /// in the 30–60s band: frequent enough that a dropped-send gap self-heals
    /// quickly, infrequent enough that the re-emitted (then deduped) volume is
    /// negligible next to the regular poll tail.
    const RECONCILE_INTERVAL: Duration = Duration::from_secs(45);

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

    /// Clone handle to the shared pending tool-permission hook map, for the
    /// ask-hook listener to register blocked `PreToolUse` hooks into (CCT-342).
    pub fn pending_perm_hooks(&self) -> super::PendingPermHooks {
        self.pending_perm_hooks.clone()
    }

    #[allow(clippy::cognitive_complexity)]
    pub async fn run(mut self) -> anyhow::Result<()> {
        if !self.cfg.skip_backfill {
            self.run_backfill().await;
        }
        // Dispatched-worker bring-up (CCT-471): if this daemon was launched as a
        // dispatched kube/docker worker, self-start its session before entering
        // the poll loop. Best-effort — never aborts `run`.
        self.maybe_dispatch_on_start().await;
        let mut tick = tokio::time::interval(self.cfg.poll_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => return Ok(()),
                _ = tick.tick() => {
                    if let Err(err) = self.poll_once().await {
                        tracing::debug!(%err, "claude daemon poll failed (will retry)");
                    }
                    // Periodic reconciliation re-tail (CCT-253): catch up any
                    // transcript gap the forward-only tail left behind. Driven
                    // off the poll tick (rather than a second timer) so it
                    // can't race apply_snapshot's tail/offset updates.
                    if self.last_reconcile.elapsed() >= Self::RECONCILE_INTERVAL {
                        self.reconcile_tail().await;
                    }
                }
                Some(cmd) = self.commands.recv() => {
                    // Capture the correlation id before `cmd` is moved so we can
                    // report the outcome back to the originating client (CCT-131).
                    let command_id = match &cmd {
                        AdapterCommand::Spawn { command_id, .. }
                        | AdapterCommand::Interrupt { command_id, .. } => *command_id,
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

    /// Shorts the claude daemon currently lists as live — the jobs backfill
    /// must NOT touch (CCT-565, see `backfill::run_once`). No live socket
    /// (true cold start, claude daemon down) means nothing is live and the
    /// pass may sweep everything, as before.
    async fn live_shorts(&self) -> std::collections::HashSet<String> {
        let Some(sock) = self.cfg.discovery.locate_live().await else {
            return std::collections::HashSet::new();
        };
        match socket::call::<ListResponse>(&sock, &json!({"proto": 1, "op": "list"})).await {
            Ok(resp) => resp.jobs.into_iter().map(|j| j.short).collect(),
            Err(err) => {
                tracing::debug!(%err, "backfill live-roster list failed; treating none live");
                std::collections::HashSet::new()
            }
        }
    }

    // Linear setup (config → cursor → live roster → one pass) plus outcome
    // logging; no nesting to split.
    #[allow(clippy::cognitive_complexity)]
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
        let live_shorts = self.live_shorts().await;
        match backfill::run_once(&cfg, &live_shorts, &self.events, &mut cursor, &mut self.offsets)
            .await
        {
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
    // Sequential reply-delivery pipeline (resolve short → resume-on-reply →
    // ask-form vs text → submit) whose complexity is linear `?`-propagating I/O
    // steps plus hibernation/ENOJOB recovery branches, not nesting. Splitting risks
    // the resume/recovery control flow; kept whole deliberately.
    #[allow(clippy::cognitive_complexity)]
    async fn deliver_reply(
        &self,
        sock: &std::path::Path,
        local_id: &str,
        text: &str,
        ask_picks: Option<Vec<Vec<usize>>>,
        env: &std::collections::BTreeMap<String, String>,
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
        self.resume_if_hibernated(sock, &short, local_id, env).await?;
        // If an AskUserQuestion form is up in the worker's PTY, a bare
        // `reply` just presses Enter on the highlighted option — claude
        // records option 1 ("Proceed"-style) and the user's text is
        // swallowed (CCT-219).
        let pending_ask = self.pending_asks.lock().ok().and_then(|mut m| m.remove(local_id));
        if let Some(questions) = pending_ask {
            // Native answer first (CCT-226): drive the real form via keystrokes.
            let native_picks = ask_picks
                .as_ref()
                .and_then(|picks| questions.as_ref().and_then(|q| ask_keystrokes(q, picks)));
            if let Some(chunks) = native_picks {
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
                        // A plan prompt is stored in the same pending map; emit
                        // PlanResolved too so a live Plan card drops. Idempotent
                        // (clients only clear their own kind) (CCT-347).
                        let _ = self
                            .events
                            .send(AdapterEvent::PlanResolved { local_id: local_id.to_owned() })
                            .await;
                        return Ok(());
                    }
                    Err(err) => {
                        // CCT-278: do NOT fall through to attach+ESC here. The
                        // pending-ask record can be stale (the form already
                        // resolved in the native TUI or timed out, and the
                        // `resolved` hook hasn't reached us yet), in which case
                        // an ESC lands on whatever is now on screen — typically a
                        // running tool — and aborts the turn. That is exactly the
                        // "answering interrupted the tool" symptom. A stray text
                        // reply is harmless by comparison, so just deliver it.
                        tracing::warn!(%err, %short, "native ask answer failed; delivering text reply without ESC");
                    }
                }
            } else if ask_picks.is_none() {
                // Genuine free-text answer: the user typed prose rather than
                // picking options, so the form must be dismissed before the text
                // lands or claude records option 1 + swallows the text (CCT-219).
                // This is the only path that intentionally dismisses the form.
                if let Err(err) = socket::attach_interrupt(sock, &short).await {
                    tracing::warn!(%err, %short, "failed to dismiss pending ask form");
                } else {
                    tracing::info!(%short, "dismissed pending ask form before free-text reply");
                    // PostToolUse never fires for a cancelled ask, so synthesize
                    // `resolved` so the server/clients drop the live card.
                    let _ = self
                        .events
                        .send(AdapterEvent::AskResolved { local_id: local_id.to_owned() })
                        .await;
                    let _ = self
                        .events
                        .send(AdapterEvent::PlanResolved { local_id: local_id.to_owned() })
                        .await;
                    // Give the TUI a beat to settle after the ESC before the
                    // reply lands.
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
            }
            // Fallback paths (native answer failed, or option picks with an
            // unanswerable shape) deliver the text reply without dismissing the
            // form. The user has still answered, so tell clients to drop the live
            // card; idempotent if a real `resolved` hook follows, and a harmless
            // repeat of the free-text branch's own emit above (CCT-278).
            let _ =
                self.events.send(AdapterEvent::AskResolved { local_id: local_id.to_owned() }).await;
            let _ = self
                .events
                .send(AdapterEvent::PlanResolved { local_id: local_id.to_owned() })
                .await;
        }
        let resp =
            socket::one_shot(sock, &json!({"proto":1,"op":"reply","short":short,"text":text}))
                .await?;
        tracing::debug!(?resp, %short, "reply ack");
        if text.contains('\n')
            && let Err(err) = socket::attach_submit(sock, &short).await
        {
            tracing::warn!(%err, %short, "failed to submit multiline reply draft");
        }
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
                self.deliver_reply(
                    &sock,
                    &local_id,
                    &text,
                    None,
                    &std::collections::BTreeMap::default(),
                )
                .await?;
            }
            AdapterCommand::Reply { local_id, text, ask_picks, env } => {
                self.deliver_reply(&sock, &local_id, &text, ask_picks, &env).await?;
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
            AdapterCommand::Interrupt { local_id, .. } => {
                // Keep-alive turn interrupt (CCT-210): the control socket has
                // no turn-interrupt op, so attach to the worker PTY and inject
                // an ESC keystroke — the same key that aborts a turn in the
                // TUI. Unlike `Kill`, the worker stays live and resumable.
                let short = self.resolve_short(&local_id)?;
                socket::attach_interrupt(&sock, &short).await?;
                tracing::info!(%short, "interrupted in-flight turn via attach+ESC");
            }
            AdapterCommand::Resume { local_id, working_dir, env } => {
                let short = self
                    .resolve_short(&local_id)
                    .or_else(|_| self.resolve_short_for_removal(&local_id))?;
                // Fall back to (local_id, working_dir) when the on-disk job state
                // is gone — archiving runs `claude rm`, which deletes state.json
                // but keeps the conversation transcript, so an explicit Resume of
                // an archived session must not depend on it (CCT-345).
                self.resume_worker(
                    &sock,
                    &short,
                    &local_id,
                    Some(&local_id),
                    working_dir.as_deref(),
                    &env,
                )
                .await?;
                tracing::info!(%short, %local_id, "resumed session via explicit command");
            }
            AdapterCommand::PermissionResponse { local_id, request_id, allow } => {
                // Preferred path (CCT-342): a bidirectional `PreToolUse` hook is
                // blocked in the listener long-polling for this decision. Hand
                // it the human's allow/deny straight back — the hook returns the
                // decision to Claude Code, so the tool runs/skips with no attach
                // + keystroke at all. `take`n so a duplicate response can't
                // double-fire on an already-resolved (and dropped) channel.
                let hook =
                    self.pending_perm_hooks.lock().ok().and_then(|mut map| map.remove(&local_id));
                if let Some(tx) = hook {
                    if tx.send(allow).is_ok() {
                        tracing::info!(%local_id, %request_id, allow, "answered permission prompt via PreToolUse hook");
                        return Ok(());
                    }
                    // The hook already gave up (its wait timed out and the
                    // receiver was dropped). Fall through to the keystroke path,
                    // which handles the now-rendered native prompt.
                    tracing::debug!(%local_id, %request_id, "perm hook receiver gone; falling back to keystroke");
                }
                // Fallback (CCT-211): no hook registered (timed out, or the
                // prompt surfaced only via the legacy `tempo:"blocked"`/`needs`
                // signal). The control socket's `permission-response` op is a
                // no-op stub, so answer the way a human does — attach to the PTY
                // and inject `1`+Enter (approve) or ESC (deny).
                let short = self.resolve_short(&local_id)?;
                socket::attach_permission_response(&sock, &short, allow).await?;
                tracing::info!(%short, %request_id, allow, "answered permission prompt via attach (fallback)");
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
            AdapterCommand::Spawn { spec, session_id, .. } => {
                self.spawn(&sock, &spec, session_id.map(|id| id.to_string())).await?;
            }
            AdapterCommand::Fork { parent_local_id, spec, session_id, .. } => {
                self.fork(&sock, &parent_local_id, &spec, session_id.as_deref()).await?;
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
            AdapterCommand::SetModel { local_id, .. } => {
                // In-place model/effort switch is not supported on claude via
                // cctui's path (CCT-303): the `claude daemon` control socket has
                // no set-model op, and the Agent SDK's `setModel()` is only
                // reachable in streaming-input mode, not through this socket.
                // The supported substitute is fork-with-`--model` (CCT-302), so
                // surface a clear error the webui can route to the fork flow.
                tracing::warn!(%local_id, "claude: in-place model/effort switch not supported; fork to change model");
                anyhow::bail!(
                    "in-place model/effort switch is not supported for claude sessions — fork to change model"
                );
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
        local_id: &str,
        env: &std::collections::BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        self.resume_worker(sock, short, local_id, None, None, env).await
    }

    /// The single source of gateway-routing env for every worker (re)launch
    /// (CCT-460): pull it from the server's durable `sessions.account_id` binding
    /// so routing survives a daemon / claude-daemon restart and session-id
    /// rotation, instead of trusting whatever env the triggering command carried.
    ///
    /// `hint` is that carried env (spawn `spec.env`, reply/resume push) — used as
    /// a fallback only when the authoritative pull is unavailable (older server,
    /// transient network) so a rollout or blip degrades to the prior push
    /// behavior rather than failing.
    ///
    /// Fail-closed: when the server reports the session IS account-bound but the
    /// resolved env is empty (account gone / unmintable), refuse the launch — a
    /// worker started without the gateway credential would silently route to the
    /// default upstream and 401. Returning `Err` aborts the dispatch loudly
    /// instead of producing another silent auth drop.
    async fn resolve_launch_env(
        &self,
        local_id: &str,
        hint: &std::collections::BTreeMap<String, String>,
    ) -> anyhow::Result<LaunchEnv> {
        let (Some(server), Some(mk)) = (self.server.as_ref(), self.machine_key.as_ref()) else {
            // No server configured (tests / legacy): best-effort hint.
            return Ok(LaunchEnv { env: hint.clone(), settings: None });
        };
        match server.gateway_env(mk, local_id).await {
            // The env decision can fail closed (account-bound but unmintable);
            // the per-account settings (CCT-540) ride the same response.
            Ok(resp) => Ok(LaunchEnv {
                env: launch_env_decision(local_id, &resp, hint)?,
                settings: resp.settings,
            }),
            Err(e) => {
                // Pull unavailable (older server / transient). Degrade to the
                // pushed hint rather than blocking the launch.
                tracing::warn!(%local_id, "gateway-env pull failed; falling back to pushed env: {e}");
                Ok(LaunchEnv { env: hint.clone(), settings: None })
            }
        }
    }

    /// Proactively heal a LIVE worker that lost its gateway env to an
    /// autonomous `claude daemon` respawn (CCT-462, follow-up to CCT-460).
    ///
    /// Candidacy is cheap and local first: a worker cctui itself launched with
    /// env (recorded in the [`HealTracker`](crate::gateway_heal)) is trusted and
    /// skipped, so the normal steady state issues NO server round-trip here. For
    /// the rare worker NOT in that set we ask the server whether the session is
    /// account-bound (it alone knows the durable `sessions.account_id` binding);
    /// only an account-bound session REQUIRES gateway env and is healed. The
    /// tracker bounds this to [`MAX_HEAL_ATTEMPTS`](crate::gateway_heal::MAX_HEAL_ATTEMPTS)
    /// kill+resume cycles per session per daemon lifetime and admits at most one
    /// heal in flight, so it cannot thrash.
    ///
    /// The heal is a KILL + cold-resume: `resume_worker` no-ops on an alive
    /// worker (its `has` check), so we must terminate the env-less worker first,
    /// then cold-resume it — which routes through `resolve_launch_env` and
    /// re-seeds both `env` and `reattachEnv`.
    ///
    /// Best-effort: any failure logs, releases the in-flight latch (so a later
    /// poll retries until the cap), and never aborts the poll.
    // Linear heal pipeline (latch → fetch gateway env → re-seed env/reattachEnv)
    // with best-effort error branches at each step; complexity is the bail-out
    // logging, not nesting. Splitting would scatter the latch-release invariant.
    #[allow(clippy::cognitive_complexity)]
    async fn maybe_heal_gateway_env(&self, short: &str, local_id: &str) {
        // Cheap, non-mutating local gate: a worker cctui launched with env is
        // trusted, and one that's in-flight/parked can't heal now — skip the
        // server round-trip for all of them (the common steady state).
        if !self.gateway_heal.lock().is_ok_and(|t| t.is_candidate(short)) {
            return;
        }
        let (Some(server), Some(mk)) = (self.server.as_ref(), self.machine_key.as_ref()) else {
            return; // No server (tests / legacy): nothing to heal against.
        };
        // Only the server knows the durable `sessions.account_id` binding; a
        // non-account verdict short-circuits `should_heal` below (non-account
        // sessions need no gateway env).
        let account_bound = match server.gateway_env(mk, local_id).await {
            Ok(resp) => resp.account_bound,
            Err(err) => {
                // Server unreachable / older server — degrade silently; the next
                // poll retries. Never escalate to a heal on an unknown binding.
                tracing::debug!(%short, %local_id, "gateway-env probe for heal failed: {err}");
                return;
            }
        };
        let want_heal =
            self.gateway_heal.lock().is_ok_and(|mut t| t.should_heal(short, true, account_bound));
        if !want_heal {
            return;
        }
        tracing::warn!(
            %short, %local_id,
            "account-bound LIVE worker missing gateway env (autonomous claude-daemon respawn) — \
             forcing kill + cold-resume to re-seed env (CCT-462)"
        );
        if let Err(err) = self.force_reresume(short, local_id).await {
            tracing::warn!(%short, %local_id, "gateway-env heal failed; will retry until cap: {err}");
            if let Ok(mut t) = self.gateway_heal.lock() {
                t.note_heal_failed(short);
            }
        }
        // On success `force_reresume`'s cold-resume already recorded the worker
        // as launched-with-env via `note_launched_with_env`, clearing the
        // in-flight latch and resetting the budget.
    }

    /// Verify a trusted worker actually RECEIVED the gateway env it was
    /// dispatched with (CCT-574).
    ///
    /// `note_launched_with_env` records trust on dispatch `ok: true`, but
    /// delivery can fail inside the claude daemon (observed: a worker claimed
    /// from the pre-warmed spare pool exec'd without the dispatch `env`). On a
    /// desktop daemon that failure is SILENT — the worker falls back to the
    /// machine's ambient `~/.claude` login and bills whatever account that is,
    /// while the session's recorded account never sees the traffic. Worse, the
    /// CCT-462 heal loops forever on it: every "successful" cold-resume
    /// re-records trust, the delivery fails again, and the cycle repeats.
    ///
    /// So: for each trusted worker whose launch env carried a gateway token
    /// (`verify_hash`), check the live process environment actually holds that
    /// token (`envcheck`, Linux `/proc`; other platforms are indeterminate and
    /// keep the old behaviour). A confirmed miss ([`ENV_VERIFY_STRIKES`]
    /// consecutive polls) revokes trust — the regular heal retries delivery —
    /// bounded by the delivery-failure budget, which `note_launched_with_env`
    /// does NOT reset; at the cap the session parks with one loud error
    /// instead of thrashing.
    ///
    /// [`ENV_VERIFY_STRIKES`]: crate::gateway_heal::ENV_VERIFY_STRIKES
    fn verify_env_delivery(&self, short: &str, local_id: &str) {
        // Cheap gate: only trusted, token-carrying, not-yet-verified launches
        // within the delivery budget are checked (the common steady state is a
        // single scan right after launch, then verified → no-op).
        let Some(want_hash) = self.gateway_heal.lock().ok().and_then(|t| t.verify_hash(short))
        else {
            return;
        };
        // Indeterminate (no process found / non-Linux): try again next poll.
        let Some(carries) = super::envcheck::worker_carries_token(short, &want_hash) else {
            return;
        };
        if carries {
            if let Ok(mut t) = self.gateway_heal.lock() {
                t.note_env_observed(short);
            }
            tracing::debug!(%short, %local_id, "gateway env delivery verified in worker process");
            return;
        }
        let verdict = match self.gateway_heal.lock() {
            Ok(mut t) => t.note_env_missing(short),
            Err(_) => return,
        };
        match verdict {
            crate::gateway_heal::EnvMissing::Strike => {}
            crate::gateway_heal::EnvMissing::Revoked => tracing::warn!(
                %short, %local_id,
                "dispatched gateway env NEVER REACHED the worker process (claude-daemon \
                 env-delivery failure) — revoking trust so the heal re-delivers (CCT-574)"
            ),
            crate::gateway_heal::EnvMissing::Exhausted => tracing::error!(
                %short, %local_id,
                "🔴 gateway env delivery failed {} times — PARKING the session: its worker is \
                 running WITHOUT gateway credentials and, on a desktop daemon, is silently \
                 consuming the machine's ambient login instead of its bound account. Kill and \
                 respawn the session, and investigate the claude-daemon spare-claim env drop \
                 (CCT-574).",
                crate::gateway_heal::MAX_DELIVERY_FAILURES,
            ),
        }
    }

    /// Low-frequency token-validity sweep for TRUSTED workers (CCT-462 finish).
    ///
    /// The env-less heal above only targets workers cctui did NOT launch — a
    /// TRUSTED worker whose `session_tokens` row got unbound/deleted
    /// server-side keeps its (now dead) gateway token and 401s forever at the
    /// gateway session-token stage without ever becoming a candidate. For each
    /// live worker with a recorded launch-token hash, ask the server whether
    /// that hash still resolves; a `valid: false` confirmed
    /// [`crate::gateway_heal::STALE_TOKEN_STRIKES`] sweeps in a row revokes the
    /// worker's trust and fires the SAME bounded heal machinery (kill +
    /// cold-resume re-mints a fresh token and re-records its hash).
    ///
    /// Network errors / non-200 count as UNKNOWN, not invalid — the heal kill
    /// is destructive, so this fails open and the next sweep retries.
    // Linear probe → strike → heal pipeline mirroring `maybe_heal_gateway_env`;
    // complexity is the best-effort bail-out logging, not nesting.
    #[allow(clippy::cognitive_complexity)]
    async fn sweep_token_validity(&self, live: &[(String, String)]) {
        let (Some(server), Some(mk)) = (self.server.as_ref(), self.machine_key.as_ref()) else {
            return; // No server (tests / legacy): nothing to probe against.
        };
        for (short, local_id) in live {
            // Cheap local gate: only trusted workers with a recorded hash and
            // an available heal budget are probed (the common steady state is
            // a handful of round-trips per sweep, one per account-bound worker).
            let Some(hash) = self.gateway_heal.lock().ok().and_then(|t| t.probe_hash(short)) else {
                continue;
            };
            let valid = match server.session_token_valid(mk, local_id, &hash).await {
                Ok(resp) => resp.valid,
                Err(err) => {
                    // Unknown, NOT invalid: server unreachable / older server /
                    // non-200. Never escalate to a destructive heal on it.
                    tracing::debug!(
                        %short, %local_id,
                        "token-validity probe failed (treated as unknown): {err}"
                    );
                    continue;
                }
            };
            let want_heal = self.gateway_heal.lock().is_ok_and(|mut t| {
                if valid {
                    t.note_token_valid(short);
                    false
                } else {
                    t.note_token_invalid(short)
                }
            });
            if !want_heal {
                continue;
            }
            tracing::warn!(
                %short, %local_id,
                "trusted worker's session token no longer resolves — forcing kill + \
                 cold-resume to re-mint (CCT-462)"
            );
            if let Err(err) = self.force_reresume(short, local_id).await {
                tracing::warn!(
                    %short, %local_id,
                    "stale-token heal failed; will retry until cap: {err}"
                );
                if let Ok(mut t) = self.gateway_heal.lock() {
                    t.note_heal_failed(short);
                }
            }
            // On success `force_reresume`'s cold-resume recorded the relaunch —
            // with its freshly-minted token's hash — via `note_launched_with_env`.
        }
    }

    /// KILL a live worker, wait for it to exit, then cold-resume it through the
    /// CCT-460 chokepoint so it relaunches with freshly-resolved gateway env.
    /// Used by the proactive heal (CCT-462) — the only path that deliberately
    /// terminates an *alive* worker to re-seed its env.
    async fn force_reresume(&self, short: &str, local_id: &str) -> anyhow::Result<()> {
        let sock = self.ensure_socket().await?;
        // Pre-flight (CCT-462 hardening): NEVER kill a live worker we then can't
        // cold-resume. `resume_worker` derives the cwd from on-disk state.json;
        // if it's absent — e.g. a worker whose state.json hasn't been written
        // with a cwd yet — fall back to deriving it locally from the Claude
        // transcript (CCT-504): its entries carry an explicit `cwd`, and even
        // the project dir name encodes one. Only a true orphan (no transcript
        // either) still aborts BEFORE the kill, leaving the live worker running
        // instead of killing it into an unrecoverable state. The next poll
        // re-evaluates (bounded by the heal cap).
        let st = StateJson::read(&self.cfg.jobs_root, short);
        let mut fallback_cwd = None;
        if st.as_ref().is_none_or(|s| s.cwd.is_none()) {
            // Try every session id the worker may have written a transcript
            // under: the rotated id first (`/clear`/`/compact` moved the live
            // conversation there, CCT-160), then the spawn id, then the emitted
            // local_id (usually == spawn id, but state.json may be gone).
            let candidates = st
                .as_ref()
                .into_iter()
                .flat_map(|s| [s.resume_session_id.as_deref(), s.session_id.as_deref()])
                .flatten()
                .chain(std::iter::once(local_id));
            fallback_cwd = candidates.into_iter().find_map(|id| {
                super::fallback_cwd::derive_cwd_from_transcript(&self.cfg.projects_root, id)
            });
            let Some(cwd) = fallback_cwd.as_deref() else {
                anyhow::bail!(
                    "refusing to heal {short}: no resumable cwd in state.json and no \
                     transcript to derive one from (a kill would not be recoverable)"
                );
            };
            tracing::info!(
                %short, %local_id, %cwd,
                "state.json has no cwd; derived fallback cwd from transcript (CCT-504)"
            );
        }
        let _ = socket::one_shot(&sock, &json!({"proto":1,"op":"kill","short":short})).await;
        Self::await_worker_exit(&sock, short).await;
        // `resume_worker` reads the rotated session id + cwd from on-disk
        // state.json (kept across the kill), resolves env via the chokepoint,
        // and records launched-with-env on success. The transcript-derived cwd
        // (if any) rides along as the fallback it uses when state.json still
        // lacks one.
        self.resume_worker(
            &sock,
            short,
            local_id,
            Some(local_id),
            fallback_cwd.as_deref(),
            &std::collections::BTreeMap::default(),
        )
        .await
    }

    /// Revive an exited worker bound to its saved conversation. Prefers the
    /// on-disk `state.json` (so `/clear`/`/compact`'s rotated `resumeSessionId`
    /// is honored, CCT-160); when it's gone — e.g. an archived session whose
    /// `claude rm` deleted the job metadata but left the transcript — falls back
    /// to the caller-supplied `(session_id, cwd)` from the server's DB row
    /// (CCT-345). No-op (one cheap `has` round-trip) when the worker is alive.
    async fn resume_worker(
        &self,
        sock: &std::path::Path,
        short: &str,
        local_id: &str,
        fallback_session_id: Option<&str>,
        fallback_cwd: Option<&str>,
        env: &std::collections::BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        let alive = |resp: &serde_json::Value| {
            resp.get("alive").and_then(serde_json::Value::as_bool).unwrap_or(false)
        };
        let has = socket::one_shot(sock, &json!({"proto":1,"op":"has","short":short})).await?;
        if alive(&has) {
            return Ok(());
        }

        // Re-derive the gateway env + per-account settings from the server's
        // durable binding (CCT-460/540), falling back to the pushed `env` hint if
        // the pull is unavailable. This is the path that broke before: a
        // cold-resume relaunched the worker with empty env and the worker 401ed.
        // Fail-closed inside `resolve_launch_env`.
        let launch = self.resolve_launch_env(local_id, env).await?;
        let env = launch.env;

        // Re-apply the managed hook settings on cold resume so the revived worker
        // keeps its ask/permission/Stop hooks AND picks up the (possibly
        // refreshed) per-account settings the env pull re-served (CCT-539/540).
        // `whip` is recovered from the settings file the original spawn wrote for
        // this `short` (its `hooks.Stop` block is whip-only) — cold resume has no
        // `spec` to read it from directly, and defaulting false would silently
        // downgrade a 🐎 session's enforcement profile.
        let whip = detect_whip_from_settings(short);
        let st = StateJson::read(&self.cfg.jobs_root, short);
        // Carry the gateway env + the session's model/effort (from `state.json`)
        // into the managed `--settings` file (CCT-577) so a spare-claimed resume
        // keeps its routing env and model/effort — the settings file survives the
        // spare-claim, the dispatch `env` and a `--model` CLI arg do not.
        let settings_arg = ensure_hook_settings(
            &self.cfg.hook_socket_path,
            whip,
            short,
            launch.settings.as_ref(),
            &env,
            st.as_ref().and_then(|s| s.model.as_deref()),
            st.as_ref().and_then(|s| s.effort.as_deref()),
        )
        .map(|p| p.to_string_lossy().into_owned());

        // `/clear`/`/compact` rotate the live conversation into the id recorded
        // in `resumeSessionId`; resuming the stale spawn id would fork the
        // conversation back at the pre-reset state (CCT-160). When state.json is
        // gone, fall back to the id/cwd the server passed from its DB row.
        let session_id = st
            .as_ref()
            .and_then(|s| s.resume_session_id.clone().or_else(|| s.session_id.clone()))
            .or_else(|| fallback_session_id.map(str::to_owned))
            .ok_or_else(|| {
                anyhow::anyhow!("no session id on disk or from caller to resume {short}")
            })?;
        let cwd = st
            .as_ref()
            .and_then(|s| s.cwd.clone())
            .or_else(|| fallback_cwd.map(str::to_owned))
            .ok_or_else(|| anyhow::anyhow!("no cwd on disk or from caller to resume {short}"))?;

        let agent = "claude";
        let nonce: String = uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect();
        let created_at = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        )
        .unwrap_or(0);
        // Launch argv + respawn flags, appending the managed `--settings` file so
        // the revived worker keeps its hooks + account settings (CCT-540).
        let mut args =
            vec!["--resume".to_owned(), session_id.clone(), "--agent".to_owned(), agent.to_owned()];
        let mut respawn_flags = vec!["--agent".to_owned(), agent.to_owned()];
        // NB: resume deliberately does NOT pass `--model`/`--effort` (CCT-577
        // regression fix). Asserting `--model` on a `--resume` forces the claude
        // daemon down its spare-claim/cold relaunch, which does NOT reapply
        // cctui's dispatch gateway env (background workers don't inherit gateway
        // vars) — so the revived worker came up with no `ANTHROPIC_BASE_URL`/
        // token and 401ed/ConnectionRefused. The resumed session already carries
        // its model/effort in the transcript; only `spawn` seeds them as flags.
        if let Some(settings) = &settings_arg {
            args.push("--settings".to_owned());
            args.push(settings.clone());
            respawn_flags.push("--settings".to_owned());
            respawn_flags.push(settings.clone());
        }
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
                // Re-inject the gateway env resolved for this session's bound
                // OAuth account so the revived worker keeps routing through the
                // gateway rather than hitting the default upstream with no
                // credential and 401ing (CCT-460). Mirror into `reattachEnv` so
                // claude's own daemon reapplies it on any internal respawn
                // (`/clear`, `/compact`) while it's alive. Empty for sessions with
                // no account binding.
                "env": &env,
                "reattachEnv": &env,
                "isolation": "none",
                "respawnFlags": respawn_flags,
                "agent": agent,
                // `state.json` already exists for this short; the daemon keeps
                // its identity fields, so the seed is just protocol filler.
                "seed": { "intent": st.as_ref().and_then(|s| s.intent.clone()).unwrap_or_default() },
                "cols": 120,
                "rows": 40,
            }
        });
        let resp: serde_json::Value = socket::call(sock, &req)
            .await
            .with_context(|| format!("resume dispatch for hibernated session {short}"))?;
        tracing::info!(?resp, %short, %session_id, "resumed hibernated session via dispatch");
        // CCT-462: this cold-resume went through the chokepoint and re-seeded
        // env (+ reattachEnv); mark the worker launched-with-env so the
        // proactive heal treats it as trusted and resets any heal budget.
        self.note_launched_with_env(short, &env);

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

    /// Dispatched-worker bring-up (CCT-471).
    ///
    /// A kube/docker worker pod is a peer machine whose daemon must *start* the
    /// dispatched session itself. The server pre-mints the session id + gateway
    /// token and tells the enrolled dispatcher to spawn the pod, but — unlike a
    /// desktop machine — it sends no WS `Spawn` command, because every
    /// dispatched pod registers under the single shared `dispatch` machine row
    /// and can't be addressed individually. So when the dispatcher-injected env
    /// (`SESSION_ID` + `TASK_PAYLOAD_JSON`) is present, we self-issue the exact
    /// control-socket `dispatch` a server-driven spawn would, reusing
    /// [`Self::spawn`] and forcing the pre-minted `session_id` (CCT-446) so the
    /// gateway token resolves and the registered id matches the dispatch.
    ///
    /// Best-effort: any failure logs and lets the daemon keep observing — it
    /// never aborts `run`. A normal machine daemon lacks these env vars and is
    /// unaffected.
    // Linear startup-dispatch pipeline (read env → build spec → force session_id
    // → spawn) with best-effort bail-out logging at each step; complexity is the
    // env-validation branches, not nesting. Kept whole to preserve the dispatch flow.
    #[allow(clippy::cognitive_complexity)]
    async fn maybe_dispatch_on_start(&self) {
        let session_id = match std::env::var("SESSION_ID") {
            Ok(s) if !s.is_empty() => s,
            _ => return,
        };
        let payload_raw = match std::env::var("TASK_PAYLOAD_JSON") {
            Ok(s) if !s.is_empty() => s,
            _ => return,
        };
        let payload: serde_json::Value = match serde_json::from_str(&payload_raw) {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(%err, "dispatch-on-start: TASK_PAYLOAD_JSON is not valid JSON");
                return;
            }
        };
        let spec = match Self::build_dispatch_spec(&payload) {
            Ok(spec) => spec,
            Err(err) => {
                tracing::error!(%err, "dispatch-on-start: could not build session spec");
                return;
            }
        };
        let sock = match self.ensure_socket().await {
            Ok(s) => s,
            Err(err) => {
                tracing::error!(%err, "dispatch-on-start: claude daemon socket unavailable");
                return;
            }
        };
        tracing::info!(session_id = %session_id, "dispatch-on-start: launching dispatched session");
        if let Err(err) = self.spawn(&sock, &spec, Some(session_id.clone())).await {
            tracing::error!(%err, session_id = %session_id, "dispatch-on-start: spawn failed");
        } else {
            tracing::info!(session_id = %session_id, "dispatch-on-start: session dispatched");
        }
    }

    /// Build a [`SessionSpec`](cctui_proto::adapter::SessionSpec) from the
    /// dispatcher's `TASK_PAYLOAD_JSON` (`prompt_file`/`prompt`, `model`,
    /// `effort`, `repo`, `env`). Working dir is `CCTUI_DISPATCH_WORKDIR`
    /// (default `/workspace`). Dispatched workers run headless, so the
    /// permission posture is `Yolo` (bypass — the pod is already sandboxed by
    /// landlock + seccomp + the guard-proxy).
    fn build_dispatch_spec(
        payload: &serde_json::Value,
    ) -> anyhow::Result<cctui_proto::adapter::SessionSpec> {
        let prompt = Self::resolve_dispatch_prompt(payload)?;
        let workdir =
            std::env::var("CCTUI_DISPATCH_WORKDIR").unwrap_or_else(|_| "/workspace".to_owned());
        let env: std::collections::BTreeMap<String, String> = payload
            .get("env")
            .and_then(serde_json::Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        let name = payload
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| std::env::var("TASK_NAME").ok().filter(|s| !s.is_empty()));
        Ok(cctui_proto::adapter::SessionSpec {
            adapter_id: cctui_proto::adapter::AdapterId("claude-code".to_owned()),
            working_dir: Some(workdir),
            prompt: Some(prompt),
            name,
            permission_mode: Some(cctui_proto::adapter::PermissionMode::Yolo),
            effort: payload
                .get("effort")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            model: payload.get("model").and_then(serde_json::Value::as_str).map(ToOwned::to_owned),
            env,
            bootstrap: serde_json::Value::Null,
        })
    }

    /// Resolve the dispatched prompt: an inline `prompt`, else a `prompt_file`
    /// searched across `CCTUI_DISPATCH_PROMPT_DIRS` (default
    /// `/opt/context/prompts:/prompts`). An absolute `prompt_file` is read as-is.
    fn resolve_dispatch_prompt(payload: &serde_json::Value) -> anyhow::Result<String> {
        if let Some(p) = payload.get("prompt").and_then(serde_json::Value::as_str)
            && !p.is_empty()
        {
            return Ok(p.to_owned());
        }
        let file = payload
            .get("prompt_file")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("dispatch payload has neither prompt nor prompt_file")
            })?;
        if file.starts_with('/') {
            return std::fs::read_to_string(file)
                .with_context(|| format!("reading prompt file {file}"));
        }
        let dirs = std::env::var("CCTUI_DISPATCH_PROMPT_DIRS")
            .unwrap_or_else(|_| "/opt/context/prompts:/prompts".to_owned());
        for dir in dirs.split(':').filter(|d| !d.is_empty()) {
            let path = std::path::Path::new(dir).join(file);
            if path.is_file() {
                return std::fs::read_to_string(&path)
                    .with_context(|| format!("reading prompt file {}", path.display()));
            }
        }
        anyhow::bail!("prompt_file {file} not found under {dirs}")
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
        forced_session_id: Option<String>,
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
        // Use the server-pre-minted session id when supplied (CCT-446) so the
        // id the server bound the gateway session token to matches the id the
        // worker registers as (otherwise `account_name` never resolves). Falls
        // back to a fresh uuid for non-account / non-HTTP spawns.
        let session_id = forced_session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
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
            args.push("--permission-mode".to_owned());
            args.push(mode.claude_flag().to_owned());
        }
        // Inject the managed `AskUserQuestion` hook settings (CCT-167), scoped
        // to this fleet-spawned worker only — the user's hand-run `claude` is
        // untouched. `--settings` merges over the resolved hierarchy, so it
        // only ADDS the hook. Goes into `respawnFlags` too so it survives the
        // `/clear`/`/compact` relaunch the claude daemon drives off them.
        let mut respawn_flags = vec!["--agent".to_owned(), agent.to_owned()];
        // NB: model/effort are NOT passed as `--model`/`--effort` CLI args
        // (CCT-577). They ride the managed `--settings` file below (`model` /
        // `effortLevel` / `CLAUDE_CODE_EFFORT_LEVEL`), which the claude daemon
        // applies to a spare-claimed worker — whereas a `--model` CLI arg forces
        // the spare-claim/cold relaunch that drops the dispatch gateway env.
        // Remember the spawn-time model/effort keyed by `short` (CCT-299) so the
        // Status emit can fall back to it while `state.json` is still being
        // written (or transiently gone across a `/clear`).
        {
            let model = spec.model.as_deref().map(str::trim).filter(|m| !m.is_empty());
            let effort = spec.effort.as_deref().map(str::trim).filter(|e| !e.is_empty());
            if (model.is_some() || effort.is_some())
                && let Ok(mut map) = self.spawn_model_effort.lock()
            {
                map.insert(short.to_owned(), (model.map(str::to_owned), effort.map(str::to_owned)));
            }
        }
        let whip = spec.permission_mode.is_some_and(cctui_proto::adapter::PermissionMode::is_whip);
        // Resolve the gateway env + per-account settings from the server's
        // durable binding BEFORE writing the managed hook-settings file, so the
        // account settings (CCT-539/540) can be deep-merged under the managed
        // hooks. Fail-closed inside `resolve_launch_env` (account-bound but
        // unmintable → abort rather than launch a worker that will 401, CCT-460).
        let launch = self.resolve_launch_env(&session_id, &spec.env).await?;
        if let Some(settings) = ensure_hook_settings(
            &self.cfg.hook_socket_path,
            whip,
            short,
            launch.settings.as_ref(),
            &launch.env,
            spec.model.as_deref(),
            spec.effort.as_deref(),
        ) {
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
        // Prepend a delimited `<session-context>` block to the SPAWN prompt only
        // (CCT-361): give the agent the same at-a-glance context a human sees in
        // the UI — name, model·effort, permission posture, env var NAMES (never
        // values — those live only in `env_json` below), cwd, and the staged
        // file list (folded in here from the old client-side `Attached files:`
        // append). Subsequent messages are untouched.
        let session_context = build_session_context(spec, cwd, &staged);
        let launch_prompt = match spec.prompt.as_deref().map(str::trim) {
            Some(b) if !b.is_empty() => Some(format!("{session_context}\n\n{b}")),
            _ => Some(session_context),
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
        //
        // Gateway env resolved above (CCT-460): a spawn whose server-side mint
        // silently produced nothing already failed closed there rather than
        // launching a worker that will 401.
        let env = launch.env;
        let env_json: serde_json::Map<String, serde_json::Value> =
            env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();

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
        // CCT-462: record that cctui launched this worker WITH gateway env so
        // the proactive heal never targets a worker we just resolved env for.
        // `short` is the first 8 hex of the session id; the poll loop keys the
        // tracker on the same `short`.
        self.note_launched_with_env(short, &env);
        Ok(())
    }

    /// Record (CCT-462) that cctui ITSELF dispatched worker `short` through the
    /// launch chokepoint, so the proactive heal never force-kills it. The trust
    /// signal is "cctui launched this worker", NOT "with non-empty env": a
    /// session with no account binding legitimately resolves to an EMPTY
    /// gateway env — it routes via the machine's own ambient credentials — yet
    /// is still a worker cctui launched and must never be healed. Recording
    /// only non-empty launches misclassified those as autonomous respawns and
    /// force-killed healthy live sessions (the v0.7.47 regression). The heal's
    /// real target — a LEGACY session present in the roster at daemon startup
    /// that cctui never launched this lifetime — is untouched (it stays
    /// un-recorded → a candidate).
    ///
    /// Trust is additionally VERIFIED after the fact (CCT-574): for launches
    /// whose env carried a token, the poll loop checks the live process really
    /// received it (`verify_env_delivery`) — dispatch success alone proved
    /// nothing when the claude daemon's spare claim dropped the env.
    ///
    /// When the env carries a gateway session token
    /// (`ANTHROPIC_AUTH_TOKEN` / `OPENAI_API_KEY`) its sha256 hex is recorded
    /// too, arming the low-frequency token-validity sweep for this worker
    /// (CCT-462 finish) — a trusted worker whose token stops resolving
    /// server-side (unbound/deleted `session_tokens` row) would otherwise 401
    /// forever without ever becoming a heal candidate. Hash only, in memory
    /// only — no token material persisted (CCT-503). Best-effort under a
    /// poisoned lock.
    fn note_launched_with_env(
        &self,
        short: &str,
        env: &std::collections::BTreeMap<String, String>,
    ) {
        let token_hash = env
            .get("ANTHROPIC_AUTH_TOKEN")
            .or_else(|| env.get("OPENAI_API_KEY"))
            .map(|t| crate::gateway_heal::sha256_hex(t));
        if let Ok(mut t) = self.gateway_heal.lock() {
            t.note_launched_with_env(short, token_hash);
        }
    }

    /// Fork an existing conversation into a brand-new claude session (CCT-302).
    ///
    /// Mirrors [`spawn`] — mints a fresh `short`/`sessionId`/`nonce` and
    /// dispatches a new worker via the control socket — but prepends `--resume
    /// <parent-session-id> --fork-session` to the launch argv so claude copies
    /// the parent's history into the new session id, leaving the parent intact.
    /// `--model`/`--effort` from `spec` ride on top (this is the supported
    /// "switch model mid-conversation" path, CCT-303).
    ///
    /// The parent session id is resolved to the id claude should resume from:
    /// the parent's on-disk `resumeSessionId` when present (so a `/clear`ed or
    /// `/compact`ed parent forks from the live conversation, not the stale spawn
    /// id — CCT-160), else the parent's `sessionId`, else the `parent_local_id`
    /// itself (covers reopening an archived parent whose `state.json` was removed
    /// by `claude rm` but whose transcript still resumes).
    ///
    /// The child's `SessionStarted` is emitted later by the roster-discovery
    /// path, which has no fork context, so we stash `parent_local_id` keyed by
    /// the new `short` in `fork_parent_by_short` for it to read.
    #[allow(clippy::too_many_lines)]
    async fn fork(
        &self,
        sock: &std::path::Path,
        parent_local_id: &str,
        spec: &cctui_proto::adapter::SessionSpec,
        forced_session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let cwd = spec
            .working_dir
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("fork: working_dir required"))?;
        let cwd_path = std::path::Path::new(cwd);
        if !cwd_path.is_dir() {
            anyhow::bail!("fork: working_dir does not exist or is not a directory: {cwd}");
        }

        // Resolve the id to resume+fork from. Prefer the parent's on-disk
        // `resumeSessionId` (the live conversation head after `/clear`/`/compact`
        // — CCT-160), then its `sessionId`, then the raw parent id (archived
        // parent whose job state was removed by `claude rm`, but whose transcript
        // still resumes — the native "reopen archived as a new conversation").
        let resume_id = self
            .resolve_short_for_removal(parent_local_id)
            .ok()
            .and_then(|short| StateJson::read(&self.cfg.jobs_root, &short))
            .and_then(|st| st.resume_session_id.or(st.session_id))
            .unwrap_or_else(|| parent_local_id.to_owned());

        let agent = "claude";
        // Use the server-pre-minted child id when supplied (CCT-345) so the id
        // the webui navigated to matches the worker the daemon launches.
        let session_id =
            forced_session_id.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
        let short = session_id[..8].to_owned();
        let nonce: String = uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect();
        let created_at = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        )
        .unwrap_or(0);

        // Launch argv: resume the parent + fork into the fresh session id. The
        // `--session-id` we minted is the new (child) id; `--resume`/`--fork-session`
        // seed it from the parent's history.
        let mut args = vec![
            "--resume".to_owned(),
            resume_id.clone(),
            "--fork-session".to_owned(),
            "--session-id".to_owned(),
            session_id.clone(),
            "--agent".to_owned(),
            agent.to_owned(),
        ];
        if let Some(name) = &spec.name {
            args.push("--name".to_owned());
            args.push(name.clone());
        }
        if let Some(mode) = spec.permission_mode {
            args.push("--permission-mode".to_owned());
            args.push(mode.claude_flag().to_owned());
        }
        let mut respawn_flags = vec!["--agent".to_owned(), agent.to_owned()];
        if let Some(effort) = spec.effort.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
            args.push("--effort".to_owned());
            args.push(effort.to_owned());
            respawn_flags.push("--effort".to_owned());
            respawn_flags.push(effort.to_owned());
        }
        if let Some(model) = spec.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            args.push("--model".to_owned());
            args.push(model.to_owned());
            respawn_flags.push("--model".to_owned());
            respawn_flags.push(model.to_owned());
        }
        {
            let model = spec.model.as_deref().map(str::trim).filter(|m| !m.is_empty());
            let effort = spec.effort.as_deref().map(str::trim).filter(|e| !e.is_empty());
            if (model.is_some() || effort.is_some())
                && let Ok(mut map) = self.spawn_model_effort.lock()
            {
                map.insert(short.clone(), (model.map(str::to_owned), effort.map(str::to_owned)));
            }
        }
        let whip = spec.permission_mode.is_some_and(cctui_proto::adapter::PermissionMode::is_whip);
        // Gateway env + per-account settings for the fork child (CCT-460): the
        // fork dispatch used to hardcode empty env, so a fork of an account-bound
        // conversation 401ed. Resolve for the child id first; if the server
        // hasn't bound it yet, inherit the parent's account env (and settings) so
        // the child routes through the gateway from its first turn. Empty when
        // neither is account-bound. Resolved BEFORE the hook-settings file is
        // written so the account settings can be merged under the managed hooks
        // (CCT-539/540).
        let mut launch =
            self.resolve_launch_env(&session_id, &std::collections::BTreeMap::default()).await?;
        if launch.env.is_empty() {
            launch = self
                .resolve_launch_env(parent_local_id, &std::collections::BTreeMap::default())
                .await?;
        }
        if let Some(settings) = ensure_hook_settings(
            &self.cfg.hook_socket_path,
            whip,
            &short,
            launch.settings.as_ref(),
            &launch.env,
            None,
            None,
        ) {
            let settings = settings.to_string_lossy().into_owned();
            args.push("--settings".to_owned());
            args.push(settings.clone());
            respawn_flags.push("--settings".to_owned());
            respawn_flags.push(settings);
        }
        // Optional first turn on the forked branch.
        if let Some(prompt) = spec.prompt.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            args.push("--".to_owned());
            args.push(prompt.to_owned());
        }
        let intent = spec.prompt.clone().or_else(|| spec.name.clone()).unwrap_or_default();
        let mut seed = serde_json::Map::new();
        seed.insert("intent".to_owned(), json!(intent));
        if let Some(name) = &spec.name {
            seed.insert("name".to_owned(), json!(name));
            seed.insert("nameSource".to_owned(), json!("user"));
        }

        // Remember the parent BEFORE dispatching so the roster-discovery emit
        // (which can race in on the very next poll) finds the link.
        if let Ok(mut map) = self.fork_parent_by_short.lock() {
            map.insert(short.clone(), parent_local_id.to_owned());
        }

        // Gateway env resolved above (CCT-460).
        let env = launch.env;
        let env_json: serde_json::Map<String, serde_json::Value> =
            env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();

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
        let resp: serde_json::Value = socket::call(sock, &req)
            .await
            .with_context(|| format!("dispatch fork of {parent_local_id} in {cwd}"))?;
        tracing::info!(?resp, %cwd, %session_id, %parent_local_id, %resume_id, "fork dispatched via control socket");
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
            // Roster churn (CCT-253): the socket vanished and sessions were
            // flushed. When it comes back the workers are re-pinned and the
            // tail resumes from the persisted offset — but a re-home can leave
            // a gap (briefly tailing a file no longer appended, or a send
            // dropped during the churn). Arm an immediate reconcile on the
            // next successful poll so the gap self-heals without waiting for
            // the periodic cycle.
            self.churned = true;
            return Ok(());
        };

        let resp: ListResponse = socket::call(&sock, &json!({"proto": 1, "op": "list"})).await?;
        self.apply_snapshot(resp.jobs).await;
        if self.churned {
            self.churned = false;
            self.reconcile_tail().await;
        }
        Ok(())
    }

    #[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
    async fn apply_snapshot(&mut self, jobs: Vec<LiveSnapshot>) {
        let visible: Vec<LiveSnapshot> =
            jobs.into_iter().filter(LiveSnapshot::is_user_visible).collect();
        let now_shorts: HashSet<String> = visible.iter().map(|j| j.short.clone()).collect();

        // Ground-truth effort for every live worker in one `/proc` pass (CCT-577),
        // reused across the per-job Status build below so a busy roster doesn't
        // rescan `/proc` per session.
        let observed_efforts = super::envcheck::worker_efforts(&now_shorts);

        // CCT-509: grandfather the FIRST successful snapshot of this daemon
        // lifetime. Its workers were already alive when cctui (re)started — they
        // were brought up by a PRIOR lifetime (a self-update re-exec or
        // sleep-wake survivor cctui launched correctly), not by an autonomous
        // respawn we observed flip absent→present. The in-memory `HealTracker`
        // is empty at startup, so without this they'd all be untrusted →
        // mistaken for env-less respawns → force-killed (the restart-amnesia
        // kill storm). Trust them; a genuine respawn AFTER startup still arms
        // the heal because it drops off the roster (`forget`) then reappears
        // untracked. A worker cctui itself launches is recorded with env at the
        // chokepoint regardless, so this only matters for inherited survivors.
        if !self.grandfathered {
            self.grandfathered = true;
            if let Ok(mut t) = self.gateway_heal.lock() {
                for short in &now_shorts {
                    // No hash: cctui didn't launch these this lifetime, so it
                    // doesn't know what token (if any) they carry — trusted but
                    // never validity-probed (CCT-462).
                    t.note_launched_with_env(short, None);
                }
            }
        }

        // Newly started.
        for job in &visible {
            if !self.roster.contains(&job.short) {
                let session_id = job.session_id().map_or_else(|| job.short.clone(), str::to_owned);
                self.short_by_session.insert(session_id.clone(), job.short.clone());
                // If this short was just forked (CCT-302), carry the parent link
                // so the server resolves it into `parent_id`. Consumed once.
                let parent_local_id =
                    self.fork_parent_by_short.lock().ok().and_then(|mut m| m.remove(&job.short));
                self.emit(AdapterEvent::SessionStarted {
                    local_id: session_id,
                    meta: SessionMeta {
                        working_dir: job.cwd.clone(),
                        parent_local_id: parent_local_id.clone(),
                        extra: json!({
                            "short": job.short,
                            "cli_version": job.cli_version,
                            "relation": if parent_local_id.is_some() { "fork" } else { "root" },
                        }),
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

            // Dead-but-still-listed (CCT-252). claude can keep a session in
            // `daemon list` while its worker process is gone (e.g. it died
            // while the supervisor was down — "process gone while supervisor
            // down"). Previously cctui only transitioned a session to
            // hibernated/ended when its `short` DROPPED OFF the roster
            // (`gone` handling below), and liveness was otherwise purely
            // time-derived (server `derive_liveness`), so such a session kept
            // showing its last status with a green/stale dot for minutes. Here
            // we surface the dead state within one poll instead.
            if job.is_dead() {
                // Emit the terminal transition exactly once, then mark the
                // short sticky so the still-present roster entry can't re-emit
                // a non-terminal Status and re-green it (daemon-side mirror of
                // the server's CCT-192 sticky terminal status). Mirrors the
                // roster-disappearance path: hibernated if job state survives
                // on disk (revivable red dot, CCT-228), else SessionEnded.
                if self.dead_shorts.insert(job.short.clone()) {
                    self.clear_permission(&job.short).await;
                    if on_disk.is_some() {
                        self.emit(AdapterEvent::Status {
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
                        .await;
                    } else {
                        self.emit(AdapterEvent::SessionEnded {
                            local_id: local_id.clone(),
                            reason: EndReason::Completed,
                        })
                        .await;
                        // Truly gone — drop the remembered spawn flags (CCT-299).
                        if let Ok(mut m) = self.spawn_model_effort.lock() {
                            m.remove(&job.short);
                        }
                    }
                    // Drop the cached status so a later revive (worker reports
                    // alive again) is detected as a change and re-emitted.
                    self.last_status.remove(&job.short);
                }
                // Sticky: skip transcript re-pin + Status for this poll. The
                // roster-disappearance branch still cleans up if it later
                // drops off; a revive clears `dead_shorts` (below) so live
                // status resumes.
                continue;
            }
            // Revived: claude reports this short alive again after we marked it
            // dead — clear the sticky flag so live status flows again.
            self.dead_shorts.remove(&job.short);

            // Proactive gateway-env heal (CCT-462): a LIVE, account-bound worker
            // that cctui did NOT launch with env was brought up by an autonomous
            // `claude daemon` respawn (bypassing the CCT-460 chokepoint) and may
            // be env-less → 401. Force a re-resume through the chokepoint. The
            // candidate set is normally empty (every cctui launch records env),
            // so the server round-trip only fires for the rare autonomous-respawn
            // case. Bounded/idempotent via `HealTracker`; see `gateway_heal`.
            self.maybe_heal_gateway_env(&job.short, &local_id).await;

            // Verify env DELIVERY for workers cctui just launched with a
            // gateway token (CCT-574): dispatch `ok: true` is not proof the
            // worker process actually carries the env — the claude daemon's
            // spare-claim path has been observed exec'ing workers without it,
            // silently falling back to the machine's ambient login. A confirmed
            // miss revokes trust so the heal above retries delivery, bounded by
            // the delivery-failure budget (no more infinite heal↔resume loops).
            self.verify_env_delivery(&job.short, &local_id);

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
            // Prefer the on-disk state.json; fall back to the spawn-time flags
            // we remembered while state.json is absent/transient (CCT-299).
            let spawned =
                self.spawn_model_effort.lock().ok().and_then(|m| m.get(&job.short).cloned());
            let model = on_disk
                .as_ref()
                .and_then(|s| s.model.clone())
                .or_else(|| spawned.as_ref().and_then(|(m, _)| m.clone()));
            // Prefer the GROUND-TRUTH effort the live worker actually booted at
            // (read from its `CLAUDE_EFFORT` env), so the UI shows what the
            // session is running rather than what we requested — a spare-claim or
            // a silent background clamp can make them differ (CCT-577). Fall back
            // to the requested value (state.json flags, then the spawn cache)
            // while the worker is mid-exec / not yet found in `/proc`.
            let effort = observed_efforts
                .get(&job.short)
                .cloned()
                .or_else(|| on_disk.as_ref().and_then(|s| s.effort.clone()))
                .or_else(|| spawned.as_ref().and_then(|(_, e)| e.clone()));
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

        // Low-frequency token-validity sweep for TRUSTED workers (CCT-462
        // finish): a worker cctui launched with env never enters the env-less
        // heal above, but its `session_tokens` row can still be unbound/deleted
        // server-side — it then 401s at the gateway forever with nothing
        // observing it. Every `TOKEN_VALIDITY_SWEEP_POLLS` polls (~1/min at the
        // default cadence), ask the server whether each trusted worker's
        // recorded token hash still resolves; a twice-confirmed `valid: false`
        // heals through the same bounded kill + cold-resume machinery.
        self.polls_since_validity_sweep = self.polls_since_validity_sweep.saturating_add(1);
        if self.polls_since_validity_sweep >= TOKEN_VALIDITY_SWEEP_POLLS {
            self.polls_since_validity_sweep = 0;
            let live: Vec<(String, String)> = visible
                .iter()
                .filter(|j| !self.dead_shorts.contains(&j.short))
                .map(|j| {
                    // Same stable-local_id derivation as the Status loop above.
                    let local_id = self
                        .transcript_locations
                        .get(&j.short)
                        .map(|loc| loc.local_id.clone())
                        .or_else(|| j.session_id().map(str::to_owned))
                        .unwrap_or_else(|| j.short.clone());
                    (j.short.clone(), local_id)
                })
                .collect();
            self.sweep_token_validity(&live).await;
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
            // Drop heal bookkeeping for a worker that left the live roster
            // (CCT-462) so it doesn't leak for the daemon lifetime; the next
            // launch re-records launched-with-env from scratch.
            if let Ok(mut t) = self.gateway_heal.lock() {
                t.forget(short);
            }
            let was_dead = self.dead_shorts.remove(short);
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
                //
                // Skip if we already emitted this short's dead transition while
                // it was still listed (CCT-252 `dead_shorts`) — the hibernated
                // Status already went out; re-emitting it here is redundant.
                if !was_dead && StateJson::read(&self.cfg.jobs_root, short).is_some() {
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
            for entry in transcript::discover_subagents(&dir) {
                let transcript::SubagentEntry { agent_id, path, workflow } = entry;
                if self.ended_subagents.contains(&agent_id) {
                    continue;
                }
                if !self.subagents.contains_key(&agent_id) {
                    self.subagents.insert(
                        agent_id.clone(),
                        SubagentState { parent_local_id: parent_id.clone(), idle_ticks: 0 },
                    );
                    // Base subagent meta; Workflow-tool agents (CCT-225) add
                    // workflow run context so the UI can group them under a
                    // named "Workflow: <name> (<runId>)" node.
                    let mut extra = json!({ "subagent": true, "agent_id": agent_id });
                    if let Some(wf) = &workflow {
                        let obj = extra.as_object_mut().expect("json object literal");
                        obj.insert("workflow_run_id".into(), json!(wf.run_id));
                        if let Some(name) = &wf.name {
                            obj.insert("workflow_name".into(), json!(name));
                        }
                        obj.insert(
                            "agent_type".into(),
                            json!(wf.agent_type.as_deref().unwrap_or("workflow-subagent")),
                        );
                    }
                    self.emit(AdapterEvent::SessionStarted {
                        local_id: agent_id.clone(),
                        meta: SessionMeta {
                            working_dir: Some(cwd.clone()),
                            parent_local_id: Some(parent_id.clone()),
                            extra,
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

    /// Periodic + churn-triggered reconciliation re-tail (CCT-253, A1).
    ///
    /// Nothing re-reads a transcript once its persisted offset advances, so a
    /// gap is otherwise permanent: an event emitted but never persisted
    /// server-side (a dropped WS send while the offset still flushed), or a
    /// roster-churn re-home that briefly tailed a file no longer being
    /// appended. Here we re-read each live session's transcript from a
    /// checkpoint a fixed window BEHIND its persisted offset and re-emit the
    /// events. The server inserts `stream_events` with
    /// `ON CONFLICT (session_id,event_type,content_hash) DO NOTHING` and
    /// broadcasts only newly-inserted rows, so re-emitting already-seen lines
    /// is idempotent and cheap — only real gaps surface, and they self-heal.
    ///
    /// Crucially this NEVER touches the persisted offset: it is a pure
    /// catch-up replay layered on top of the forward-only tail in
    /// `apply_snapshot`. The checkpoint is realigned to a JSONL line boundary
    /// inside `transcript::reconcile_tail`, so backing up mid-line can't
    /// corrupt parsing.
    async fn reconcile_tail(&mut self) {
        self.last_reconcile = Instant::now();
        let locations: Vec<TranscriptLocation> =
            self.transcript_locations.values().cloned().collect();
        for loc in locations {
            let off = self.offsets.get(&loc.offset_key);
            match transcript::reconcile_tail(&loc.path, &loc.local_id, off) {
                Ok(events) => {
                    if !events.is_empty() {
                        tracing::debug!(
                            count = events.len(),
                            path = %loc.path.display(),
                            "reconcile re-tail re-emitting (server dedups)"
                        );
                    }
                    for evt in events {
                        self.emit(evt).await;
                    }
                }
                Err(err) => {
                    tracing::debug!(%err, path = %loc.path.display(), "reconcile re-tail failed");
                }
            }
        }
    }

    async fn flush_roster(&mut self, reason: EndReason) {
        // The daemon/socket is gone — stop dialing it from every attach task.
        self.attach.cancel_all();
        let shorts: Vec<String> = self.roster.drain().collect();
        self.last_status.clear();
        // CCT-509: do NOT clear heal bookkeeping here. A flush fires when the
        // control socket is momentarily unreachable (on-demand daemon
        // idle-shutdown / kickstart race) — that is NOT evidence the workers
        // died; they stay alive and reappear on the next successful poll.
        // Forgetting their launched-with-env trust here made cctui mistake its
        // own live, account-bound sessions for env-less autonomous respawns and
        // force-kill them as soon as the socket returned (the observed kill
        // loop). Trust is dropped only when a worker genuinely leaves a
        // *successful* roster snapshot (`apply_snapshot`'s `gone` handling),
        // which a real death/respawn does and a socket blip does not.
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
/// Build the spawn-time `<session-context>` block (CCT-361) prepended to the
/// initial prompt. Mirrors what a human sees on the session card — name,
/// model·effort, permission posture, env var NAMES, cwd, and staged files.
/// Env var VALUES are never included (only `spec.env` keys, sorted by the
/// `BTreeMap`). Empty fields are omitted so the block stays tight.
fn build_session_context(
    spec: &cctui_proto::adapter::SessionSpec,
    cwd: &str,
    staged: &[String],
) -> String {
    let mut b = String::from("<session-context>\n");
    if let Some(name) = spec.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        let _ = writeln!(b, "session: {name}");
    }
    if let Some(model) = spec.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        match spec.effort.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
            Some(effort) => {
                let _ = writeln!(b, "model: {model} · effort: {effort}");
            }
            None => {
                let _ = writeln!(b, "model: {model}");
            }
        }
    }
    if let Some(mode) = spec.permission_mode {
        let _ = writeln!(b, "permission-mode: {}", mode.normalized_label());
    }
    let _ = writeln!(b, "cwd: {cwd}");
    if !spec.env.is_empty() {
        let names = spec.env.keys().cloned().collect::<Vec<_>>().join(", ");
        let _ = writeln!(b, "env (names only): {names}");
    }
    if !staged.is_empty() {
        b.push_str("attached files:\n");
        for p in staged {
            let _ = writeln!(b, "  - {p}");
        }
    }
    b.push_str("</session-context>");
    b
}

fn stage_uploads(session_id: &str, bootstrap: &serde_json::Value) -> anyhow::Result<Vec<String>> {
    if bootstrap.is_null() {
        return Ok(Vec::new());
    }
    let parsed: cctui_proto::adapter::BootstrapUploads =
        serde_json::from_value(bootstrap.clone()).context("decoding bootstrap uploads")?;
    stage_upload_files(session_id, &parsed.uploads)
}

/// Decode + write a batch of uploaded files into the per-session staging dir
/// (`/tmp/cctui-uploads/<session_id>/`), returning the staged absolute paths.
///
/// Shared by spawn-time bootstrap uploads ([`stage_uploads`]) and mid-chat
/// attachments (CCT-236). Files are written 0600 (Unix). Name collisions —
/// against an existing staged file from an earlier upload in the same session —
/// are resolved by inserting a numeric suffix before the extension
/// (`report.pdf` → `report-1.pdf`) rather than overwriting, so a later
/// attachment never clobbers one the agent may still reference.
fn stage_upload_files(
    session_id: &str,
    uploads: &[cctui_proto::adapter::BootstrapFile],
) -> anyhow::Result<Vec<String>> {
    use base64::Engine;

    if uploads.is_empty() {
        return Ok(Vec::new());
    }
    let dir = std::path::Path::new("/tmp/cctui-uploads").join(session_id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating upload dir {}", dir.display()))?;
    let mut paths = Vec::with_capacity(uploads.len());
    for file in uploads {
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
        let path = unique_staging_path(&dir, name);
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
    tracing::info!(%session_id, count = paths.len(), "staged uploaded files");
    Ok(paths)
}

/// Resolve a non-colliding path in `dir` for `name`. If `dir/name` is free use
/// it; otherwise append `-1`, `-2`, … before the extension until a free path is
/// found.
fn unique_staging_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = std::path::Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 1u32.. {
        let alt = ext.map_or_else(|| format!("{stem}-{n}"), |ext| format!("{stem}-{n}.{ext}"));
        let candidate = dir.join(alt);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("exhausted u32 collision suffixes")
}

/// Public entry point for mid-chat attachment staging (CCT-236). Thin wrapper
/// over [`stage_upload_files`] so the supervisor can stage without reaching into
/// control internals.
pub fn stage_mid_chat_files(
    session_id: &str,
    uploads: &[cctui_proto::adapter::BootstrapFile],
) -> anyhow::Result<Vec<String>> {
    stage_upload_files(session_id, uploads)
}

/// Recover a session's whip posture from the per-session settings file the
/// original spawn wrote for `short` (CCT-540). The whip profile is the only one
/// that emits a top-level `hooks.Stop` block (the `whip-stop-hook`), so its
/// presence is a reliable discriminator. Used by cold resume, which has no
/// `spec` to read `permission_mode` from. Absent/unreadable file → not whip.
fn detect_whip_from_settings(short: &str) -> bool {
    let Some(path) = hook_settings_path(&format!("hook-settings-{short}.json")) else {
        return false;
    };
    std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("hooks").and_then(|h| h.get("Stop")).cloned())
        .is_some()
}

fn hook_settings_path(file: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("cctui").join(file))
}

/// Recursively deep-merge `overlay` into `base`, with `overlay` winning at every
/// level (CCT-540). Object nodes are merged key-by-key (recursing on shared
/// keys); every other node kind (scalars, arrays) is replaced wholesale by the
/// overlay value. Keys present only in `base` are preserved.
///
/// The daemon uses this to layer its load-bearing managed settings (the ask /
/// permission / Stop hooks) as the `overlay` OVER server-provided per-account
/// settings (the `base`) — so account settings can add keys but can never
/// clobber a managed key. See [`ensure_hook_settings`].
pub(super) fn deep_merge(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(b), serde_json::Value::Object(o)) => {
            for (k, ov) in o {
                match b.get_mut(k) {
                    Some(bv) => deep_merge(bv, ov),
                    None => {
                        b.insert(k.clone(), ov.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// Produce the final `--settings` document by deep-merging the server-provided
/// per-account `settings` UNDERNEATH the daemon's `managed` settings (CCT-540).
///
/// Managed values win at every level: we start from a clone of the account
/// settings (an object; anything non-object is discarded as malformed) and
/// overlay the managed settings on top via [`deep_merge`]. An account blob that
/// tries to set its own `hooks` therefore loses to the managed `hooks` block —
/// the ask/permission/Stop hooks survive intact.
pub(super) fn merge_account_under_managed(
    managed: serde_json::Value,
    account: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut merged = match account {
        Some(a @ serde_json::Value::Object(_)) => a.clone(),
        // No account settings, or a non-object blob we can't safely merge under:
        // fall back to managed-only, exactly as before CCT-540.
        _ => return managed,
    };
    deep_merge(&mut merged, &managed);
    merged
}

/// Write (idempotently, on every spawn so it tracks binary upgrades) the
/// managed Claude Code settings file that registers the `AskUserQuestion`
/// PreToolUse/PostToolUse hooks, pointing at this daemon binary and the given
/// delivery socket (CCT-167). Returns the file path to inject via `--settings`,
/// or `None` if we can't locate the binary / config dir (in which case spawning
/// proceeds without the hook rather than failing).
///
/// `whip` (CCT-352) toggles the 🐎 enforcement profile: the `AskUserQuestion`
/// `PreToolUse` hook gains `--deny` (it still notifies the UI, but returns a
/// `deny` decision so the form never renders), and a `Stop` hook
/// (`whip-stop-hook`) blocks stalling / hand-back language so the worker runs to
/// genuine completion.
///
/// The file is written to a PER-SESSION path (keyed by `short`) so different
/// sessions — potentially bound to different accounts with different
/// `account_settings` — never clobber each other's `--settings` file.
///
/// `account_settings` (CCT-540) is the server-provided, per-account
/// `settings_json` that rode the gateway-env pull. It is deep-merged UNDERNEATH
/// the managed settings: account keys are layered in, but the managed `hooks`
/// block (and any other key the daemon sets) ALWAYS WINS — a malicious or
/// stale account blob that specifies its own `hooks` can never disable the
/// ask/permission/Stop hooks. `None` → managed settings only, exactly as before.
pub(super) fn ensure_hook_settings(
    sock: &std::path::Path,
    whip: bool,
    short: &str,
    account_settings: Option<&serde_json::Value>,
    gateway_env: &std::collections::BTreeMap<String, String>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Option<PathBuf> {
    let path = hook_settings_path(&format!("hook-settings-{short}.json"))?;
    let exe = std::env::current_exe()
        .map_err(|err| tracing::warn!(%err, "ask-hook: cannot resolve current_exe"))
        .ok()?;
    let exe = exe.to_string_lossy();
    let sock = sock.to_string_lossy();
    let deny = if whip { " --deny" } else { "" };
    let hook = |event: &str| {
        let extra = if event == "pre" { deny } else { "" };
        json!({
            // AskUserQuestion + ExitPlanMode both fire this hook: the former
            // surfaces a live question card, the latter a live Plan card
            // (CCT-347). Both are single-select PTY prompts answered the same
            // way (digit keystroke / dismiss-then-reply).
            "matcher": "AskUserQuestion|ExitPlanMode",
            "hooks": [{
                "type": "command",
                "command": format!("{exe} ask-hook --event {event} --sock {sock}{extra}"),
                "timeout": 5,
            }],
        })
    };
    // Bidirectional tool-permission hook (CCT-342). Scoped to the mutating /
    // executing tools that actually trigger an interactive approval (read-only
    // tools auto-allow, so blocking on them would needlessly stall the turn).
    // Distinct from the `AskUserQuestion` matcher above so both PreToolUse hooks
    // can coexist. The hook BLOCKS, long-polls the daemon for the human's
    // decision, and returns an allow/deny `permissionDecision` — no keystroke in
    // the common case. The `timeout` is deliberately high (the daemon resolves
    // the hook with a `defer` well before this fires, on its own bounded wait);
    // a hook that overran *its* timeout would be treated by Claude Code as a
    // hard deny, which we must never do to a slow human.
    let perm_matcher = "Bash|Edit|MultiEdit|Write|NotebookEdit|WebFetch|Task|KillShell|BashOutput";
    let perm_hook = json!({
        "matcher": perm_matcher,
        "hooks": [{
            "type": "command",
            "command": format!("{exe} ask-hook --event perm --sock {sock}"),
            "timeout": 600,
        }],
    });
    let pre_hooks = json!([hook("pre"), perm_hook]);
    let hooks = if whip {
        json!({
            "PreToolUse": pre_hooks,
            "PostToolUse": [hook("post")],
            "Stop": [{
                "hooks": [{
                    "type": "command",
                    "command": format!("{exe} whip-stop-hook"),
                    "timeout": 10,
                }],
            }],
        })
    } else {
        json!({ "PreToolUse": pre_hooks, "PostToolUse": [hook("post")] })
    };
    let managed = managed_settings(hooks, gateway_env, model, effort);
    // Layer the server-provided per-account settings UNDERNEATH the managed
    // settings (CCT-540): account keys are merged in, but the managed keys
    // (hooks, gateway env, model/effort) always win so they can never be
    // clobbered.
    let settings = merge_account_under_managed(managed, account_settings);
    if let Some(Err(err)) = path.parent().map(std::fs::create_dir_all) {
        tracing::warn!(%err, "ask-hook: cannot create settings dir");
        return None;
    }
    if let Err(err) = std::fs::write(&path, serde_json::to_vec_pretty(&settings).ok()?) {
        tracing::warn!(%err, path = %path.display(), "ask-hook: cannot write settings");
        return None;
    }
    // The file now carries the gateway bearer token (CCT-577) — restrict it to
    // owner-only so the secret isn't world-readable on disk.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!(%err, path = %path.display(), "ask-hook: cannot chmod settings 0600");
        }
    }
    Some(path)
}

/// Build the managed `--settings` document (CCT-577): the ask/permission/Stop
/// `hooks`, the gateway routing `env`, and the session `model`/`effortLevel`,
/// all in one file. The claude daemon applies a session's `--settings` to a
/// spare-claimed worker but deliberately does NOT reapply the dispatch `env`, so
/// carrying the gateway env HERE is the only channel that survives the
/// spare-claim on every platform (replacing the Linux-`/proc`-only gateway-env
/// heal). Split out from [`ensure_hook_settings`] so the shape is unit-testable
/// without touching the filesystem.
fn managed_settings(
    hooks: serde_json::Value,
    gateway_env: &std::collections::BTreeMap<String, String>,
    model: Option<&str>,
    effort: Option<&str>,
) -> serde_json::Value {
    let mut managed = serde_json::Map::new();
    managed.insert("hooks".to_owned(), hooks);
    if let Some(m) = model.map(str::trim).filter(|m| !m.is_empty()) {
        managed.insert("model".to_owned(), json!(m));
    }
    let mut env_obj: serde_json::Map<String, serde_json::Value> =
        gateway_env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    // `effortLevel` only accepts low|medium|high|xhigh; `max`/`ultracode` are
    // session-only and rejected in a settings file, so they ride the
    // `CLAUDE_CODE_EFFORT_LEVEL` env var instead (which accepts them).
    if let Some(e) = effort.map(str::trim).filter(|e| !e.is_empty()) {
        if matches!(e, "low" | "medium" | "high" | "xhigh") {
            managed.insert("effortLevel".to_owned(), json!(e));
        } else {
            env_obj.insert("CLAUDE_CODE_EFFORT_LEVEL".to_owned(), json!(e));
        }
    }
    if !env_obj.is_empty() {
        managed.insert("env".to_owned(), serde_json::Value::Object(env_obj));
    }
    serde_json::Value::Object(managed)
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
    use cctui_proto::api::GatewayEnvResponse;

    fn env_of(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn dispatch_spec_built_from_payload_with_inline_prompt() {
        // CCT-471: the dispatcher injects prompt/model/effort/env inside
        // TASK_PAYLOAD_JSON; the daemon turns it into a headless SessionSpec.
        let payload = serde_json::json!({
            "prompt": "do the thing",
            "model": "opus",
            "effort": "low",
            "repo": "acme",
            "name": "triage-PROJ",
            "env": { "ANTHROPIC_BASE_URL": "https://x/gateway/anthropic", "ANTHROPIC_AUTH_TOKEN": "cctui_s_x" },
        });
        let spec = Driver::build_dispatch_spec(&payload).expect("spec");
        assert_eq!(spec.prompt.as_deref(), Some("do the thing"));
        assert_eq!(spec.model.as_deref(), Some("opus"));
        assert_eq!(spec.effort.as_deref(), Some("low"));
        assert_eq!(spec.name.as_deref(), Some("triage-PROJ"));
        assert_eq!(spec.adapter_id.0, "claude-code");
        assert!(matches!(spec.permission_mode, Some(cctui_proto::adapter::PermissionMode::Yolo)));
        assert_eq!(spec.env.get("ANTHROPIC_AUTH_TOKEN").map(String::as_str), Some("cctui_s_x"));
    }

    #[test]
    fn dispatch_prompt_reads_absolute_file() {
        let dir = std::env::temp_dir().join(format!("cctui-disp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("p.md");
        std::fs::write(&f, "PROMPT BODY").unwrap();
        let payload = serde_json::json!({ "prompt_file": f.to_str().unwrap() });
        assert_eq!(Driver::resolve_dispatch_prompt(&payload).unwrap(), "PROMPT BODY");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dispatch_prompt_errors_when_neither_present() {
        let payload = serde_json::json!({ "model": "opus" });
        assert!(Driver::resolve_dispatch_prompt(&payload).is_err());
    }

    #[test]
    fn managed_settings_carries_gateway_env_model_and_effort() {
        // CCT-577: gateway env + model + effort ride the `--settings` file so
        // they survive the claude-daemon spare-claim (which drops the dispatch
        // env). Enum efforts use the `effortLevel` key; `max` falls back to the
        // CLAUDE_CODE_EFFORT_LEVEL env var (the settings key rejects it).
        let mut env = std::collections::BTreeMap::new();
        env.insert("ANTHROPIC_BASE_URL".to_owned(), "https://x/gateway/anthropic".to_owned());
        env.insert("ANTHROPIC_AUTH_TOKEN".to_owned(), "cctui_s_tok".to_owned());

        let s = managed_settings(
            json!({ "PreToolUse": [] }),
            &env,
            Some("claude-fable-5[1m]"),
            Some("medium"),
        );
        assert_eq!(s["model"], json!("claude-fable-5[1m]"));
        assert_eq!(s["effortLevel"], json!("medium"));
        assert_eq!(s["env"]["ANTHROPIC_BASE_URL"], json!("https://x/gateway/anthropic"));
        assert_eq!(s["env"]["ANTHROPIC_AUTH_TOKEN"], json!("cctui_s_tok"));
        assert!(s["hooks"].is_object());

        // `max` is not a valid `effortLevel`; it rides the env var instead.
        let s = managed_settings(json!({}), &env, None, Some("max"));
        assert!(s.get("effortLevel").is_none());
        assert_eq!(s["env"]["CLAUDE_CODE_EFFORT_LEVEL"], json!("max"));

        // No env / model / effort → only hooks, no empty `env` object.
        let s = managed_settings(json!({}), &std::collections::BTreeMap::new(), None, None);
        assert!(s.get("env").is_none());
        assert!(s.get("model").is_none());
    }

    /// The managed settings shape produced by `ensure_hook_settings` for a
    /// non-whip session (hooks the ask form + permission flow depend on). Kept
    /// in the test so the merge assertions below pin the load-bearing keys
    /// without dragging in `current_exe`/socket resolution.
    fn managed_ask_settings() -> serde_json::Value {
        json!({
            "hooks": {
                "PreToolUse": [
                    { "matcher": "AskUserQuestion|ExitPlanMode", "hooks": [{ "type": "command", "command": "cctui ask-hook --event pre" }] },
                    { "matcher": "Bash|Edit", "hooks": [{ "type": "command", "command": "cctui ask-hook --event perm" }] },
                ],
                "PostToolUse": [{ "matcher": "AskUserQuestion|ExitPlanMode", "hooks": [{ "type": "command", "command": "cctui ask-hook --event post" }] }],
            },
        })
    }

    #[test]
    fn deep_merge_overlay_wins_and_recurses() {
        // CCT-540: overlay wins at every level; base-only keys are preserved;
        // nested objects merge key-by-key rather than replacing wholesale.
        let mut base = json!({
            "a": 1,
            "keep": "me",
            "nested": { "x": "base-x", "base_only": true },
        });
        let overlay = json!({
            "a": 2,
            "nested": { "x": "overlay-x", "y": "overlay-y" },
        });
        deep_merge(&mut base, &overlay);
        assert_eq!(base["a"], json!(2), "overlay scalar wins");
        assert_eq!(base["keep"], json!("me"), "base-only top key preserved");
        assert_eq!(base["nested"]["x"], json!("overlay-x"), "overlay wins nested");
        assert_eq!(base["nested"]["base_only"], json!(true), "base-only nested key preserved");
        assert_eq!(base["nested"]["y"], json!("overlay-y"), "overlay-only nested key added");
    }

    #[test]
    fn account_settings_cannot_clobber_managed_hooks() {
        // (a) An account blob that specifies its OWN hooks must lose to the
        // managed hooks entirely — the ask/permission hooks survive intact.
        let managed = managed_ask_settings();
        let account = json!({
            "hooks": { "PreToolUse": [{ "matcher": "*", "hooks": [{ "type": "command", "command": "evil --disable" }] }] },
            "env": { "MY_ACCOUNT_VAR": "1" },
            "permissions": { "allow": ["Bash(ls:*)"] },
        });
        let merged = merge_account_under_managed(managed.clone(), Some(&account));
        // Managed hooks win wholesale (the malicious PreToolUse never appears).
        assert_eq!(merged["hooks"], managed["hooks"], "managed hooks always win");
        assert_eq!(
            merged["hooks"]["PreToolUse"].as_array().unwrap().len(),
            2,
            "both managed PreToolUse hooks (ask + perm) survive"
        );
        // (b) account NON-hook keys are merged in.
        assert_eq!(merged["env"]["MY_ACCOUNT_VAR"], json!("1"));
        assert_eq!(merged["permissions"]["allow"], json!(["Bash(ls:*)"]));
    }

    #[test]
    fn account_non_hook_keys_merge_and_nested_managed_survives() {
        // (c) A nested merge where the account also supplies `hooks.PostToolUse`
        // must not drop the managed `hooks.PreToolUse` sub-key.
        let managed = managed_ask_settings();
        let account = json!({
            "hooks": { "PostToolUse": [{ "matcher": "*", "hooks": [{ "command": "acct-post" }] }], "SessionStart": [{ "hooks": [] }] },
            "statusLine": { "type": "command", "command": "mystatus" },
        });
        let merged = merge_account_under_managed(managed.clone(), Some(&account));
        // Managed PreToolUse + PostToolUse both intact (managed wins on PostToolUse).
        assert_eq!(merged["hooks"]["PreToolUse"], managed["hooks"]["PreToolUse"]);
        assert_eq!(merged["hooks"]["PostToolUse"], managed["hooks"]["PostToolUse"]);
        // Account's brand-new hook event (no managed counterpart) is added.
        assert!(merged["hooks"]["SessionStart"].is_array());
        // Non-hook top-level account key merged in.
        assert_eq!(merged["statusLine"]["command"], json!("mystatus"));
    }

    #[test]
    fn no_account_settings_is_managed_only() {
        let managed = managed_ask_settings();
        assert_eq!(merge_account_under_managed(managed.clone(), None), managed);
        // A non-object account blob is treated as absent (never merged).
        assert_eq!(merge_account_under_managed(managed.clone(), Some(&json!("garbage"))), managed);
    }

    #[test]
    fn launch_env_merges_server_env_over_hint_when_account_bound() {
        // CCT-460 follow-up: a bound session launches with the server-resolved
        // gateway env merged OVER the pushed hint. Gateway keys win for routing,
        // but user-supplied non-gateway env (e.g. FOO) survives the relaunch
        // instead of being dropped.
        let resp = GatewayEnvResponse {
            account_bound: true,
            env: env_of(&[("ANTHROPIC_BASE_URL", "https://x/gateway/anthropic")]),
            ..Default::default()
        };
        let hint = env_of(&[("FOO", "bar"), ("ANTHROPIC_BASE_URL", "https://stale")]);
        let got = launch_env_decision("s1", &resp, &hint).unwrap();
        assert_eq!(
            got,
            env_of(&[("FOO", "bar"), ("ANTHROPIC_BASE_URL", "https://x/gateway/anthropic")])
        );
    }

    #[test]
    fn launch_env_fails_closed_when_bound_but_empty() {
        // CCT-460: account-bound + empty env must REFUSE the launch rather than
        // start a worker that silently routes to the default upstream and 401s.
        let resp = GatewayEnvResponse {
            account_bound: true,
            env: std::collections::BTreeMap::default(),
            ..Default::default()
        };
        let err = launch_env_decision("s1", &resp, &env_of(&[("HINT", "1")])).unwrap_err();
        assert!(err.to_string().contains("account-bound"), "got: {err}");
    }

    #[test]
    fn launch_env_uses_hint_when_not_bound() {
        // No account binding: gateway env isn't required; preserve any hint
        // (e.g. user-supplied non-gateway env) and never fail closed.
        let resp = GatewayEnvResponse {
            account_bound: false,
            env: std::collections::BTreeMap::default(),
            ..Default::default()
        };
        let hint = env_of(&[("FOO", "bar")]);
        assert_eq!(launch_env_decision("s1", &resp, &hint).unwrap(), hint);
    }

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
            gone: false,
            dead: false,
            alive: None,
            status: None,
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

    #[test]
    fn session_context_block_lists_env_names_not_values() {
        use cctui_proto::adapter::{AdapterId, PermissionMode, SessionSpec};
        let mut env = std::collections::BTreeMap::new();
        env.insert("CCTUI_GITHUB_TOKEN".to_owned(), "super-secret".to_owned());
        env.insert("REGISTRY_USER".to_owned(), "admin".to_owned());
        let spec = SessionSpec {
            adapter_id: AdapterId::new("claude-code"),
            working_dir: Some("/work/cctui".to_owned()),
            prompt: Some("refactor it".to_owned()),
            name: Some("refactor the dispatcher".to_owned()),
            permission_mode: Some(PermissionMode::Auto),
            effort: Some("high".to_owned()),
            model: Some("opus".to_owned()),
            env,
            bootstrap: serde_json::Value::Null,
        };
        let block = build_session_context(&spec, "/work/cctui", &["a.rs".to_owned()]);
        assert!(block.starts_with("<session-context>\n"));
        assert!(block.ends_with("</session-context>"));
        assert!(block.contains("session: refactor the dispatcher"));
        assert!(block.contains("model: opus · effort: high"));
        // acceptEdits/Auto normalizes to `auto`.
        assert!(block.contains("permission-mode: auto"));
        assert!(block.contains("cwd: /work/cctui"));
        assert!(block.contains("env (names only): CCTUI_GITHUB_TOKEN, REGISTRY_USER"));
        assert!(block.contains("  - a.rs"));
        // VALUES must never leak.
        assert!(!block.contains("super-secret"));
        assert!(!block.contains("admin"));
    }

    #[test]
    fn stage_mid_chat_files_suffixes_name_collisions() {
        use base64::Engine;
        use cctui_proto::adapter::BootstrapFile;

        let session_id = format!("test-{}", uuid::Uuid::new_v4());
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
        let dir = std::path::Path::new("/tmp/cctui-uploads").join(&session_id);

        // First upload stages report.pdf.
        let first = stage_mid_chat_files(
            &session_id,
            &[BootstrapFile { name: "report.pdf".into(), content_b64: b64("one") }],
        )
        .expect("stage ok");
        assert_eq!(first, vec![dir.join("report.pdf").to_string_lossy().into_owned()]);

        // A later upload with the same name must NOT overwrite — it gets a suffix.
        let second = stage_mid_chat_files(
            &session_id,
            &[
                BootstrapFile { name: "report.pdf".into(), content_b64: b64("two") },
                BootstrapFile { name: "report.pdf".into(), content_b64: b64("three") },
            ],
        )
        .expect("stage ok");
        assert_eq!(
            second,
            vec![
                dir.join("report-1.pdf").to_string_lossy().into_owned(),
                dir.join("report-2.pdf").to_string_lossy().into_owned(),
            ]
        );
        assert_eq!(std::fs::read_to_string(dir.join("report.pdf")).unwrap(), "one");
        assert_eq!(std::fs::read_to_string(dir.join("report-1.pdf")).unwrap(), "two");
        assert_eq!(std::fs::read_to_string(dir.join("report-2.pdf")).unwrap(), "three");

        let _ = std::fs::remove_dir_all(&dir);
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
    async fn empty_env_launch_is_trusted_and_never_a_heal_candidate() {
        // CCT-462 regression (v0.7.47): a session bound to the user's own
        // subscription account (e.g. `personal`) resolves to an EMPTY gateway
        // env — it routes via the user's own credentials, not a gateway token —
        // but cctui still LAUNCHED it, so the proactive heal must never target
        // it. `note_launched_with_env` must record the worker regardless of env
        // emptiness; recording only non-empty launches misclassified these as
        // autonomous respawns and force-killed healthy live sessions.
        let (d, _rx) = driver();
        d.note_launched_with_env("aaaa1111", &std::collections::BTreeMap::default());
        assert!(
            !d.gateway_heal.lock().unwrap().is_candidate("aaaa1111"),
            "a cctui-launched worker must be trusted even with empty gateway env (CCT-462)"
        );
    }

    #[tokio::test]
    async fn startup_roster_is_grandfathered_not_healed() {
        // CCT-509: the in-memory HealTracker is empty after a daemon (re)start
        // (self-update re-exec / sleep-wake), so every session that was already
        // alive would look like an env-less autonomous respawn and be
        // force-killed — the restart-amnesia kill storm. The FIRST successful
        // snapshot must grandfather the pre-existing roster as trusted.
        let (mut d, _rx) = driver();
        d.apply_snapshot(vec![snap("aaaa1111", "working", None)]).await;
        assert!(
            !d.gateway_heal.lock().unwrap().is_candidate("aaaa1111"),
            "a session already alive at daemon startup must be grandfathered, not a heal candidate (CCT-509)"
        );
    }

    #[tokio::test]
    async fn worker_appearing_after_startup_stays_a_heal_candidate() {
        // CCT-509: grandfathering applies ONLY to the first snapshot. A worker
        // that flips absent→present LATER is exactly the heal's genuine target —
        // an autonomous claude-daemon respawn cctui never launched this lifetime
        // — and must remain a candidate so the env-less case is still healed.
        let (mut d, _rx) = driver();
        d.apply_snapshot(vec![snap("aaaa1111", "working", None)]).await; // startup → grandfathered
        d.apply_snapshot(vec![
            snap("aaaa1111", "working", None),
            snap("bbbb2222", "working", None),
        ])
        .await;
        let (aaaa_candidate, bbbb_candidate) = {
            let t = d.gateway_heal.lock().unwrap();
            (t.is_candidate("aaaa1111"), t.is_candidate("bbbb2222"))
        };
        assert!(!aaaa_candidate, "the grandfathered startup session stays trusted");
        assert!(bbbb_candidate, "a post-startup arrival is the heal's genuine target");
    }

    #[tokio::test]
    async fn socket_loss_flush_keeps_heal_trust() {
        // CCT-509: a flush fires when the control socket is momentarily
        // unreachable, NOT when workers die — they stay alive and reappear next
        // poll. Forgetting their launched-with-env trust here made cctui
        // force-kill its own live sessions once the socket returned. Trust must
        // survive the flush.
        let (mut d, _rx) = driver();
        d.apply_snapshot(vec![snap("aaaa1111", "working", None)]).await; // in roster + trusted
        assert!(!d.gateway_heal.lock().unwrap().is_candidate("aaaa1111"));
        d.flush_roster(EndReason::Other { detail: "socket blip".into() }).await;
        assert!(
            !d.gateway_heal.lock().unwrap().is_candidate("aaaa1111"),
            "heal trust must survive a transient socket-loss flush — the worker is still alive (CCT-509)"
        );
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

    #[test]
    fn is_dead_parses_defensive_shapes() {
        // CCT-252: no live known-dead sample, so several plausible shapes.
        let mut s = snap("abcd1234", "working", None);
        assert!(!s.is_dead(), "live working session is not dead");

        s.gone = true;
        assert!(s.is_dead(), "gone flag → dead");
        s.gone = false;

        s.dead = true;
        assert!(s.is_dead(), "dead flag → dead");
        s.dead = false;

        s.alive = Some(false);
        assert!(s.is_dead(), "alive:false → dead");
        s.alive = Some(true);
        assert!(!s.is_dead(), "alive:true → not dead");
        s.alive = None;

        s.status = Some("Exited".into());
        assert!(s.is_dead(), "status:exited (case-insensitive) → dead");
        s.status = Some("process gone".into());
        assert!(s.is_dead(), "status:'process gone' → dead");
        s.status = Some("running".into());
        assert!(!s.is_dead(), "status:running → not dead");
        s.status = None;

        s.state = Some("gone".into());
        assert!(s.is_dead(), "state:gone → dead");
        s.state = Some("working".into());
        assert!(!s.is_dead(), "state:working → not dead");
        s.state = None;

        s.tempo = Some("dead".into());
        assert!(s.is_dead(), "tempo:dead → dead");
        s.tempo = None;

        // CCT-355: the observed live shape — state:"failed", tempo:"idle",
        // detail:"process gone while supervisor was down". The phrase is
        // embedded in a sentence in `detail`, so it must match as a substring.
        s.state = Some("failed".into());
        s.tempo = Some("idle".into());
        s.detail = Some("process gone while supervisor was down".into());
        assert!(s.is_dead(), "detail containing 'process gone' → dead");
        s.detail = Some("Working on the fix".into());
        assert!(!s.is_dead(), "ordinary detail → not dead");
    }

    #[tokio::test]
    async fn dead_in_roster_emits_hibernated_with_state_json() {
        // CCT-252 B2: a still-listed session that claude reports dead emits a
        // hibernated Status (state.json survives → revivable red dot) within
        // one poll, without waiting for roster disappearance.
        let (mut d, mut rx) = driver();
        // Start it live.
        d.apply_snapshot(vec![snap("aaaa0001", "working", None)]).await;
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::SessionStarted { .. }));
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::Status { .. }));

        // Persist on-disk job state so the dead transition picks hibernated.
        let short_dir = d.cfg.jobs_root.join("aaaa0001");
        std::fs::create_dir_all(&short_dir).unwrap();
        std::fs::write(
            short_dir.join("state.json"),
            r#"{"sessionId":"sess-a","cwd":"/tmp","state":"working"}"#,
        )
        .unwrap();

        // Same short, now reported dead but STILL listed.
        let mut dead = snap("aaaa0001", "working", None);
        dead.gone = true;
        d.apply_snapshot(vec![dead]).await;
        let evt = rx.recv().await.unwrap();
        match evt {
            AdapterEvent::Status { tempo, .. } => {
                assert_eq!(tempo.as_deref(), Some("hibernated"));
            }
            other => panic!("expected hibernated Status, got {other:?}"),
        }

        // B3 sticky: a second poll still reporting dead must NOT re-emit
        // (no Status, no SessionEnded) — the dot can't be re-greened.
        let mut dead2 = snap("aaaa0001", "working", None);
        dead2.gone = true;
        d.apply_snapshot(vec![dead2]).await;
        assert!(rx.try_recv().is_err(), "dead-in-roster is sticky: no re-emit");
    }

    #[tokio::test]
    async fn dead_in_roster_emits_ended_without_state_json() {
        // CCT-252 B2: dead-but-listed with no surviving job state → SessionEnded
        // (the server marks the row `ended`, which is sticky per CCT-192).
        let (mut d, mut rx) = driver();
        d.apply_snapshot(vec![snap("bbbb0002", "working", None)]).await;
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::SessionStarted { .. }));
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::Status { .. }));

        let mut dead = snap("bbbb0002", "working", None);
        dead.status = Some("exited".into());
        d.apply_snapshot(vec![dead]).await;
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, AdapterEvent::SessionEnded { .. }));
    }

    #[tokio::test]
    async fn revive_clears_dead_sticky_and_resumes_status() {
        // CCT-252: if claude reports the short alive again after we marked it
        // dead, the sticky flag clears and live Status flows once more.
        let (mut d, mut rx) = driver();
        d.apply_snapshot(vec![snap("cccc0003", "working", None)]).await;
        rx.recv().await.unwrap(); // Started
        rx.recv().await.unwrap(); // Status

        let mut dead = snap("cccc0003", "working", None);
        dead.dead = true;
        d.apply_snapshot(vec![dead]).await;
        // SessionEnded (no state.json).
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::SessionEnded { .. }));

        // Revived: alive again → a fresh Status is emitted.
        d.apply_snapshot(vec![snap("cccc0003", "busy", None)]).await;
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::Status { .. }));
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
    async fn workflow_subagent_carries_workflow_meta() {
        // CCT-225: a Workflow-tool agent under subagents/workflows/<runId>/ is
        // discovered and its SessionStarted meta.extra carries workflow context.
        use std::io::Write;
        let (mut d, mut rx) = driver();
        let sess = "abcd1234-uuid";
        let parent_path = transcript::transcript_path(&d.cfg.projects_root, "/tmp", sess);
        let run_dir = transcript::subagents_dir(&parent_path).join("workflows").join("wf_test123");
        std::fs::create_dir_all(&run_dir).unwrap();
        let mut f = std::fs::File::create(run_dir.join("agent-wfa.jsonl")).unwrap();
        f.write_all(br#"{"type":"assistant","isSidechain":true,"agentId":"wfa","message":{"content":[{"type":"text","text":"wf work"}]}}"#).unwrap();
        f.write_all(b"\n").unwrap();
        std::fs::write(
            run_dir.join("agent-wfa.meta.json"),
            br#"{"agentType":"workflow-subagent"}"#,
        )
        .unwrap();
        // Run-state with name (sibling: <session>/workflows/<runId>.json).
        let wf_state = parent_path.with_extension("").join("workflows");
        std::fs::create_dir_all(&wf_state).unwrap();
        std::fs::write(wf_state.join("wf_test123.json"), br#"{"name":"deep-research"}"#).unwrap();

        d.apply_snapshot(vec![snap("abcd1234", "working", None)]).await;

        let mut extra = None;
        while let Ok(evt) = rx.try_recv() {
            if let AdapterEvent::SessionStarted { local_id, meta } = evt
                && local_id == "wfa"
            {
                extra = Some(meta.extra);
            }
        }
        let extra = extra.expect("workflow subagent SessionStarted expected");
        assert_eq!(extra.get("subagent").and_then(serde_json::Value::as_bool), Some(true));
        assert_eq!(extra.get("workflow_run_id").and_then(|v| v.as_str()), Some("wf_test123"));
        assert_eq!(extra.get("workflow_name").and_then(|v| v.as_str()), Some("deep-research"));
        assert_eq!(extra.get("agent_type").and_then(|v| v.as_str()), Some("workflow-subagent"));
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
