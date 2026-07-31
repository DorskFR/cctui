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

/// What a session is allowed to spawn through the daemon's `CctuiAgent` tool.
///
/// Set on the spawn/dispatch request by whoever launches the session; the
/// session itself can never write it. Absent capability = spawning denied, so
/// every check fails closed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpawnCapability {
    /// Adapter ids the session may spawn children under. Empty = deny all.
    #[serde(default)]
    pub adapters: Vec<String>,
    /// Ceiling for a child's `budget_usd`, and the budget applied when a call
    /// names none. `None` = no dollar budget may be requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_budget_usd: Option<f64>,
    /// Total children this session may spawn over its life. `None` = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_children: Option<u32>,
}

impl SpawnCapability {
    /// Whether this capability permits `adapter`. Matching is exact against the
    /// declared list.
    #[must_use]
    pub fn allows_adapter(&self, adapter: &str) -> bool {
        self.adapters.iter().any(|a| a == adapter)
    }

    /// A capability that permits nothing is equivalent to having none.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }
}

/// Body for `POST /api/v1/daemon/sessions/{id}/spawn-child` — the server side of
/// the `CctuiAgent` tool. `{id}` is the calling (parent) session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpawnChildRequest {
    pub adapter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Reply to [`SpawnChildRequest`]: the child's pre-minted session id, which is
/// also the `local_id` the daemon waits on for the child's completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnChildResponse {
    pub session_id: String,
    /// Budget actually applied to the child after clamping to the capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
}

/// Response for `GET /api/v1/daemon/sessions/{id}/gateway-env`.
///
/// The daemon pulls this at every worker (re)launch — spawn, resume,
/// cold-resume, fork — to obtain the gateway-routing env for the session's
/// bound OAuth account from the server's durable `sessions.account_id`
/// binding, instead of relying on each launch path to carry it. `account_bound`
/// distinguishes "this session has no account, empty env is correct" from
/// "account bound but the server couldn't mint env" — the latter (`account_bound`
/// with empty `env`) is the daemon's signal to refuse the launch rather than
/// start a worker that would silently hit the default upstream and 401.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayEnvResponse {
    pub account_bound: bool,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// Deep-merged per-account `settings_json` for the session's bound
    /// account(s). Travels alongside `env` on every daemon
    /// gateway-env pull so it is re-served on spawn/resume/cold-resume/fork and
    /// survives a daemon / claude-daemon restart (the failure class).
    /// The daemon deep-merges this UNDER its managed hook settings when writing
    /// the worker's `--settings` file — the managed hooks always win (the
    /// daemon-side merge is). `None` / absent → no per-account settings;
    /// older daemons that don't read this field simply ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
    /// The user's clamped `whipStopPhrases` block: `{ mode, phrases,
    /// guidance? }`. Per-user, delivered on this pull because the bare
    /// `whip-stop-hook` subprocess has no server connection; `None` → the hook
    /// uses its compiled defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whip_phrases: Option<serde_json::Value>,
    /// The session's `CctuiAgent` spawn capability, re-served on every launch
    /// pull so it survives a daemon restart. `None` → the session gets no
    /// `CctuiAgent` tool at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_capability: Option<SpawnCapability>,
}

/// Response for `GET /api/v1/daemon/sessions/{id}/token-valid?hash=<sha256hex>`.
///
/// The daemon's low-frequency validity sweep asks whether the session token it
/// launched a TRUSTED worker with still resolves — i.e. a `session_tokens` row
/// with that hash exists, is not revoked, and joins a live `account_providers`
/// row. Only the sha256 hex of the token travels on the wire, never the token
/// itself. `valid: false` (confirmed twice) is the daemon's signal that the
/// worker will 401 at the gateway forever and needs a kill + cold-resume to
/// re-mint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenValidResponse {
    pub valid: bool,
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

// Public wire/data shape mirrored to TS bindings; the bool fields are independent
// session flags, not a state machine, so refactoring them into enums would churn the API.
#[allow(clippy::struct_excessive_bools)]
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
    /// both clients. Defaults to `Working` for back-compat.
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
    /// Operator-set badge hue for the machine (0-359). `None` =
    /// client derives the hue from the machine name hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_hue: Option<i16>,
    /// Machine kind (resolved from `machine_id`): `"persistent"`
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
    /// Timestamp the conversation was first registered. Surfaced so
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
    /// Whether cctui-side auto-approve is on for this session.
    /// In-memory server state, reflected so clients can show the toggle.
    #[serde(default)]
    pub auto_approve: bool,
    /// Transcript snippet around a keyword match. Only populated by
    /// the search endpoint to show *why* a session matched; `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_snippet: Option<String>,
    /// Cold-cache surfacing. Timestamp of the most recent
    /// assistant turn (the last `session_token_usage` row). Lets the client
    /// predict prompt-cache expiry — Anthropic's cache is a ~5-minute sliding
    /// window — before the next send, independent of `cache_cold` (which is
    /// only known *after* a turn). `None` when no usage has been recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<chrono::DateTime<chrono::Utc>>,
    /// *Confirmed* cold cache: the most recent assistant turn
    /// re-billed the full context (`cache_creation_tokens > 0` and
    /// `cache_read_tokens == 0`), i.e. the prompt cache had gone cold and that
    /// turn paid to rewrite it. Drives the ❄️ glyph on the session list.
    #[serde(default)]
    pub cache_cold: bool,
    /// Approximate number of tokens that get re-written to cache on the next
    /// send when the cache is cold — the cached-context size from
    /// the last turn (`cache_read_tokens + cache_creation_tokens`). A rough
    /// estimate, shown on the composer's burst-cost indicator. `None` when no
    /// usage has been recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_burst_tokens: Option<u64>,
    /// Hibernated: the worker process has exited but its job state
    /// survives on disk, so a reply revives it (daemon resume-on-reply).
    /// Derived from the adapter's final `tempo:"hibernated"` Status. Drives
    /// the claude-style red "exited, will resume on reply" dot.
    #[serde(default)]
    pub hibernated: bool,
    /// Pinned/starred: the operator pinned this session so it sorts
    /// above everything in the live list and is exempt from the auto-archive
    /// reaper regardless of heartbeat age. DB-backed (`sessions.pinned`).
    #[serde(default)]
    pub pinned: bool,
    /// User-defined colored labels attached to this session.
    /// Many-to-many (`labels` / `session_labels` tables); empty when unlabeled.
    #[serde(default)]
    pub labels: Vec<Label>,
    /// Last activity timestamp from `sessions.last_heartbeat`. Bumped
    /// per real work event, and — since — also by subagent activity up
    /// the `parent_id` chain. Surfaced so clients can derive a long-horizon
    /// "stale" display signal (Working session with no activity for >30min)
    /// purely from the clock, the same way liveness tiers are time-derived.
    /// Distinct from `last_activity_at`, which is the last *assistant turn*
    /// (token-usage row) used for cache-expiry prediction. `None` only on stub
    /// rows that never carry liveness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat: Option<chrono::DateTime<chrono::Utc>>,
    /// OAuth account this session runs under, resolved from the most
    /// recent non-revoked `session_tokens` row joined to `account_providers` (name from its `accounts` parent).
    /// Surfaced so clients can show which account is driving the session (key
    /// icon + name tooltip). `None` for sessions with no minted gateway token
    /// (e.g. local sessions that never routed through the cctui gateway).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
    /// Unread assistant `message` events for the calling user:
    /// messages newer than that user's `session_reads.last_seen_at` (all when
    /// never seen), capped at 99. Only the live list populates it; search and
    /// get-one default it to `0`.
    #[serde(default)]
    pub unread_count: u32,
    /// Live activity headline: the daemon's spinner text from the
    /// claude-daemon control-socket `list` snapshot (`sessions.activity`), e.g.
    /// "Central verify + cascade cleanup…". Already persisted per Status event;
    /// now surfaced on the list so a working row shows *what* it's doing without
    /// opening the conversation. `None` when the session has no headline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_detail: Option<String>,
    /// When the session (or any subagent, rolled up the `parent_id` chain like
    /// the heartbeat) last emitted a `ToolUse`. Lets clients tell a
    /// *grinding* session (fresh tool calls) from one that's *asleep* — a bare
    /// heartbeat with no tool activity for minutes — far tighter than the 30-min
    /// `last_heartbeat` staleness. `None` when no tool call has been observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Name of the most recent tool call feeding `last_tool_at`, e.g.
    /// `"Read"`, `"Edit"`. `None` when no tool call has been observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool_name: Option<String>,
    /// Running count of this session's `ToolUse` events for the current turn,
    /// reset on a new user prompt. This session's own count only —
    /// a parent's rolled-up child activity shows via `last_tool_at`, not this.
    #[serde(default)]
    pub tool_use_count: u32,
    /// Live token↔account credential binding: a non-revoked
    /// `session_tokens` row with a present `encrypted_token`. Distinct from
    /// `account_name`, which is `None` when the token's `accounts` row was
    /// deleted even though the binding still exists.
    #[serde(default)]
    pub has_token_credentials: bool,
    /// Whether the session's gateway token has actually been presented at the
    /// gateway (`session_tokens.last_used_at`). An account-bound session
    /// (`account_name` set) with this `false` is bound in the DB but its worker's
    /// traffic never reached the gateway — the "account-bound but no gateway
    /// traffic observed" warning state, i.e. it may be silently riding ambient
    /// creds. `true` for any session whose token the gateway has seen.
    #[serde(default)]
    pub account_traffic_observed: bool,
    /// Linked-PR hrefs from `sessions.children`. Drives the PR link
    /// shown on the session card / TUI line and the `Ready for review` bucket.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pr_links: Vec<String>,
}

/// A reusable, user-defined colored label.
///
/// Labels are global (shared
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
/// list shows (`↑in ↓out ⚡cache`).
///
/// Cache-creation tokens are intentionally
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
///
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

/// One time bucket of aggregate token usage for the Overview usage chart.
///
/// (.) `bucket` is the `date_trunc`'d instant (RFC3339, in the fixed
/// reporting timezone anchored by the caller's `tz_offset`, mapped back to a
/// UTC instant like `today` in [`TokenUsageWindows`]). Missing buckets are
/// zero-filled client-side, not in SQL.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageBucket {
    /// Start of the bucket as an RFC3339 UTC instant.
    pub bucket: String,
    /// Non-cached prompt tokens summed over the bucket.
    pub input: u64,
    /// Generated tokens summed over the bucket.
    pub output: u64,
    /// Prompt-cache read tokens (⚡) summed over the bucket.
    pub cache_read: u64,
    /// Prompt-cache creation tokens summed over the bucket.
    pub cache_creation: u64,
}

/// Per-model token + message totals over the reporting range.
///
/// Model attribution is session-level (`sessions.model`); per-turn model
/// accuracy is out of scope. Sessions with no recorded model bucket under
/// `"unknown"`.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModelUsage {
    /// `sessions.model`, or `"unknown"` when the session has no model recorded.
    pub model: String,
    /// Non-cached prompt tokens attributed to the model.
    pub input: u64,
    /// Generated tokens attributed to the model.
    pub output: u64,
    /// Prompt-cache read tokens attributed to the model.
    pub cache_read: u64,
    /// Assistant messages (rows) attributed to the model.
    pub messages: u64,
}

/// One hour-of-week cell of the Overview activity heatmap. Extracted
/// in the reporting timezone; cells with no activity are absent (the grid is
/// filled client-side).
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HeatmapCell {
    /// Day of week, 0=Sunday..6=Saturday (Postgres `EXTRACT(dow …)`).
    pub dow: u8,
    /// Hour of day, 0..23.
    pub hour: u8,
    /// Assistant messages in the cell.
    pub messages: u64,
    /// Generated (output) tokens in the cell.
    pub output: u64,
}

/// Overview usage analytics: tokens-over-time buckets, per-model
/// breakdown, and an activity heatmap — one endpoint, one round-trip set for
/// the whole Overview usage section.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageAnalytics {
    /// Bucket granularity: `"hour"` for short ranges, else `"day"`.
    pub granularity: String,
    /// Tokens-over-time buckets, ordered oldest→newest.
    pub buckets: Vec<UsageBucket>,
    /// Per-model breakdown, ordered by output-token volume (desc).
    pub models: Vec<ModelUsage>,
    /// Sparse hour-of-week activity cells.
    pub heatmap: Vec<HeatmapCell>,
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
/// auto-approve convenience.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoApproveRequest {
    pub enabled: bool,
}

/// Body for `POST /api/v1/sessions/{id}/set-model`.
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

/// Body for `POST /api/v1/sessions/{id}/fork`.
///
/// Fork an existing conversation into a brand-new session. All fields are
/// optional overrides; omitted fields inherit from the parent (the working
/// directory is always inherited from the parent server-side, and the
/// adapter/account follow the parent too). `model`/`effort` default to the
/// parent's current values (the webui pre-fills them), so a plain fork
/// preserves the model; setting them is how "fork to change model" works for
/// claude (which has no in-place switch). `prompt` is an optional
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
    /// Conversation-extract selector: fork only a slice of the
    /// parent's history. `None` → full-history fork. Claude-only; the
    /// server rejects it for codex sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<crate::adapter::ForkExtract>,
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
    /// Per-spawn permission posture: `yolo` skips all prompts +
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
    /// Model family to launch under. Passed to claude as `--model`
    /// and to codex as `-c model="…"`. Free-form (the adapter resolves family
    /// aliases like `opus`/`sonnet`/`haiku`/`fable`); `None` → the adapter's
    /// own default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Environment secrets to inject into the worker process env at spawn time.
    /// Keys must match `^[A-Z_][A-Z0-9_]*$`. Carried to the runtime
    /// like a bearer capability: NEVER persisted, NEVER logged, NEVER written to
    /// the transcript/timeline. `Debug` redacts the values.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    /// Named OAuth account to run the session under. Resolved against
    /// the caller's own vault; the server mints a session-scoped gateway token
    /// and injects `ANTHROPIC_BASE_URL`/`ANTHROPIC_AUTH_TOKEN` (or the codex
    /// equivalents) into `env` so the worker's traffic flows through the
    /// passthrough gateway under that account. `None` → no gateway injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Provider of the selected `account`: `anthropic` |
    /// `anthropic-compatible` | `openai` | `openai-compatible`. Disambiguates a
    /// name shared across providers so the account drives the base URL + family
    /// unambiguously (instead of inferring the family from `adapter_id`). `None`
    /// → fall back to the adapter-derived family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Explicit unbound spawn: when true the server does NOT resolve a
    /// default account for an empty `account` — the worker runs on the machine's
    /// own ambient login (no gateway env, no session token). This is distinct
    /// from an unset `account`, which auto-binds the caller's single
    /// matching-family account. Ignored when `account` names an
    /// account (a named account always binds).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_account: bool,
    /// Stage this spawn as a draft instead of dispatching it. When
    /// true the server validates + persists a `draft` session row carrying the
    /// spawn payload in `metadata.draft` and does NOT mint account env or
    /// dispatch to the daemon. A later `POST /sessions/{id}/launch` mints env
    /// fresh and dispatches the real spawn. `env` is ignored for a draft (no
    /// secrets at rest — re-entered at launch time).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub save_draft: bool,
    /// What this session may spawn through the `CctuiAgent` tool. Omitted →
    /// the session cannot spawn children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub spawn_capability: Option<SpawnCapability>,
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
            .field("no_account", &self.no_account)
            .field("env", &format_args!("<{} secret(s) redacted>", self.env.len()))
            .field("save_draft", &self.save_draft)
            .field("spawn_capability", &self.spawn_capability)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SpawnResponse {
    pub command_id: Uuid,
    pub status: String,
    /// Account the spawn bound, surfaced so the client can show which
    /// credential is in play — chiefly for an auto-bound default the user never
    /// named. `None` for an unbound spawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// Body for `POST /api/v1/sessions/{id}/launch` — promote a draft
/// session to a live spawn.
///
/// The stored draft holds prompt + config only; env
/// secrets are entered fresh here (never persisted at rest) and account gateway
/// tokens are minted at launch so they're never stale. An empty map is fine for
/// drafts that need no manual secrets.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaunchRequest {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
}

/// Response to `POST /api/v1/sessions/{id}/fork`.
///
/// Like
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

/// Response to `POST /api/v1/sessions/{id}/files` (mid-chat
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

/// dispatcher-routed session start.
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
    /// The worker POSTs its deterministic result here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_url: Option<String>,
    /// Server-side completion-webhook target: the eventual
    /// replacement for `reply_url`. When set, the SERVER (not the worker) POSTs
    /// the completion payload here once the dispatched session reaches a
    /// terminal state — INCLUDING crash cases the worker's exit trap can miss
    /// (OOM/SIGKILL, daemon never connected, connection lost past the grace
    /// window). The wire shape matches the `reply_url` contract (`task_id`,
    /// `status`, `error`/verdict) so flows migrate by swapping the URL. This is
    /// additive: `reply_url` keeps working during migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_url: Option<String>,
    /// Optional per-target HMAC secret. When set, the server signs the
    /// completion-webhook body with HMAC-SHA256 and sends the hex digest in an
    /// `X-CCTUI-Signature: sha256=<hex>` header so the receiver can verify the
    /// POST originated from cctui. Never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_secret: Option<String>,
    /// Free-form, opaque to cctui. Forwarded to the runtime as-is.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub payload: serde_json::Value,
    /// Named account to run the dispatched session under. When set the
    /// server mints a session-scoped gateway token bound to `(session_id,
    /// account)` and merges the gateway base-url + token into `payload.env`, so a
    /// dispatched worker routes through the passthrough gateway exactly like a
    /// machine spawn. `None` → no gateway injection (the worker's own auth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Provider of the selected `account`, disambiguating a shared
    /// name across providers. `None` → assume the claude-code (anthropic) family,
    /// matching the k8s claude-worker the dispatch path runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Multiple accounts to route the dispatched session through.
    /// When non-empty the server mints a session-scoped gateway token for EACH
    /// account and merges every family's env into `payload.env`, so one worker
    /// can carry `ANTHROPIC_*` and `OPENAI_*` at once (e.g. claude + codex both
    /// authenticating through the passthrough gateway). At most one account per
    /// provider family — two accounts of the same family collide on the same env
    /// keys and the dispatch is rejected. Takes precedence over the singular
    /// `account`/`provider` shortcut (and the dispatcher's bound default) when
    /// present; an empty list falls back to the single-account path unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<DispatchAccount>,
}

/// One `(account, provider)` entry in [`DispatchRequest::accounts`].
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DispatchAccount {
    /// Named account to mint a gateway token for.
    pub account: String,
    /// Provider disambiguating a name shared across providers. `None` → the
    /// anthropic family, matching the singular-account default.
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
    /// Dispatch outcome: `dispatched` (a fresh run was launched),
    /// `deduplicated` (an in-flight Job already owns the one callback the caller
    /// is waiting on), or `redispatched` (a *terminal* Job was deleted and a
    /// fresh run launched — so the caller's wait resolves on the new callback
    /// instead of parking on a Job that already ran and will never call back).
    pub status: String,
}

/// Reply to `POST /api/v1/daemon/sessions/{id}/images`: the stored
/// blob id the daemon rewrites into a `cctui-img://<image_id>` message marker.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionImageUploadResponse {
    pub image_id: String,
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
