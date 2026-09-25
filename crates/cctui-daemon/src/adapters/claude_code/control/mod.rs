//! Driver for the `claude daemon` control-socket adapter path.
//!
//! Polls `list` every `poll_interval`, diffs against the previous roster
//! to emit `SessionStarted` / `SessionEnded`, and merges identity fields
//! from `~/.claude/jobs/<short>/state.json` to produce `Status` events.
//!
//! Per-session `subscribe` streams and the transcript tail land in
//! Phase 3 — `list` already gives us state/tempo/detail at the
//! 2s poll cadence.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;
use cctui_proto::adapter::{
    AdapterCommand, AdapterEvent, EndReason, JobShort, RemoveInitiator, SessionMeta,
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::backfill::{self, BackfillConfig, CursorFile, default_cursor_path};
use super::discovery::Discovery;
use super::dispatch_done::{self, DispatchDoneTracker};
use super::kickstart::Kickstarter;
use super::launch::{self, JobIds, LaunchArgs};
use super::state::{StateJson, default_jobs_root};
use super::transcript::{self, OffsetStore, default_projects_root};
use super::{SessionMap, socket};
use crate::adapter_runtime::{CommandOutcome, Handled, SessionDriver};

mod diagnose;
mod removal;
mod reply;
mod settings;
mod snapshot;
mod spawn;
mod subagents;
mod tail;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub(super) use settings::ensure_hook_settings;
pub use settings::stage_mid_chat_files;
use settings::{
    agent_relay_config, build_session_context, detect_whip_from_settings, stage_uploads,
};

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
    /// Local socket the `AskUserQuestion` hook delivers to. Shared
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

/// The `source` every cctui dispatch stamps on its jobs. An absent source is
/// treated as ours (older claude builds omit the field).
pub(super) const FLEET_SOURCE: &str = "fleet";

const SPARE_SOURCE: &str = "spare";

/// How many times one `claude rm` target may be retried while clearing the
/// occupants of its worktree.
const MAX_REMOVE_ATTEMPTS: u8 = 3;

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
    /// tool-permission prompt it reads e.g. `"approve Bash: touch /tmp/x"`.
    /// Empty/absent when nothing is pending.
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
    /// Claude's per-session "process gone" flag. When the worker
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

    /// Skip dying workers and the daemon's pre-warmed spares, which are not
    /// sessions anyone drives.
    fn is_user_visible(&self) -> bool {
        !self.dying && !self.is_spare()
    }

    /// Dispatched by someone other than cctui — a human's `claude --bg`, a
    /// spare, a shell session. Registered and driveable like any other, but no
    /// automatic path of ours may attach to, kill or `claude rm` it.
    fn is_foreign(&self) -> bool {
        self.source.as_deref().is_some_and(|s| s != FLEET_SOURCE)
    }

    fn is_spare(&self) -> bool {
        self.source.as_deref() == Some(SPARE_SOURCE)
    }

    /// Whether claude reports this still-listed session as dead / "process
    /// gone". Parsed DEFENSIVELY across the plausible wire shapes:
    ///   - boolean `gone` / `dead` flags,
    ///   - `alive: false`,
    ///   - a terminal `status`/`state`/`tempo` string
    ///     (`gone`/`exited`/`dead`/`process gone`),
    ///   - a `detail` that CONTAINS `process gone` — the live wire shape we
    ///     actually observed is `state:"failed"`, `tempo:"idle"`,
    ///     `detail:"process gone while supervisor was down"`. None of
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

/// Spawn-time `(model, effort)` pair remembered per worker `short`.
type SpawnModelEffort = (Option<String>, Option<String>);

/// How long after a reply op its turn id keeps landing on the session's user
/// events. Claude emits the re-encodings within seconds; anything later is a
/// different turn, most likely typed into the native TUI.
const TURN_ID_WINDOW: Duration = Duration::from_mins(1);

#[derive(Clone, Copy)]
struct PendingTurn {
    id: uuid::Uuid,
    at: Instant,
}

/// A prepared spawn/fork `dispatch` request whose control-socket round-trip
/// is awaited outside the run loop, so a slow or silent claude daemon can't
/// wedge polling and every later command for the adapter.
pub struct DeferredDispatch {
    sock: PathBuf,
    req: serde_json::Value,
    short: String,
    what: String,
    session_id: String,
    gate: Option<LaunchGate>,
}

/// Everything the launch needs to ask the server whether the job's model may
/// run yet, and to report the wait on the session card.
pub struct LaunchGate {
    server: crate::client::ServerClient,
    machine_key: String,
    session_id: String,
    short: String,
    model: Option<String>,
    events: mpsc::Sender<AdapterEvent>,
}

impl LaunchGate {
    /// Block until the job's model is allowed, the hold outlives
    /// [`crate::launchgate::MAX_HOLD`], or the limits call fails.
    async fn hold(&self) {
        let began = Instant::now();
        let mut waiting = false;
        loop {
            let limits = match self
                .server
                .session_limits(&self.machine_key, &self.session_id, self.model.as_deref())
                .await
            {
                Ok(limits) => limits,
                // Fail open: a limits endpoint having a bad day must not stop
                // launches.
                Err(err) => {
                    tracing::warn!(session = %self.session_id, %err, "launch limits check failed; launching anyway");
                    return;
                }
            };
            let Some(hold) = crate::launchgate::hold_from_limits(&limits, self.model.as_deref())
            else {
                if waiting {
                    tracing::info!(
                        session = %self.session_id,
                        waited_secs = %began.elapsed().as_secs(),
                        "launch limit cleared; dispatching"
                    );
                    self.report(None).await;
                }
                return;
            };
            if crate::launchgate::expired(began) {
                tracing::warn!(
                    session = %self.session_id,
                    reason = %hold.reason,
                    "launch held too long; dispatching anyway"
                );
                self.report(None).await;
                return;
            }
            if !waiting {
                tracing::info!(
                    session = %self.session_id,
                    model = ?self.model,
                    reason = %hold.reason,
                    retry_after_secs = %hold.retry_after.as_secs(),
                    "holding launch: the model is limit blocked"
                );
            }
            waiting = true;
            self.report(Some(&hold)).await;
            tokio::time::sleep(crate::launchgate::backoff(&hold)).await;
        }
    }

    /// Put the wait (or its end) on the session card.
    async fn report(&self, hold: Option<&crate::launchgate::Hold>) {
        let state = if hold.is_some() { "held" } else { "starting" };
        let _ = self
            .events
            .send(AdapterEvent::Status {
                local_id: self.short.clone(),
                tempo: None,
                state: Some(state.to_owned()),
                detail: hold.map(crate::launchgate::Hold::card_detail),
                activity: None,
                name: None,
                intent: None,
                model: self.model.clone(),
                effort: None,
                permission_mode: None,
                children: Vec::new(),
            })
            .await;
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
    /// Shorts claude reports dead-but-still-listed. Once we emit the
    /// dead transition (hibernated or `SessionEnded`) we record the short here
    /// and suppress further live-status emits for it, so the still-present
    /// roster entry can't re-emit a non-terminal Status and re-green the dot
    /// (daemon-side sticky, mirroring the server's sticky terminal status).
    /// Cleared when the worker revives (reports alive again) or drops
    /// off the roster.
    dead_shorts: HashSet<String>,
    /// Shorts the last `list` reported with a non-fleet `source`. No automatic
    /// path may attach to, kill or `claude rm` one of these.
    foreign_shorts: HashSet<String>,
    /// Whether any of those is a session a human drives (a spare is not), which
    /// vetoes the version gate's `daemon stop --any`.
    native_live: bool,
    /// Shared `session_id → stable local_id` map. Populated as transcripts are
    /// pinned (incl. across `/clear` rotations) and read by the ask-hook
    /// listener so a hook's live `session_id` resolves to the `local_id` the
    /// server keys on.
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
    /// Task-tool subagents currently tracked, keyed by `agentId`.
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
    /// socket present".
    kickstarter: Kickstarter,
    /// Cycles the claude daemon when a CLI auto-update left it on an older
    /// version, but only while no worker is running.
    version_gate: super::version_gate::VersionGate,
    /// Holds a persistent headless `attach` open per live session so the
    /// dispatched worker actually wakes (focus-in seed) and is kept off the
    /// 60s idle-retire path. Without this, dispatched/replied sessions sit in
    /// limbo until a human opens them in `claude agents`.
    attach: super::attach::AttachManager,
    /// Tool-permission prompts currently pending, keyed by worker `short`.
    /// Derived from the snapshot's `tempo:"blocked"`/`needs` signal:
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
    /// maintained by the ask-hook listener. A `reply` injected while
    /// the form is up would just confirm the highlighted option — the reply
    /// path dismisses the form (attach+ESC) first so the user's actual text is
    /// what claude receives.
    pending_asks: super::PendingAsks,
    /// Tool-permission `PreToolUse` hooks currently parked in the ask-hook
    /// listener, long-polling for a human's decision. Keyed by the
    /// session's stable `local_id`. The `PermissionResponse` handler resolves
    /// the matching entry — handing the decision straight back to the blocked
    /// hook (which returns an `allow`/`deny` decision to Claude Code) — instead
    /// of attaching + injecting `1\r`/ESC keystrokes. The keystroke path is kept
    /// only as the fallback for when no hook is registered (hook timed out, or
    /// a prompt that surfaced via the legacy `tempo:"blocked"` signal).
    pending_perm_hooks: super::PendingPermHooks,
    /// When the last periodic divergence check ran. At idle
    /// it's a no-op; it re-sends a bounded window only when a session's offset
    /// has run ahead of the server's mark (see [`Driver::reconcile_tail`]).
    last_reconcile: Instant,
    /// Set when the control socket vanished (roster flushed) so the next
    /// successful poll triggers an immediate reconciliation re-tail rather
    /// than waiting for the periodic cycle.
    churned: bool,
    /// Best-known server transcript high-water mark per `offset_key`.
    /// Seeded by the server's `ResumeMarks` on connect and advanced as the
    /// forward tail emits, so the periodic pass only re-sends on real
    /// divergence (local offset ahead of what the server holds).
    server_marks: HashMap<String, u64>,
    /// Transcript marks the server itself reported (`ResumeMarks`), never
    /// advanced by our own emits: the only proof of what it stored.
    acked_marks: HashMap<String, u64>,
    /// Spawn-time `--model`/`--effort` remembered per worker `short`.
    /// Used as a fallback for the Status event when `state.json` isn't on disk
    /// yet (freshly spawned) or transiently absent (`/clear` rotation), so the
    /// session list still shows the model/effort we launched the worker with.
    /// `Mutex` because `spawn` takes `&self` while the poll loop holds `&mut self`.
    spawn_model_effort: std::sync::Mutex<HashMap<String, SpawnModelEffort>>,
    /// The turn id of the reply most recently injected into each session, with
    /// the instant it was injected. Claude re-encodes one injected turn as
    /// several transcript lines, so every user event inside
    /// [`TURN_ID_WINDOW`] takes the same id; past the window a turn typed
    /// straight into the native TUI would otherwise inherit it.
    /// `Mutex` because the reply path takes `&self` while the poll loop holds
    /// `&mut self`.
    pending_turns: std::sync::Mutex<HashMap<String, PendingTurn>>,
    /// Parent session id remembered per freshly-forked child `short`.
    /// `fork` dispatches a new worker but the `SessionStarted` for it is emitted
    /// later by the poll loop when the short first appears in the roster — that
    /// path has no idea it was a fork, so we stash the parent here and the
    /// roster-discovery emit reads it to set `SessionMeta::parent_local_id` (the
    /// link the server resolves into `parent_id`). `Mutex` for the same reason as
    /// `spawn_model_effort`.
    fork_parent_by_short: std::sync::Mutex<HashMap<String, (String, &'static str)>>,
    /// Authenticated server client + machine key for the launch-time gateway-env
    /// pull. Every worker (re)launch resolves the session's account
    /// env here from the server's durable `sessions.account_id` binding, so
    /// routing survives a daemon / claude-daemon restart and session-id rotation
    /// instead of depending on env carried by the triggering command. `None` in
    /// tests / when no server is configured — the chokepoint then falls back to
    /// the pushed env hint.
    server: Option<crate::client::ServerClient>,
    machine_key: Option<String>,
    /// Turn-complete watcher for the one session `maybe_dispatch_on_start`
    /// launched: writes `<jobs_root>/<short>/dispatch_done` once
    /// that session has been busy and then settles idle, so the worker
    /// entrypoint can wind the pod down instead of idling to the Job
    /// deadline. `None` on normal (non-dispatched) daemons — interactive
    /// sessions never get a marker. `Mutex` because it's armed from
    /// `maybe_dispatch_on_start` (`&self`).
    dispatch_done: std::sync::Mutex<Option<DispatchDoneTracker>>,
    /// Last ask/permission/plan hook delivery per `local_id`, maintained by
    /// the ask-hook listener. Read by the diagnose aggregation.
    hook_log: super::HookLog,
    /// When each short was last seen in a `list` snapshot — the observation
    /// timestamp behind the diagnose report's effective-state fact.
    last_status_at: HashMap<String, std::time::SystemTime>,
    /// Kind + time of the last event parsed out of each transcript tail,
    /// keyed by `offset_key`.
    last_parsed: HashMap<String, (String, std::time::SystemTime)>,
    /// Permission posture (`default`/`auto`/`yolo`/`whip`) recorded per
    /// worker `short` at spawn/fork time. `Mutex` for the same
    /// reason as `spawn_model_effort`.
    spawn_permission_mode: std::sync::Mutex<HashMap<String, String>>,
    /// Read-only live-view PTY relay. Opens a fresh viewer attach per
    /// watched session while a browser has its terminal open, forwarding
    /// coalesced PTY bytes as `PtyChunk` events. Interior-mutable so the `&self`
    /// command path can start/stop viewers.
    pty_view: super::pty_view::PtyViewManager,
    last_reseed: Option<Instant>,
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

/// Everything the launch chokepoint pulls from the server's durable binding for
/// a (re)launch: the gateway-routing `env`, the per-account `settings_json`
/// the daemon merges under its managed hook settings, and the user's
/// `whipStopPhrases` override.
#[derive(Debug, Default)]
pub(super) struct LaunchEnv {
    pub env: std::collections::BTreeMap<String, String>,
    pub settings: Option<serde_json::Value>,
    pub whip_phrases: Option<serde_json::Value>,
    /// Present only when the server says this session may spawn subagents; it
    /// gates whether the `CctuiAgent` MCP server is registered at all.
    pub spawn_capability: Option<cctui_proto::api::SpawnCapability>,
}

/// Resolve a launch env into a full [`LaunchEnv`] from the server pull, shared
/// by the control/headless/oneshot drivers. Fail-closed refusal (account-bound
/// but missing/partial gateway env) surfaces as `Err`; a pull failure or absent
/// server degrades to `hint`.
pub(super) async fn resolve_launch_env_for(
    server: Option<&crate::client::ServerClient>,
    machine_key: Option<&String>,
    local_id: &str,
    hint: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<LaunchEnv> {
    let (Some(server), Some(mk)) = (server, machine_key) else {
        return Ok(LaunchEnv { env: with_resume_guard(hint.clone()), ..Default::default() });
    };
    match server.gateway_env(mk, local_id).await {
        Ok(resp) => Ok(LaunchEnv {
            env: with_resume_guard(crate::adapters::gateway_env::launch_env_decision(
                "claude",
                local_id,
                &resp,
                hint,
                crate::adapters::gateway_env::CLAUDE_GATEWAY_KEYS,
            )?),
            settings: resp.settings,
            whip_phrases: resp.whip_phrases,
            spawn_capability: resp.spawn_capability,
        }),
        Err(e) => {
            tracing::warn!(%local_id, "gateway-env pull failed; falling back to pushed env: {e}");
            Ok(LaunchEnv { env: with_resume_guard(hint.clone()), ..Default::default() })
        }
    }
}

/// Bound what Claude Code's own supervisor may do when it respawns one of our
/// workers: no auto-continue, and a max age so the injected
/// `CLAUDE_CODE_RESUME_PROMPT` continuation is skipped too — unset or `0` there
/// means *no* bound, which is why it must be written explicitly. Caller-supplied
/// values win.
fn with_resume_guard(
    mut env: std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    for (key, value) in [
        ("CLAUDE_CODE_RESUME_INTERRUPTED_TURN", "0"),
        ("CLAUDE_CODE_RESUME_INTERRUPTED_TURN_MAX_AGE_MS", RESUME_INTERRUPTED_TURN_MAX_AGE_MS),
    ] {
        env.entry(key.to_owned()).or_insert_with(|| value.to_owned());
    }
    env
}

/// One minute: long enough that a worker restarted while a human watches can
/// still pick its turn up, short enough that a supervisor revive hours later
/// never re-sends a whole context.
const RESUME_INTERRUPTED_TURN_MAX_AGE_MS: &str = "60000";

/// Parse `CCTUI_GATEWAY_RESEED_SECS` (positive integer seconds) or fall back to
/// one hour — comfortably under the server's default 12h token TTL.
fn reseed_interval_from(var: Option<String>) -> Duration {
    var.and_then(|v| v.parse::<u64>().ok())
        .filter(|s| *s > 0)
        .map_or(Duration::from_hours(1), Duration::from_secs)
}

/// Whether the gateway-env re-seed pass should run this poll: always on a
/// (re)attach, otherwise once `interval` has elapsed since the last pass (and
/// unconditionally on the very first pass).
fn reseed_due(last: Option<Instant>, interval: Duration, reattached: bool) -> bool {
    reattached || last.is_none_or(|t| t.elapsed() >= interval)
}

impl Driver {
    pub fn new(
        cfg: DriverConfig,
        events: mpsc::Sender<AdapterEvent>,
        commands: mpsc::Receiver<AdapterCommand>,
        shutdown: CancellationToken,
    ) -> Self {
        // Offsets are kept in-memory only in production: the
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
        let version_gate = super::version_gate::VersionGate::new(cfg.claude_bin.clone());
        let attach = super::attach::AttachManager::new(cfg.discovery.clone(), shutdown.clone());
        let pty_view = super::pty_view::PtyViewManager::new(
            events.clone(),
            cfg.discovery.clone(),
            shutdown.clone(),
        );
        Self {
            cfg,
            events,
            commands,
            shutdown,
            roster: HashSet::new(),
            last_status: HashMap::new(),
            dead_shorts: HashSet::new(),
            foreign_shorts: HashSet::new(),
            native_live: false,
            session_to_local: Arc::new(Mutex::new(HashMap::new())),
            offsets,
            transcript_locations: HashMap::new(),
            short_by_session: HashMap::new(),
            subagents: HashMap::new(),
            ended_subagents: HashSet::new(),
            kickstarter,
            version_gate,
            attach,
            pending_perms: HashMap::new(),
            perm_seq: 0,
            pending_asks: super::PendingAsks::default(),
            pending_perm_hooks: super::PendingPermHooks::default(),
            last_reconcile: Instant::now(),
            churned: false,
            server_marks: HashMap::new(),
            acked_marks: HashMap::new(),
            spawn_model_effort: std::sync::Mutex::new(HashMap::new()),
            pending_turns: std::sync::Mutex::new(HashMap::new()),
            fork_parent_by_short: std::sync::Mutex::new(HashMap::new()),
            server: None,
            machine_key: None,
            dispatch_done: std::sync::Mutex::new(None),
            hook_log: super::HookLog::default(),
            last_status_at: HashMap::new(),
            last_parsed: HashMap::new(),
            spawn_permission_mode: std::sync::Mutex::new(HashMap::new()),
            pty_view,
            last_reseed: None,
        }
    }

    /// Attach the authenticated server client + machine key used by the
    /// launch-time gateway-env pull. Builder-style so the test
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

    /// How often the periodic reconciliation re-tail runs. Chosen
    /// in the 30–60s band: frequent enough that a dropped-send gap self-heals
    /// quickly, infrequent enough that the re-emitted (then deduped) volume is
    /// negligible next to the regular poll tail.
    const RECONCILE_INTERVAL: Duration = Duration::from_secs(45);

    /// Must stay well under the server's session-token TTL (default 12h) so a
    /// live worker's token is re-minted before it can expire.
    fn reseed_interval() -> Duration {
        reseed_interval_from(std::env::var("CCTUI_GATEWAY_RESEED_SECS").ok())
    }

    /// Clone handle to the shared `session_id → local_id` map, for the
    /// ask-hook listener to translate live `session_id`s.
    pub fn session_map(&self) -> SessionMap {
        self.session_to_local.clone()
    }

    /// Clone handle to the shared pending-ask set, for the ask-hook listener
    /// to maintain.
    pub fn pending_asks(&self) -> super::PendingAsks {
        self.pending_asks.clone()
    }

    /// Clone handle to the shared pending tool-permission hook map, for the
    /// ask-hook listener to register blocked `PreToolUse` hooks into.
    pub fn pending_perm_hooks(&self) -> super::PendingPermHooks {
        self.pending_perm_hooks.clone()
    }

    /// Clone handle to the shared hook-delivery log, for the ask-hook
    /// listener to maintain.
    pub fn hook_log(&self) -> super::HookLog {
        self.hook_log.clone()
    }

    #[allow(clippy::cognitive_complexity)]
    pub async fn run(mut self) -> anyhow::Result<()> {
        if !self.cfg.skip_backfill {
            self.run_backfill().await;
        }
        // Dispatched-worker bring-up: if this daemon was launched as a
        // dispatched kube/docker worker, self-start its session before entering
        // the poll loop. Best-effort — never aborts `run`.
        self.maybe_dispatch_on_start().await;
        let mut tick = tokio::time::interval(self.cfg.poll_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let events = self.events.clone();
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => {
                    self.flush_before_teardown().await;
                    return Ok(());
                }
                _ = tick.tick() => {
                    if let Err(err) = self.poll_once().await {
                        tracing::debug!(%err, "claude daemon poll failed (will retry)");
                    }
                    // Periodic reconciliation re-tail: catch up any
                    // transcript gap the forward-only tail left behind. Driven
                    // off the poll tick (rather than a second timer) so it
                    // can't race apply_snapshot's tail/offset updates.
                    if self.last_reconcile.elapsed() >= Self::RECONCILE_INTERVAL {
                        self.reconcile_tail(false).await;
                        self.maybe_cycle_stale_daemon().await;
                    }
                }
                Some(cmd) = self.commands.recv() => {
                    crate::adapter_runtime::dispatch_command(&mut self, &events, cmd).await;
                }
            }
        }
    }

    /// Shorts the claude daemon currently lists as live — the jobs backfill
    /// must NOT touch (see `backfill::run_once`). No live socket
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

    async fn kill_session(
        &self,
        sock: &Path,
        local_id: &str,
        signal: Option<i32>,
    ) -> anyhow::Result<()> {
        let short = self.resolve_short(local_id)?;
        let mut req = json!({"proto":1,"op":"kill","short":short});
        if let Some(s) = signal {
            // Claude's control-socket `kill` op validates `signal`
            // against the string enum ["SIGTERM","SIGKILL"] (zod). A
            // numeric signal (e.g. the interrupt route's `15`) fails
            // that validation and the whole op is rejected silently. The
            // control socket exposes no in-place turn-interrupt op, so
            // the best we can do for a headless worker is terminate it;
            // map to the enum name the daemon accepts.
            req["signal"] = serde_json::Value::String(kill_signal_name(s).to_owned());
        }
        let resp = socket::one_shot(sock, &req).await?;
        tracing::debug!(?resp, %short, "kill ack");
        Ok(())
    }

    async fn answer_permission(
        &self,
        sock: &Path,
        local_id: &str,
        request_id: &str,
        allow: bool,
    ) -> anyhow::Result<()> {
        // Preferred path: a bidirectional `PreToolUse` hook is
        // blocked in the listener long-polling for this decision. Hand
        // it the human's allow/deny straight back — the hook returns the
        // decision to Claude Code, so the tool runs/skips with no attach
        // and no keystroke at all. `take`n so a duplicate response can't
        // double-fire on an already-resolved (and dropped) channel.
        let hook = self.pending_perm_hooks.lock().ok().and_then(|mut map| map.remove(local_id));
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
        // Fallback: no hook registered (timed out, or the
        // prompt surfaced only via the legacy `tempo:"blocked"`/`needs`
        // signal). The control socket's `permission-response` op is a
        // no-op stub, so answer the way a human does — attach to the PTY
        // and inject `1`+Enter (approve) or ESC (deny).
        let short = self.resolve_short(local_id)?;
        socket::attach_permission_response(sock, &short, allow).await?;
        tracing::info!(%short, %request_id, allow, "answered permission prompt via attach (fallback)");
        Ok(())
    }

    async fn remove_session(
        &self,
        sock: &Path,
        local_id: &str,
        initiator: RemoveInitiator,
    ) -> anyhow::Result<()> {
        let short = self.resolve_short_for_removal(local_id)?;
        if !Self::removal_allowed(&self.foreign_shorts, &short, initiator) {
            tracing::info!(
                %short, %local_id,
                "skipping automatic removal of a claude job cctui did not start"
            );
            return Ok(());
        }
        let rm = self.remove_job(sock, &short, local_id, initiator).await;
        crate::configsweep::remove_session_files(&short);
        rm
    }

    async fn report_command(
        events: &mpsc::Sender<AdapterEvent>,
        command_id: Option<uuid::Uuid>,
        res: anyhow::Result<()>,
    ) {
        let Some(command_id) = command_id else { return };
        let (ok, error) = match res {
            Ok(()) => (true, None),
            Err(err) => (false, Some(err.to_string())),
        };
        let _ = events.send(AdapterEvent::CommandResult { command_id, ok, error }).await;
    }

    fn resolve_short(&self, local_id: &str) -> anyhow::Result<String> {
        self.short_by_session
            .get(local_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown session {local_id}"))
    }

    /// Resolve a short for removal, tolerating sessions that have already left
    /// the live roster. Removal most often targets *completed*
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

    /// Locate the control socket, booting the on-demand claude daemon if it's
    /// missing and waiting (up to ~12s) for the socket to appear. Used on the
    /// command path, where failing to find a socket means a dropped spawn/
    /// reply rather than just a skipped poll. `claude daemon run`
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

    /// Pick up a `claude` CLI auto-update the running daemon missed. Cycles
    /// only when both our roster and the daemon's own worker count agree
    /// nothing is running; a mismatch over live work is logged and left alone.
    async fn maybe_cycle_stale_daemon(&self) {
        use super::version_gate::{CycleMethod, Decision};

        // The direct-spawn fallback (containers) is excluded: those workers run
        // `--no-auto-update`, so they never drift, and we keep no pid to stop.
        if !super::claude_service::manager_available() {
            return;
        }
        if let Some(Decision::Cycle { running, local, escalated: _ }) =
            self.version_gate.check(self.roster.len(), self.native_live).await
        {
            let method = if tokio::task::spawn_blocking(super::claude_service::service_active)
                .await
                .unwrap_or(false)
            {
                CycleMethod::ManagedService
            } else {
                CycleMethod::StopAny
            };
            self.cycle_daemon(method, &running, &local).await;
        }
    }

    #[allow(clippy::cognitive_complexity)]
    async fn cycle_daemon(&self, method: super::version_gate::CycleMethod, run: &str, new: &str) {
        tracing::info!(%run, %new, ?method, "cycling idle claude daemon onto the new CLI");
        if let Err(err) = self.version_gate.cycle(method).await {
            tracing::warn!(%err, ?method, "failed to cycle the stale claude daemon");
            return;
        }
        match self.ensure_socket().await {
            Ok(sock) => tracing::info!(sock = %sock.display(), %new, "claude daemon back up"),
            Err(err) => tracing::warn!(%err, "claude daemon did not come back after the cycle"),
        }
    }

    async fn poll_once(&mut self) -> anyhow::Result<()> {
        let Some(sock) = self.cfg.discovery.locate_live().await else {
            // Daemon isn't running. Boot it (rate-limited) so it self-heals
            // before the next dispatch, and treat any sessions we
            // previously knew about as ended.
            self.kickstarter.kick(false);
            self.flush_roster(EndReason::Other { detail: "daemon gone".into() }).await;
            // Roster churn: the socket vanished and sessions were
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
        let reattached = self.churned;
        if self.churned {
            self.churned = false;
            self.reconcile_tail(true).await;
        }
        if reseed_due(self.last_reseed, Self::reseed_interval(), reattached) {
            self.reseed_gateway_env().await;
            self.last_reseed = Some(Instant::now());
        }
        Ok(())
    }

    /// Renew each live account-bound worker's gateway token by re-pulling its
    /// env. The token STRING is stable, so the running worker (and its persisted
    /// `--settings` delivery) needs no rewrite — the pull only re-mints to bump
    /// the short-TTL expiry. A genuinely env-less worker is deliberately NOT
    /// force-respawned here; that fails loud at the launch chokepoint instead.
    async fn reseed_gateway_env(&self) {
        let (Some(server), Some(mk)) = (self.server.as_ref(), self.machine_key.as_ref()) else {
            return;
        };
        let targets: Vec<String> = self
            .short_by_session
            .iter()
            .filter(|(_, short)| self.roster.contains(*short))
            .map(|(local_id, _)| local_id.clone())
            .collect();
        let mut renewed = 0usize;
        for local_id in &targets {
            match server.gateway_env(mk, local_id).await {
                Ok(resp) if resp.account_bound => renewed += 1,
                Ok(_) => {}
                Err(err) => {
                    tracing::debug!(%local_id, %err, "gateway-env re-seed pull failed (will retry)");
                }
            }
        }
        if renewed > 0 {
            tracing::info!(renewed, "re-seeded gateway env for live account-bound workers");
        }
    }

    async fn emit(&self, evt: AdapterEvent) {
        let _ = self.events.send(self.stamp_turn(evt)).await;
    }

    /// Give a user event the id of the turn cctui injected, when one is still
    /// in flight. Only `role:"user"` events are stamped: the several encodings
    /// Claude stores one turn in must share a key, while two assistant messages
    /// in the same turn must not.
    fn stamp_turn(&self, mut evt: AdapterEvent) -> AdapterEvent {
        if let AdapterEvent::Message { local_id, payload, turn_id } = &mut evt
            && turn_id.is_none()
            && payload.get("role").and_then(serde_json::Value::as_str) == Some("user")
            && let Some(id) = self.turn_for(local_id)
        {
            *turn_id = Some(id);
        }
        evt
    }

    fn note_turn(&self, local_id: &str, turn_id: Option<uuid::Uuid>) {
        let Ok(mut map) = self.pending_turns.lock() else { return };
        match turn_id {
            Some(id) => {
                map.insert(local_id.to_owned(), PendingTurn { id, at: Instant::now() });
            }
            None => {
                map.remove(local_id);
            }
        }
    }

    fn turn_for(&self, local_id: &str) -> Option<uuid::Uuid> {
        let mut map = self.pending_turns.lock().ok()?;
        let pending = *map.get(local_id)?;
        let live = pending.at.elapsed() <= TURN_ID_WINDOW;
        if !live {
            map.remove(local_id);
        }
        drop(map);
        live.then_some(pending.id)
    }
}

/// Path of the managed hook settings file: `$XDG_CONFIG_HOME/cctui/
/// ask-hook-settings.json` (falling back to `~/.config`).
/// Map a numeric kill signal to the string name Claude's control-socket `kill`
/// op accepts. The op validates `signal` against the zod enum
/// `["SIGTERM","SIGKILL"]`, so a numeric value is rejected outright.
/// Only `SIGKILL` (9) maps to a hard kill; everything else (notably the
/// interrupt route's `15`) maps to the graceful `SIGTERM`.
const fn kill_signal_name(signal: i32) -> &'static str {
    if signal == 9 { "SIGKILL" } else { "SIGTERM" }
}

/// Every command but diagnose, the live-view toggle and resume marks needs a
/// live control socket: if the on-demand claude daemon has shut down it is
/// booted and awaited rather than failing the command outright.
#[async_trait::async_trait]
impl SessionDriver for Driver {
    fn adapter_id(&self) -> &'static str {
        "claude-code"
    }

    async fn resume_marks(&mut self, marks: Vec<(String, u64)>) -> CommandOutcome {
        self.apply_resume_marks(marks).await;
        Ok(Handled::Done)
    }

    async fn diagnose(&mut self, local_id: String, request_id: uuid::Uuid) -> CommandOutcome {
        self.handle_diagnose(&local_id, request_id).await?;
        Ok(Handled::Done)
    }

    async fn watch_pty(&mut self, local_id: String, watch: bool) -> CommandOutcome {
        match self.resolve_short(&local_id) {
            Ok(short) if watch => self.pty_view.watch(local_id, short),
            Ok(short) => self.pty_view.unwatch(&short),
            Err(err) => tracing::debug!(%err, watch, "watch_pty for unknown session; ignoring"),
        }
        Ok(Handled::Done)
    }

    async fn send_message(&mut self, local_id: String, text: String) -> CommandOutcome {
        let sock = self.ensure_socket().await?;
        self.deliver_reply(
            &sock,
            &local_id,
            &text,
            None,
            &std::collections::BTreeMap::default(),
            None,
        )
        .await?;
        Ok(Handled::Done)
    }

    async fn reply(
        &mut self,
        local_id: String,
        text: String,
        ask_picks: Option<Vec<Vec<usize>>>,
        env: std::collections::BTreeMap<String, String>,
        _command_id: Option<uuid::Uuid>,
        turn_id: Option<uuid::Uuid>,
    ) -> CommandOutcome {
        let sock = self.ensure_socket().await?;
        self.deliver_reply(&sock, &local_id, &text, ask_picks, &env, turn_id).await?;
        Ok(Handled::Done)
    }

    async fn kill(&mut self, local_id: String, signal: Option<i32>) -> CommandOutcome {
        let sock = self.ensure_socket().await?;
        self.kill_session(&sock, &local_id, signal).await?;
        Ok(Handled::Done)
    }

    async fn interrupt(
        &mut self,
        local_id: String,
        _command_id: Option<uuid::Uuid>,
    ) -> CommandOutcome {
        let sock = self.ensure_socket().await?;
        // The control socket has no turn-interrupt op: attach to the worker
        // PTY and inject the ESC that aborts a turn in the TUI.
        let short = self.resolve_short(&local_id)?;
        socket::attach_interrupt(&sock, &short).await?;
        tracing::info!(%short, "interrupted in-flight turn via attach+ESC");
        Ok(Handled::Done)
    }

    async fn resume(
        &mut self,
        local_id: String,
        working_dir: Option<String>,
        env: std::collections::BTreeMap<String, String>,
    ) -> CommandOutcome {
        let sock = self.ensure_socket().await?;
        let short =
            self.resolve_short(&local_id).or_else(|_| self.resolve_short_for_removal(&local_id))?;
        // `claude rm` deletes state.json but keeps the transcript, so an
        // archived session resumes from (local_id, working_dir) alone.
        self.resume_worker(&sock, &short, &local_id, Some(&local_id), working_dir.as_deref(), &env)
            .await?;
        tracing::info!(%short, %local_id, "resumed session via explicit command");
        Ok(Handled::Done)
    }

    async fn permission_response(
        &mut self,
        local_id: String,
        request_id: String,
        allow: bool,
    ) -> CommandOutcome {
        let sock = self.ensure_socket().await?;
        self.answer_permission(&sock, &local_id, &request_id, allow).await?;
        Ok(Handled::Done)
    }

    async fn remove(
        &mut self,
        local_id: String,
        _command_id: Option<uuid::Uuid>,
        initiator: RemoveInitiator,
    ) -> CommandOutcome {
        let sock = self.ensure_socket().await?;
        self.remove_session(&sock, &local_id, initiator).await?;
        Ok(Handled::Done)
    }

    /// The control-socket reply can take as long as a cold worker bring-up
    /// (or never come), so it is awaited off the poll path.
    async fn spawn(
        &mut self,
        spec: cctui_proto::adapter::SessionSpec,
        command_id: Option<uuid::Uuid>,
        session_id: Option<uuid::Uuid>,
    ) -> CommandOutcome {
        let sock = self.ensure_socket().await?;
        let dispatch =
            self.prepare_spawn(&sock, &spec, session_id.map(|id| id.to_string())).await?;
        dispatch.run_detached(self.events.clone(), command_id);
        Ok(Handled::Deferred)
    }

    async fn fork(
        &mut self,
        parent_local_id: String,
        spec: cctui_proto::adapter::SessionSpec,
        command_id: Option<uuid::Uuid>,
        session_id: Option<String>,
        extract: Option<cctui_proto::adapter::ForkExtract>,
    ) -> CommandOutcome {
        let sock = self.ensure_socket().await?;
        let dispatch = self
            .prepare_fork(&sock, &parent_local_id, &spec, session_id.as_deref(), extract.as_ref())
            .await?;
        dispatch.run_detached(self.events.clone(), command_id);
        Ok(Handled::Deferred)
    }

    async fn rename(&mut self, local_id: String, name: String) -> CommandOutcome {
        self.ensure_socket().await?;
        let short = self.resolve_short(&local_id)?;
        // No control-socket rename op exists; the status poll reads the name
        // back from state.json.
        StateJson::write_name(&self.cfg.jobs_root, &short, &name)
            .with_context(|| format!("rename session {short} -> {name}"))?;
        tracing::info!(%short, %name, "renamed session via state.json");
        Ok(Handled::Done)
    }

    async fn set_model(
        &mut self,
        local_id: String,
        _model: Option<String>,
        _effort: Option<String>,
        _command_id: Option<uuid::Uuid>,
    ) -> CommandOutcome {
        self.ensure_socket().await?;
        // Neither the control socket nor this path reaches the Agent SDK's
        // `setModel()`; fork-with-`--model` is the supported substitute.
        tracing::warn!(%local_id, "claude: in-place model/effort switch not supported; fork to change model");
        anyhow::bail!(
            "in-place model/effort switch is not supported for claude sessions — fork to change model"
        );
    }
}
