use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::adapter::AdapterId;
use crate::classifier::Bucket;
use crate::models::{Attention, Liveness, SessionStatus, TokenUsage};

// --- Daemon ↔ Server ---

/// Body for `POST /api/v1/daemon/auth`. The daemon presents its long-lived
/// machine key (issued at enrollment) and receives a short-lived session
/// token used for the subsequent WS upgrade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonAuthRequest {
    pub machine_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonAuthResponse {
    pub session_token: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub machine_id: Uuid,
    pub user_id: Uuid,
}

/// One declarative adapter configuration row, served to the daemon as part
/// of the initial `Reconcile` frame so the daemon knows which adapters to
/// instantiate and with what configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonAdapterConfig {
    pub adapter_id: AdapterId,
    #[serde(default)]
    pub config: serde_json::Value,
    pub enabled: bool,
}

// --- Agent-facing ---

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub machine_id: String,
    pub working_dir: String,
    pub claude_session_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub session_id: String,
    pub ws_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CheckRequest {
    pub session_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CheckResponse {
    #[serde(rename = "hookSpecificOutput")]
    pub hook_specific_output: HookOutput,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HookOutput {
    #[serde(rename = "hookEventName")]
    pub hook_event_name: String,
    #[serde(rename = "permissionDecision", skip_serializing_if = "Option::is_none")]
    pub permission_decision: Option<String>,
    #[serde(rename = "permissionDecisionReason", skip_serializing_if = "Option::is_none")]
    pub permission_decision_reason: Option<String>,
}

// --- TUI-facing ---

const fn default_liveness() -> Liveness {
    Liveness::Dead
}

const fn default_bucket() -> Bucket {
    Bucket::Working
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionListItem {
    pub id: String,
    pub parent_id: Option<String>,
    pub machine_id: String,
    pub working_dir: String,
    pub status: SessionStatus,
    /// Heartbeat-age liveness tier driving the status dot (green/orange/none).
    /// Defaults to `Dead` for back-compat with any client that omits it.
    #[serde(default = "default_liveness")]
    pub liveness: Liveness,
    /// What the session is waiting on, if anything (the ✋ "needs input"
    /// glyph). `None` when the session needs no attention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<Attention>,
    /// Classifier bucket this session falls in (Working / Needs input /
    /// Ready for review / Completed). Drives the grouped session list in
    /// both clients (CCT-90). Defaults to `Working` for back-compat.
    #[serde(default = "default_bucket")]
    pub bucket: Bucket,
    pub uptime_secs: i64,
    pub token_usage: TokenUsage,
    pub metadata: serde_json::Value,
    /// Adapter that produced this session. Defaults to `"claude-code"` for
    /// legacy rows that pre-date the `sessions.adapter_id` column.
    #[serde(default)]
    pub adapter_id: Option<AdapterId>,
    /// Machine name (resolved from `machine_id`). `None` if the machine row
    /// has been deleted but historical sessions still reference it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_name: Option<String>,
    /// Operator-set badge hue for the machine (0-359, CCT-222). `None` =
    /// client derives the hue from the machine name hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_hue: Option<i16>,
    /// Machine kind (resolved from `machine_id`, CCT-231): `"persistent"`
    /// for enrolled daemons, `"dispatch"`/`"ephemeral"` for server-managed
    /// dispatch workers. Lets clients group dispatched sessions separately.
    /// `None` when the machine row is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_kind: Option<String>,
    /// Last message text seen on this session, truncated to ~120 chars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_text: Option<String>,
    /// Timestamp of the last message event for this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Timestamp the conversation was first registered (CCT-270). Surfaced so
    /// clients can show the ISO start datetime in the relative-time tooltip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<chrono::DateTime<chrono::Utc>>,
    /// User-defined session name, when set (falls back to id in the UI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Model the session runs on (e.g. `"opus[1m]"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Reasoning/effort level (e.g. `"low"`, `"high"`), when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Whether cctui-side auto-approve is on for this session (CCT-151).
    /// In-memory server state, reflected so clients can show the toggle.
    #[serde(default)]
    pub auto_approve: bool,
    /// Transcript snippet around a keyword match (CCT-184). Only populated by
    /// the search endpoint to show *why* a session matched; `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_snippet: Option<String>,
    /// Cold-cache surfacing (CCT-189). Timestamp of the most recent
    /// assistant turn (the last `session_token_usage` row). Lets the client
    /// predict prompt-cache expiry — Anthropic's cache is a ~5-minute sliding
    /// window — before the next send, independent of `cache_cold` (which is
    /// only known *after* a turn). `None` when no usage has been recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<chrono::DateTime<chrono::Utc>>,
    /// *Confirmed* cold cache (CCT-189): the most recent assistant turn
    /// re-billed the full context (`cache_creation_tokens > 0` and
    /// `cache_read_tokens == 0`), i.e. the prompt cache had gone cold and that
    /// turn paid to rewrite it. Drives the ❄️ glyph on the session list.
    #[serde(default)]
    pub cache_cold: bool,
    /// Approximate number of tokens that get re-written to cache on the next
    /// send when the cache is cold (CCT-189) — the cached-context size from
    /// the last turn (`cache_read_tokens + cache_creation_tokens`). A rough
    /// estimate, shown on the composer's burst-cost indicator. `None` when no
    /// usage has been recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_burst_tokens: Option<u64>,
    /// Hibernated (CCT-228): the worker process has exited but its job state
    /// survives on disk, so a reply revives it (daemon resume-on-reply).
    /// Derived from the adapter's final `tempo:"hibernated"` Status. Drives
    /// the claude-style red "exited, will resume on reply" dot.
    #[serde(default)]
    pub hibernated: bool,
    /// Pinned/starred (CCT-267): the operator pinned this session so it sorts
    /// above everything in the live list and is exempt from the auto-archive
    /// reaper regardless of heartbeat age. DB-backed (`sessions.pinned`).
    #[serde(default)]
    pub pinned: bool,
    /// User-defined colored labels attached to this session (CCT-360).
    /// Many-to-many (`labels` / `session_labels` tables); empty when unlabeled.
    #[serde(default)]
    pub labels: Vec<Label>,
    /// Last activity timestamp from `sessions.last_heartbeat` (CCT-365). Bumped
    /// per real work event, and — since CCT-366 — also by subagent activity up
    /// the `parent_id` chain. Surfaced so clients can derive a long-horizon
    /// "stale" display signal (Working session with no activity for >30min)
    /// purely from the clock, the same way liveness tiers are time-derived.
    /// Distinct from `last_activity_at`, which is the last *assistant turn*
    /// (token-usage row) used for cache-expiry prediction. `None` only on stub
    /// rows that never carry liveness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat: Option<chrono::DateTime<chrono::Utc>>,
}

/// A reusable, user-defined colored label (CCT-360). Labels are global (shared
/// across sessions) and attached many-to-many; `color` is a CSS hex string
/// (e.g. `"#e11d48"`) chosen via the label picker's swatches/color input.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Label {
    pub id: String,
    pub name: String,
    pub color: String,
}

/// Body for `POST /api/v1/labels` — create (or get-or-create by name) a label.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateLabelRequest {
    pub name: String,
    pub color: String,
}

/// Body for `PATCH /api/v1/labels/{id}` — rename and/or recolor an existing
/// label by id. Either field may be omitted to leave it unchanged.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateLabelRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Body for `POST /api/v1/sessions/{id}/labels` — attach an existing label.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AttachLabelRequest {
    pub label_id: String,
}

/// Response for `GET /api/v1/labels` — every label known to the server.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LabelListResponse {
    pub labels: Vec<Label>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionListItem>,
}

/// Aggregate session counts for the Overview page (`GET /api/v1/sessions/stats`).
///
/// Computed from full SQL aggregates + the live registry rather than the capped
/// session list, so the numbers stay correct past the list's display limit.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionStats {
    /// All sessions, including archived.
    pub total: i64,
    /// Sessions currently live in the registry (active or new).
    pub live: i64,
    /// Sessions whose classifier bucket is `Blocked` (✋ needs input).
    pub needs_input: i64,
    /// Sessions in the sticky `archived` state.
    pub archived: i64,
}

/// Token totals for one time window, mirroring the three figures the session
/// list shows (`↑in ↓out ⚡cache`). Cache-creation tokens are intentionally
/// omitted here — the Overview surfaces the same readout as the session card.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WindowTokenUsage {
    /// Non-cached prompt tokens (`input_tokens`).
    pub input: u64,
    /// Generated tokens (`output_tokens`).
    pub output: u64,
    /// Tokens served from the prompt cache (`cache_read_tokens`, the ⚡ figure).
    pub cache_read: u64,
}

/// Aggregate token usage across rolling time windows for the Overview page.
/// `today` is calendar-day (since local midnight, derived from the caller's
/// timezone offset); the others are rolling intervals back from now.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TokenUsageWindows {
    /// Last 60 minutes.
    pub hour: WindowTokenUsage,
    /// Since local midnight.
    pub today: WindowTokenUsage,
    /// Last 24 hours.
    pub day: WindowTokenUsage,
    /// Last 7 days.
    pub week: WindowTokenUsage,
    /// Last 30 days.
    pub month: WindowTokenUsage,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MessageRequest {
    pub content: String,
}

/// Body for `PATCH /api/v1/sessions/{id}` — rename a session after creation.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RenameRequest {
    pub name: String,
}

/// Body for `POST /api/v1/sessions/{id}/auto-approve` — toggle the cctui-side
/// auto-approve convenience (CCT-151).
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoApproveRequest {
    pub enabled: bool,
}

/// Body for `POST /api/v1/sessions/{id}/set-model` (CCT-303).
///
/// Changes the model and/or reasoning effort of a running session in place. At
/// least one of `model`/`effort` should be set; an empty string clears nothing
/// (the field is simply omitted from the adapter command when `None`).
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetModelRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// Body for `POST /api/v1/sessions/{id}/fork` (CCT-302).
///
/// Fork an existing conversation into a brand-new session. All fields are
/// optional overrides; omitted fields inherit from the parent (the working
/// directory is always inherited from the parent server-side, and the
/// adapter/account follow the parent too). `model`/`effort` default to the
/// parent's current values (the webui pre-fills them), so a plain fork
/// preserves the model; setting them is how "fork to change model" works for
/// claude (which has no in-place switch — CCT-303). `prompt` is an optional
/// first turn to send on the forked branch.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ForkRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiError {
    pub error: String,
}

#[derive(Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SpawnRequest {
    pub machine_id: String,
    pub working_dir: String,
    pub prompt: Option<String>,
    pub prompt_name: Option<String>,
    /// Optional session display name, launched via the adapter (claude `--name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Adapter to spawn under. Defaults to `"claude-code"` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
    /// Per-spawn permission posture (CCT-149): `yolo` skips all prompts +
    /// sandbox, `auto` auto-applies without prompts but keeps the sandbox,
    /// `ask` prompts on every action. `None` → the daemon's per-host
    /// default. See [`cctui_proto::adapter::PermissionMode`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<crate::adapter::PermissionMode>,
    /// Reasoning/effort level to launch the session with (claude `--effort`,
    /// codex `model_reasoning_effort`). Valid values differ per adapter
    /// (claude: `low`/`medium`/`high`/`xhigh`/`max`; codex:
    /// `minimal`/`low`/`medium`/`high`). `None` → the adapter's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Model family to launch under (CCT-274). Passed to claude as `--model`
    /// and to codex as `-c model="…"`. Free-form (the adapter resolves family
    /// aliases like `opus`/`sonnet`/`haiku`/`fable`); `None` → the adapter's
    /// own default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Environment secrets to inject into the worker process env at spawn time
    /// (CCT-202). Keys must match `^[A-Z_][A-Z0-9_]*$`. Carried to the runtime
    /// like a bearer capability: NEVER persisted, NEVER logged, NEVER written to
    /// the transcript/timeline. `Debug` redacts the values.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    /// Named OAuth account to run the session under (CCT-232). Resolved against
    /// the caller's own vault; the server mints a session-scoped gateway token
    /// and injects `ANTHROPIC_BASE_URL`/`ANTHROPIC_AUTH_TOKEN` (or the codex
    /// equivalents) into `env` so the worker's traffic flows through the
    /// passthrough gateway under that account. `None` → no gateway injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Provider of the selected `account` (CCT-399): `anthropic` |
    /// `anthropic-compatible` | `openai` | `openai-compatible`. Disambiguates a
    /// name shared across providers so the account drives the base URL + family
    /// unambiguously (instead of inferring the family from `adapter_id`). `None`
    /// → fall back to the adapter-derived family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl std::fmt::Debug for SpawnRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnRequest")
            .field("machine_id", &self.machine_id)
            .field("working_dir", &self.working_dir)
            .field("prompt", &self.prompt)
            .field("prompt_name", &self.prompt_name)
            .field("name", &self.name)
            .field("adapter_id", &self.adapter_id)
            .field("permission_mode", &self.permission_mode)
            .field("effort", &self.effort)
            .field("model", &self.model)
            .field("account", &self.account)
            .field("provider", &self.provider)
            .field("env", &format_args!("<{} secret(s) redacted>", self.env.len()))
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SpawnResponse {
    pub command_id: Uuid,
    pub status: String,
}

/// Response to `POST /api/v1/sessions/{id}/fork` (CCT-345). Like
/// [`SpawnResponse`] but also returns the child `session_id` the server
/// pre-minted (when the adapter supports a caller-supplied id, i.e. claude) so
/// the webui can navigate to the new conversation immediately instead of
/// waiting for the next roster poll to discover it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ForkResponse {
    pub command_id: Uuid,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Response to `POST /api/v1/sessions/{id}/files` (CCT-236, mid-chat
/// attachments).
///
/// The staged absolute paths on the session's machine, in the same order the
/// files were uploaded. The webui appends these under the reply prompt so the
/// agent reads them — the same convention as spawn-time uploads.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StageFilesResponse {
    pub paths: Vec<String>,
}

/// CCT-107: dispatcher-routed session start.
///
/// `dispatcher` selects which [`Dispatcher`] impl on the server materializes
/// the request (e.g. `"k8s_job"`). Everything else is deliberately
/// runtime-agnostic: cctui mints/dedups the session, carries `reply_url` to
/// the runtime, sets the per-flow `timeout`, and forwards `payload` verbatim.
///
/// `payload` is **opaque to cctui** — never typed or inspected here. It is
/// forwarded verbatim to the dispatcher, so the caller↔runtime contract can
/// evolve with zero cctui changes.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DispatchRequest {
    pub dispatcher: String,
    /// Optional pre-minted session id. When absent the server mints one.
    /// Doubles as the **idempotency key**: a repeat dispatch with the same
    /// id returns the existing session without launching a second runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Per-flow timeout in minutes. Sets the K8s Job `activeDeadlineSeconds`
    /// and is mirrored by the caller's own wait limit. Falls back to the
    /// runtime default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u32>,
    /// Caller resume URL (e.g. an automation `$execution.resumeUrl`). A **bearer
    /// capability** — carried to the runtime, never logged or persisted.
    /// The worker POSTs its deterministic result here (CCT-119).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_url: Option<String>,
    /// Free-form, opaque to cctui. Forwarded to the runtime as-is.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub payload: serde_json::Value,
    /// Named account to run the dispatched session under (CCT-399). When set the
    /// server mints a session-scoped gateway token bound to `(session_id,
    /// account)` and merges the gateway base-url + token into `payload.env`, so a
    /// dispatched worker routes through the passthrough gateway exactly like a
    /// machine spawn. `None` → no gateway injection (the worker's own auth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Provider of the selected `account` (CCT-399), disambiguating a shared
    /// name across providers. `None` → assume the claude-code (anthropic) family,
    /// matching the k8s claude-worker the dispatch path runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DispatchResponse {
    pub session_id: String,
    pub dispatcher: String,
    /// Opaque per-dispatcher identifier (e.g. `"jobs/claude-worker-abc-…"`).
    pub handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Dispatch outcome (CCT-207): `dispatched` (a fresh run was launched),
    /// `deduplicated` (an in-flight Job already owns the one callback the caller
    /// is waiting on), or `redispatched` (a *terminal* Job was deleted and a
    /// fresh run launched — so the caller's wait resolves on the new callback
    /// instead of parking on a Job that already ran and will never call back).
    pub status: String,
}

/// One row of the per-user archive index (all archives reachable via the
/// caller's `user_id`, across all of that user's machines).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveIndexEntry {
    pub machine_id: Uuid,
    pub project_dir: String,
    pub session_id: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub line_count: Option<i32>,
    pub uploaded_at: chrono::DateTime<chrono::Utc>,
}

/// One entry of a machine's expected-files manifest. Uploaded on agent
/// startup and every 15 min; lets the server diff "expected" vs `archive_index`
/// to compute per-session sync state (CCT-68).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub project_dir: String,
    pub session_id: String,
    pub size_bytes: i64,
    pub mtime: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestPostRequest {
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveSyncState {
    /// Server has a matching upload: `uploaded_size` >= `expected_size`.
    Synced,
    /// Manifest entry has no corresponding `archive_index` row.
    Missing,
    /// Uploaded row exists but is smaller than the local file or older than mtime.
    Stale,
}

/// One row of the per-machine / per-session sync status, computed by joining
/// `archive_manifest` (expected) with `archive_index` (uploaded).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveStatusEntry {
    pub machine_id: Uuid,
    pub project_dir: String,
    pub session_id: String,
    pub expected_size: i64,
    pub expected_mtime: chrono::DateTime<chrono::Utc>,
    pub uploaded_size: Option<i64>,
    pub uploaded_sha256: Option<String>,
    pub uploaded_at: Option<chrono::DateTime<chrono::Utc>>,
    pub state: ArchiveSyncState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveStatusResponse {
    pub entries: Vec<ArchiveStatusEntry>,
}

/// One row of the skill registry (one per skill name — last-write-wins).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIndexEntry {
    pub name: String,
    pub version: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub uploaded_by_machine: Option<Uuid>,
    pub uploaded_at: chrono::DateTime<chrono::Utc>,
    pub content_type: String,
}
