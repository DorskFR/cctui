use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::adapter::{AdapterId, RemoveInitiator};
use crate::classifier::Bucket;
use crate::models::{Attention, Liveness, SessionEndReason, SessionStatus, TokenUsage};

// --- Daemon ↔ Server ---

/// Body for `POST /api/v1/daemon/auth`: machine key in, short-lived WS session token out.
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

/// What a session may spawn through `CctuiAgent`. Set only by the launcher;
/// absent = spawning denied.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpawnCapability {
    /// Empty = deny all.
    #[serde(default)]
    pub adapters: Vec<String>,
    /// Per-child `budget_usd` ceiling and default. `None` = no budget may be requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_budget_usd: Option<f64>,
    /// Lifetime child count. `None` = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_children: Option<u32>,
    /// Most permissive child posture. `None` = capped only by the parent's mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_permission_mode: Option<crate::adapter::PermissionMode>,
    /// Further generations allowed. `0` = none, `None` = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    /// Aggregate budget across all descendants of `tree_root`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tree_budget_usd: Option<f64>,
    /// `None` = this session is the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_root: Option<String>,
}

impl SpawnCapability {
    #[must_use]
    pub fn allows_adapter(&self, adapter: &str) -> bool {
        self.adapters.iter().any(|a| a == adapter)
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }

    /// Default for an interactive machine spawn that names no capability.
    #[must_use]
    pub fn machine_default() -> Self {
        Self {
            adapters: crate::adapter::KNOWN_ADAPTERS.iter().map(|a| (*a).to_owned()).collect(),
            max_budget_usd: Some(DEFAULT_CHILD_BUDGET_USD),
            max_children: Some(DEFAULT_MAX_CHILDREN),
            max_permission_mode: None,
            max_depth: Some(DEFAULT_MAX_DEPTH),
            max_tree_budget_usd: Some(DEFAULT_TREE_BUDGET_USD),
            tree_root: None,
        }
    }

    /// Capability handed to a child: per-child ceiling, posture and depth only
    /// shrink; tree budget and root carry over.
    #[must_use]
    pub fn inherited(
        &self,
        self_id: &str,
        child_budget: Option<f64>,
        child_mode: Option<crate::adapter::PermissionMode>,
    ) -> Self {
        let max_budget_usd = match (self.max_budget_usd, child_budget) {
            (Some(mine), Some(granted)) => Some(mine.min(granted)),
            (Some(mine), None) => Some(mine),
            (None, granted) => granted,
        };
        let max_permission_mode = match (self.max_permission_mode, child_mode) {
            (Some(mine), Some(child)) => {
                Some(crate::adapter::PermissionMode::stricter(mine, child))
            }
            (mine, child) => mine.or(child),
        };
        Self {
            adapters: self.adapters.clone(),
            max_budget_usd,
            max_children: self.max_children,
            max_permission_mode,
            max_depth: self.max_depth.map(|d| d.saturating_sub(1)),
            max_tree_budget_usd: self.max_tree_budget_usd,
            tree_root: Some(self.tree_root.clone().unwrap_or_else(|| self_id.to_owned())),
        }
    }
}

pub const DEFAULT_CHILD_BUDGET_USD: f64 = 20.0;

pub const DEFAULT_MAX_CHILDREN: u32 = 16;

/// Generations below the root.
pub const DEFAULT_MAX_DEPTH: u32 = 3;

pub const DEFAULT_TREE_BUDGET_USD: f64 = 400.0;

/// Body for `POST /api/v1/daemon/sessions/{id}/spawn-child`; `{id}` is the parent.
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
    /// Never more permissive than the parent's. `None` = the parent's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<crate::adapter::PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Body for `POST /api/v1/daemon/sessions/{id}/message-child`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageChildRequest {
    pub session_id: String,
    pub prompt: String,
}

/// `session_id` is pre-minted and is the `local_id` the daemon waits on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnChildResponse {
    pub session_id: String,
    /// Budget applied after clamping to the capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
}

/// Response for `GET /api/v1/daemon/sessions/{id}/gateway-env`, pulled at every
/// worker launch. `account_bound` with empty `env` means the daemon must refuse
/// the launch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayEnvResponse {
    pub account_bound: bool,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// Per-account `settings_json`, merged under the daemon's managed hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
    /// `{ mode, phrases, guidance? }`. `None` = hook defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whip_phrases: Option<serde_json::Value>,
    /// `None` = no `CctuiAgent` tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_capability: Option<SpawnCapability>,
    /// Runtime plugins the session owner enabled that ship skills. The daemon
    /// mirrors each one under its cache and passes it as `--plugin-dir`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<SessionPlugin>,
}

/// One enabled plugin's skill bundle, as served under
/// `/plugins/{id}/skills/{file}` for every entry of `files`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionPlugin {
    pub id: String,
    pub version: String,
    /// Content hash of the skill files; the daemon's cache key.
    pub skills_hash: String,
    /// Paths relative to the plugin's `skills/` folder.
    pub files: Vec<String>,
    /// The owner's plugin settings as `env name -> value`, exported into the
    /// agent's environment. Names are validated on both ends.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
}

/// Response for `GET /api/v1/daemon/sessions/{id}/token-valid?hash=<sha256hex>`.
/// `false` means the worker's gateway token is revoked or unbound.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenValidResponse {
    pub valid: bool,
}

/// Adapter configuration row sent in the initial `Reconcile` frame.
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
    #[serde(default = "default_liveness")]
    pub liveness: Liveness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<Attention>,
    #[serde(default = "default_bucket")]
    pub bucket: Bucket,
    pub token_usage: TokenUsage,
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub adapter_id: Option<AdapterId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_name: Option<String>,
    /// 0–359. `None` = derived from the machine name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_hue: Option<i16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub auto_approve: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_seq: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub cache_cold: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_burst_tokens: Option<u64>,
    #[serde(default)]
    pub hibernated: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
    /// Capped at 99; populated by the live list only.
    #[serde(default)]
    pub unread_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool_name: Option<String>,
    #[serde(default)]
    pub tool_use_count: u32,
    /// Empty means render nothing.
    #[serde(default)]
    pub todos: Vec<TodoEntry>,
    #[serde(default)]
    pub has_token_credentials: bool,
    #[serde(default)]
    pub account_traffic_observed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pr_links: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<SessionEndReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_archive_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_by: Option<RemoveInitiator>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keepalive: Option<KeepaliveState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_keepalive_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Cache keep-alive: one tick every `interval_secs` while idle, stopping after
/// `max_ticks` (`0` = never). Human activity resets `ticks_sent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KeepaliveState {
    pub interval_secs: u32,
    pub max_ticks: u32,
    #[serde(default)]
    pub ticks_sent: u32,
    /// `None` when indefinite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<chrono::DateTime<chrono::Utc>>,
}

/// `enabled: false` clears the schedule; omitted fields use provider defaults.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionKeepaliveRequest {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_secs: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ticks: Option<u32>,
}

/// Task-list entry. `status` is `pending`, `in_progress` or `completed`;
/// `active_form` is claude-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TodoEntry {
    pub content: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
}

/// `color` is a CSS hex string.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Label {
    pub id: String,
    pub name: String,
    pub color: String,
}

/// Get-or-create by name.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateLabelRequest {
    pub name: String,
    pub color: String,
}

/// Omitted fields are unchanged.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateLabelRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AttachLabelRequest {
    pub label_id: String,
}

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

/// Session counts from SQL aggregates, not the capped list.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionStats {
    /// Includes archived.
    pub total: i64,
    pub live: i64,
    pub needs_input: i64,
    pub archived: i64,
    /// Registered in local calendar periods, including archived.
    pub today: i64,
    pub yesterday: i64,
    /// Since Monday, local.
    pub week: i64,
    pub month: i64,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WindowTokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
}

/// `today` is since local midnight; the rest are rolling.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TokenUsageWindows {
    pub hour: WindowTokenUsage,
    pub today: WindowTokenUsage,
    pub day: WindowTokenUsage,
    pub week: WindowTokenUsage,
    pub month: WindowTokenUsage,
}

/// Missing buckets are zero-filled client-side.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageBucket {
    /// Bucket start, RFC3339 UTC.
    pub bucket: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

/// Attributed by session model, not per turn.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModelUsage {
    /// `"unknown"` when unrecorded.
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    /// Assistant messages.
    pub messages: u64,
}

/// Empty cells are omitted.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HeatmapCell {
    /// 0 = Sunday.
    pub dow: u8,
    pub hour: u8,
    pub messages: u64,
    pub output: u64,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageAnalytics {
    /// `hour` or `day`.
    pub granularity: String,
    /// Oldest first.
    pub buckets: Vec<UsageBucket>,
    /// By output tokens, descending.
    pub models: Vec<ModelUsage>,
    pub heatmap: Vec<HeatmapCell>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MessageRequest {
    pub content: String,
    /// Client-minted `UUIDv7` echoed on every event of the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<uuid::Uuid>,
    /// RFC3339, at most 30 days ahead; the request is queued and returns 202.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliver_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RenameRequest {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoApproveRequest {
    pub enabled: bool,
}

/// `None` fields are left unchanged.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetModelRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// Omitted fields inherit from the parent; working dir, adapter and account always do.
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
    /// Fork only a slice of history. Claude only.
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
// `no_account` / `auto_account` / `save_draft` / `auto_archive` are independent
// wire flags, each defaulting to false; an enum would change the JSON shape.
#[allow(clippy::struct_excessive_bools)]
pub struct SpawnRequest {
    pub machine_id: String,
    pub working_dir: String,
    pub prompt: Option<String>,
    pub prompt_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Defaults to `claude-code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
    /// `None` = the daemon's per-host default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<crate::adapter::PermissionMode>,
    /// Adapter-specific level. `None` = adapter default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Model or family alias. `None` = adapter default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Codex only: `fast` or `default`. `None` = the account's setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// Keys match `^[A-Z_][A-Z0-9_]*$`. Never persisted or logged.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    /// Account from the caller's vault to route through the gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Disambiguates `account` across providers. `None` = derived from the adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Run on the machine's ambient login. Ignored when `account` is set.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_account: bool,
    /// Pick the matching account with the most allocation left instead of
    /// rejecting an ambiguous choice. Ignored when `account` or `no_account` is set.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_account: bool,
    /// Pick among this pool's members only. Ignored when `account` or `no_account` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
    /// Persist as a draft without dispatching; `env` is not stored.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub save_draft: bool,
    /// Archive once the first turn ends cleanly.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_archive: bool,
    /// Draft only: env var names, values are never stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_keys: Vec<String>,
    /// Draft only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_names: Vec<String>,
    /// Attached when the worker registers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub label_ids: Vec<String>,
    /// `None` = cannot spawn children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub spawn_capability: Option<SpawnCapability>,
    /// `followup`: the first prompt embeds the parent's brief. `None` = root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
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
            .field("service_tier", &self.service_tier)
            .field("account", &self.account)
            .field("provider", &self.provider)
            .field("no_account", &self.no_account)
            .field("auto_account", &self.auto_account)
            .field("pool", &self.pool)
            .field("env", &format_args!("<{} secret(s) redacted>", self.env.len()))
            .field("save_draft", &self.save_draft)
            .field("auto_archive", &self.auto_archive)
            .field("env_keys", &self.env_keys)
            .field("attachment_names", &self.attachment_names)
            .field("label_ids", &self.label_ids)
            .field("spawn_capability", &self.spawn_capability)
            .field("relation", &self.relation)
            .field("parent_session_id", &self.parent_session_id)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SpawnResponse {
    pub command_id: Uuid,
    pub status: String,
    /// `None` when unbound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Pre-minted id, when the adapter accepts one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
}

/// Body for `POST /api/v1/sessions/{id}/launch`; env is entered fresh, never stored.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaunchRequest {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
}

/// `session_id` is set when the adapter accepts a pre-minted id.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ForkResponse {
    pub command_id: Uuid,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// User/assistant transcript as markdown, oldest turns elided past the caps.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BriefResponse {
    pub markdown: String,
    pub turns: u32,
    pub omitted: u32,
    pub truncated: bool,
}

/// Absolute paths on the session's machine, in upload order.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StageFilesResponse {
    pub paths: Vec<String>,
}

/// Dispatcher-routed session start. `payload` is opaque and forwarded verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DispatchRequest {
    pub dispatcher: String,
    /// Idempotency key; minted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Minutes. `None` = runtime default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u32>,
    /// Bearer capability the worker posts its result to. Never logged or persisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_url: Option<String>,
    /// The server posts the completion payload here on any terminal state, crashes included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_url: Option<String>,
    /// Signs the webhook body: `X-CCTUI-Signature: sha256=<hex>`. Never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_secret: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub payload: serde_json::Value,
    /// Account whose gateway env is merged into `payload.env`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// `None` = anthropic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// At most one per provider family. Takes precedence over `account`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<DispatchAccount>,
}

/// One `(account, provider)` entry in [`DispatchRequest::accounts`].
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DispatchAccount {
    pub account: String,
    /// `None` = anthropic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DispatchResponse {
    pub session_id: String,
    pub dispatcher: String,
    /// Opaque dispatcher handle.
    pub handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// `dispatched`, `deduplicated` or `redispatched`.
    pub status: String,
}

/// `image_id` is referenced as `cctui-img://<image_id>`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionImageUploadResponse {
    pub image_id: String,
}

/// One per skill name, last write wins.
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
