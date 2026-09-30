//! `/api/v1/accounts` — account identities, their provider credentials,
//! usage windows and share grants.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// One selectable model on a compatible-endpoint or `fireworks` provider:
/// `model` is the `--model` code, `label` the display name. Safe to return over
/// the API — model names are not secret (unlike the credential).
///
/// Pricing is per *million* tokens in USD and is account-owned data: it is what
/// a pay-per-token provider is metered against, so it lives on the row rather
/// than in a table someone has to redeploy to correct.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AccountModel {
    pub model: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "number | null", optional))]
    pub price_input_per_mtok: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "number | null", optional))]
    pub price_cached_input_per_mtok: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "number | null", optional))]
    pub price_output_per_mtok: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "number | null", optional))]
    pub context_length: Option<i64>,
}

/// API view of one provider credential under an account. Secrets (the
/// OAuth/static tokens) are deliberately absent; `base_url`/`auth_scheme` are
/// surfaced so the accounts UI can render/edit a compatible endpoint in place.
#[derive(Debug, serde::Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "AccountProvider"))]
pub struct ProviderInfo {
    pub id: Uuid,
    pub account_id: Uuid,
    /// `anthropic` | `openai` | `anthropic-compatible` | `openai-compatible`.
    pub provider: String,
    /// Provider family (generated column): `anthropic` | `openai`. At most one
    /// provider per family per account, guaranteed by construction.
    pub family: String,
    /// Models this credential offers, declared by the operator, with optional
    /// pricing. `None`/empty falls back to the harness catalog / native
    /// families. Honoured for every provider kind.
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<AccountModel>>"))]
    pub models: Option<serde_json::Value>,
    /// Per-provider logical→concrete model alias map, e.g.
    /// `{"opus": "claude-opus-4-8[1m]"}`. Resolved server-side at spawn.
    #[cfg_attr(feature = "ts", ts(type = "Record<string, string> | null"))]
    pub model_aliases: Option<serde_json::Value>,
    /// `true` for a server-synthesized (managed) provider — read-only over the
    /// API (the back-compat shim for `CCTUI_CLAUDE_LITELLM_*`).
    pub managed: bool,
    /// Compatible-endpoint base URL; NULL for native providers.
    pub base_url: Option<String>,
    /// `oauth` (native) | `bearer` | `api_key` (compatible).
    pub auth_scheme: String,
    /// Upstream account id (Codex `chatgpt_account_id`).
    pub provider_account_id: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub request_count: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub bytes_transferred: i64,
    /// Total tokens (input + output + cache) attributed to this provider across
    /// all its sessions. Joined from `session_tokens` →
    /// `session_token_usage` at read time.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total_tokens: i64,
    /// USD cost of this provider's recorded usage. For a pay-per-token
    /// (`fireworks`) row this is priced per model from the account's own
    /// catalog and is the real spend; for subscription rows it is a blended-rate
    /// estimate — a usage-weight signal, not a bill.
    pub est_cost_usd: f64,
    /// Per-provider soft limits: a validated JSONB map keyed by
    /// canonical window identity (`session` | `weekly_all` | `weekly_model:<id>`),
    /// each value `{cap_pct?, bypass_minutes?, pace_cap?}`. NULL ⇒ no soft limits configured.
    #[cfg_attr(
        feature = "ts",
        ts(as = "Option<std::collections::BTreeMap<String, crate::api::soft_limit::SoftLimit>>")
    )]
    pub soft_limits: Option<serde_json::Value>,
    /// Usage ticker `{ enabled, step_pct }`; NULL ⇒ off.
    #[cfg_attr(feature = "ts", ts(as = "Option<crate::api::gateway::UsageNotices>"))]
    pub usage_notices: Option<serde_json::Value>,
    /// Whether this credential's gauge shows in the header strip.
    pub header_pin: bool,
    /// Credential health: `true` once the gateway saw the upstream
    /// provider reject this credential, cleared on the next successful upstream
    /// call. The accounts UI shows a "reauthenticate" badge.
    pub needs_reauth: bool,
    pub last_auth_error: Option<String>,
    pub last_auth_error_at: Option<DateTime<Utc>>,
    /// Validated, allowlisted subset of harness settings applied to sessions run
    /// under this provider. Config, not secret → returned normally.
    #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown> | null"))]
    pub settings_json: Option<serde_json::Value>,
    /// Gateway request-shaping settings for this credential (fireworks:
    /// `context_length_exceeded_behavior`, session affinity, extra body keys).
    /// Distinct from `settings_json`, which is harness settings.
    #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown> | null"))]
    pub provider_settings: Option<serde_json::Value>,
    /// Per-(account, provider) gateway rate limits `{ rpm?, tpm? }`, enforced in
    /// the proxy path. NULL ⇒ no throttling.
    #[cfg_attr(feature = "ts", ts(as = "Option<crate::api::gateway::RateLimits>"))]
    pub rate_limits: Option<serde_json::Value>,
    /// Quota probe from the server's registry used to measure this
    /// compatible-endpoint credential. NULL ⇒ unmeasured (no windows, so no pace
    /// and no soft limit).
    pub usage_probe: Option<String>,
}

/// API view of an account identity: name, owner, timestamps, and its
/// provider credentials.
#[derive(Debug, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "OAuthAccount"))]
pub struct AccountInfo {
    pub id: Uuid,
    pub name: String,
    /// Owner-chosen identity glyph: one emoji grapheme, or NULL for the
    /// generated letter square.
    pub emoji: Option<String>,
    /// Owning user — admins see all accounts, so the owner matters.
    pub user_id: Uuid,
    /// Owner's name for display, joined from `users`.
    pub user_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub providers: Vec<ProviderInfo>,
    /// The owner's veto on pool membership: with this false, only the owner
    /// may enrol this account in an account pool. Grantees can still launch on
    /// it by name — they just cannot make it a silent overflow target.
    pub pool_eligible: bool,
    /// Relative plan size inside a pool aggregate (upstream reports percent,
    /// never the plan behind it). `1` = same as the other members.
    pub pool_weight: f32,
    /// Names (only) of the account's free-form extra env vars, sorted.
    /// Values stay WRITE-ONLY (encrypted, never returned) — the names let the UI
    /// show what is currently set with a replace-on-save affordance.
    pub env_names: Vec<String>,
    // NOTE: env VALUES are deliberately NOT a field here — the `env_json` blob
    // holds encrypted extra environment (possibly secrets) and is WRITE-ONLY,
    // never returned over the API, exactly like the OAuth tokens.
}

/// Provider-credential payload: the create/attach fields for one
/// provider row. Used standalone by `POST /accounts/{id}/providers` and
/// flattened into [`CreateAccount`] so the legacy one-shot account+credential
/// create keeps working.
#[derive(Debug, Default, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "CreateProvider"))]
pub struct ProviderSpec {
    /// `anthropic` | `openai` (native subscription) | `anthropic-compatible` |
    /// `openai-compatible`. Optional only when flattened into
    /// [`CreateAccount`] (identity-only create); required on the provider route.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub provider: Option<String>,
    /// OAuth refresh token (subscription providers). Optional for compatible
    /// endpoints, which store only a static credential (in `access_token`).
    /// Stored encrypted; the gateway exchanges it for access tokens on demand.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub refresh_token: Option<String>,
    /// Initial access token (subscription) OR the static credential (a compatible
    /// endpoint's bearer/api key). Stored encrypted; never read back.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub access_token: Option<String>,
    /// Optional access-token expiry (unix seconds). When absent the gateway
    /// refreshes on first use (subscription) / never refreshes (compatible).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number", optional))]
    pub expires_at: Option<i64>,
    /// Compatible-endpoint base URL, e.g. a LiteLLM/vLLM/Ollama-proxy.
    /// Required for `*-compatible` providers; ignored for native ones.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub base_url: Option<String>,
    /// Models this credential offers; empty falls back to the harness catalog.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<AccountModel>>", optional))]
    pub models: Option<Vec<AccountModel>>,
    /// Logical→concrete model alias map, e.g.
    /// `{"opus": "claude-opus-4-8[1m]"}`. Honoured for every provider.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "Record<string, string>", optional))]
    pub model_aliases: Option<std::collections::HashMap<String, String>>,
    /// Credential scheme for a compatible endpoint: `bearer` | `api_key`.
    /// Defaults to `bearer`. Native providers are always `oauth`.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub auth_scheme: Option<String>,
    /// Per-provider soft limits: a canonical-key map
    /// `{ "session": {cap_pct?, bypass_minutes?, pace_cap?}, "weekly_all": {…}, … }`.
    /// Absent ⇒ NULL (no caps). Validated before persist.
    #[serde(default)]
    #[cfg_attr(
        feature = "ts",
        ts(
            as = "Option<std::collections::BTreeMap<String, crate::api::soft_limit::SoftLimit>>",
            optional
        )
    )]
    pub soft_limits: Option<serde_json::Value>,
    /// Legacy scalar soft-limit fields, still accepted on create and
    /// folded into the `session` / `weekly_all` keys when `soft_limits` is absent.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub soft_limit_5h_pct: Option<i32>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub soft_limit_7d_pct: Option<i32>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub soft_limit_bypass_5h_minutes: Option<i32>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub soft_limit_bypass_7d_minutes: Option<i32>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub soft_limit_bypass_minutes: Option<i32>,
    /// Validated, allowlisted harness settings for this provider.
    /// Server rejects MANAGED/SYSTEM keys before persist. Returned normally.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown>", optional))]
    pub settings_json: Option<serde_json::Value>,
    /// Gateway request-shaping settings. Absent on a `fireworks` create seeds
    /// the defaults so every knob is visible and editable from the start.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown>", optional))]
    pub provider_settings: Option<serde_json::Value>,
    /// Per-(account, provider) gateway rate limits `{ rpm?, tpm? }`. Absent ⇒
    /// NULL (no throttling). Validated before persist.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<crate::api::gateway::RateLimits>", optional))]
    pub rate_limits: Option<serde_json::Value>,
    /// Quota probe from the server's registry that measures this credential.
    /// Compatible endpoints only; absent ⇒ unmeasured.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub usage_probe: Option<String>,
}

/// `POST /api/v1/accounts` payload: the identity fields, plus an
/// optionally flattened [`ProviderSpec`] — supplying `provider` creates the
/// account and its first credential in one call (the legacy shape).
#[derive(Debug, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CreateAccount {
    pub name: String,
    /// Identity glyph; one emoji grapheme. Blank/absent ⇒ the letter square.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub emoji: Option<String>,
    /// Owning user — required (and only honoured) when authenticated with the
    /// admin token, which has no user identity of its own.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub user_id: Option<Uuid>,
    /// Extra environment variables for sessions run under this account.
    /// Stored ENCRYPTED at rest and never returned over the API (write-only,
    /// like the OAuth tokens). An empty map ⇒ no override.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "Record<string, string>", optional))]
    pub env_json: Option<std::collections::HashMap<String, String>>,
    #[serde(flatten)]
    pub provider: ProviderSpec,
}

/// `PATCH /api/v1/accounts/{id}` payload: identity-level fields only.
/// `name` renames; `env_json` provided → re-encrypts and replaces (an empty map
/// clears it); absent → unchanged (write-only, never returned). The legacy
/// provider-ish fields are accepted syntactically but rejected with a pointer
/// to the provider route, so an un-migrated client gets a clear 400 instead of
/// a silent no-op.
#[derive(Debug, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UpdateAccount {
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub name: Option<String>,
    /// Identity glyph. A blank string clears it back to the letter square;
    /// absent leaves it unchanged.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub emoji: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "Record<string, string>", optional))]
    pub env_json: Option<std::collections::HashMap<String, String>>,
    /// Names to remove from the stored env without re-sending the other
    /// values: decrypt → drop names → re-encrypt, all server-side. Ignored
    /// when `env_json` is provided (replace-all wins).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string[]", optional))]
    pub env_remove: Option<Vec<String>>,
    /// Owner-only: whether grantees may enrol this account in their pools.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "boolean", optional))]
    pub pool_eligible: Option<bool>,
    /// Owner-only: the account's relative plan size in pool aggregates. Must
    /// be a finite positive number.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number", optional))]
    pub pool_weight: Option<f32>,
    // Legacy provider fields (any shape): presence ⇒ 400 pointing at
    // PATCH /accounts/{id}/providers/{provider_id}.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub base_url: Option<serde_json::Value>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub auth_scheme: Option<serde_json::Value>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub models: Option<serde_json::Value>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub model_aliases: Option<serde_json::Value>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub access_token: Option<serde_json::Value>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub soft_limits: Option<serde_json::Value>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub settings_json: Option<serde_json::Value>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub defaults: Option<serde_json::Value>,
}

/// `PATCH /api/v1/accounts/{id}/providers/{provider_id}` payload. A partial update:
/// for a non-managed compatible endpoint the operator may edit `base_url`,
/// `auth_scheme`, and rotate the static credential (`access_token`).
/// `models` / `model_aliases` / `soft_limits` / `settings_json` are editable
/// for every provider. All optional; an absent field leaves that column unchanged.
/// `base_url`/credential are never returned, so the editor re-supplies
/// `base_url` when changing it and leaves the credential blank to keep the
/// stored one.
#[derive(Debug, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UpdateProvider {
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub base_url: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub auth_scheme: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<AccountModel>>", optional))]
    pub models: Option<Vec<AccountModel>>,
    /// Replacement model alias map. Provided → replaces the stored map
    /// wholesale (an empty object clears it); absent → unchanged.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "Record<string, string>", optional))]
    pub model_aliases: Option<std::collections::HashMap<String, String>>,
    /// New static credential for a compatible endpoint; blank/absent keeps the
    /// stored one.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub access_token: Option<String>,
    /// Replacement soft-limit config: a canonical-key map
    /// `{ key: {cap_pct?, bypass_minutes?, pace_cap?} }`. Provided → replaces the whole
    /// stored map (an empty object clears it, an omitted key drops that window);
    /// absent → unchanged. Validated before persist.
    #[serde(default)]
    #[cfg_attr(
        feature = "ts",
        ts(
            as = "Option<std::collections::BTreeMap<String, crate::api::soft_limit::SoftLimit>>",
            optional
        )
    )]
    pub soft_limits: Option<serde_json::Value>,
    /// Replacement usage ticker `{ enabled?, step_pct? }`. Provided → replaces
    /// (an empty object / `enabled: false` turns it off); absent → unchanged.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<crate::api::gateway::UsageNotices>", optional))]
    pub usage_notices: Option<serde_json::Value>,
    /// Show this credential's gauge in the header strip; absent → unchanged.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "boolean", optional))]
    pub header_pin: Option<bool>,
    /// Replacement validated settings blob. Provided → replaces the
    /// stored settings wholesale (an empty object clears it); absent → unchanged.
    /// Validated against the allowlist before persist.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown>", optional))]
    pub settings_json: Option<serde_json::Value>,
    /// Replacement gateway settings object. Provided → replaces wholesale (an
    /// empty object drops back to the family defaults); absent → unchanged.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown>", optional))]
    pub provider_settings: Option<serde_json::Value>,
    /// Replacement rate-limit object `{ rpm?, tpm? }`. Provided → replaces the
    /// stored value (an empty object / zeros clear it); absent → unchanged.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<crate::api::gateway::RateLimits>", optional))]
    pub rate_limits: Option<serde_json::Value>,
    /// Replacement quota probe id. Provided → replaces it (an empty string
    /// clears it, leaving the credential unmeasured); absent → unchanged. Must
    /// name a registered probe.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub usage_probe: Option<String>,
}

/// The settings catalog as served to the webui account-settings editor.
/// Everything here comes from the embedded catalog — the webui
/// carries NO mirror of the key list, so it cannot drift from the server that
/// validates the writes. `managed`/`system` keys are omitted entirely.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SettingsCatalogResponse {
    /// Exposable (safe/care) `settings.json` keys, catalog order. Keys with a
    /// `group`/`label` are the curated boolean toggles; the rest are settable
    /// via the raw-JSON box only.
    pub keys: Vec<crate::api::settings_catalog::SettingKey>,
    /// The curated env-var allowlist (all exposable by construction).
    pub env: Vec<crate::api::settings_catalog::EnvVar>,
    /// The "Quiet defaults" preset, with its `settings` filtered to exposable
    /// keys (the server-applied MANAGED keys are not offerable per-account).
    pub preset: crate::api::settings_catalog::Preset,
}

#[derive(Debug, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct OAuthStartResponse {
    pub nonce: String,
    pub authorize_url: String,
}

#[derive(Debug, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct OAuthFinish {
    pub nonce: String,
    /// New-account name. Required unless the flow was started with an
    /// `account_id` attach target (then ignored).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub name: Option<String>,
    /// anthropic: the `code#state` pair pasted from claude.ai (the `#state`
    /// suffix is optional). Either this or `callback_url` must be present.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub code: Option<String>,
    /// openai/Codex: the full `http://localhost:1455/auth/callback?code=…&state=…`
    /// URL the user copies from the browser address bar after the redirect fails
    /// to load (the fixed redirect can't reach cctui).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub callback_url: Option<String>,
}

/// Usage windows surfaced per provider credential. `usage` mirrors
/// Anthropic's free OAuth usage payload (`five_hour`/`seven_day` utilization +
/// reset timestamps); `None` means the provider has no usage API (Codex) or the
/// credential has no active windows — the webui hides the indicator in that
/// case. `account_id` is the provider-row id (the legacy field name is the
/// API contract).
#[derive(Debug, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AccountUsage {
    pub account_id: Uuid,
    pub provider: String,
    /// Raw upstream usage JSON (passed through verbatim) or `null`.
    #[cfg_attr(
        feature = "ts",
        ts(
            type = "Record<string, { utilization?: number | null, resets_at?: string | null } | null> | null"
        )
    )]
    pub usage: Option<serde_json::Value>,
    /// Normalized, provider-agnostic usage windows: the collection the
    /// UI renders and the soft-limit evaluator gates on. Empty ⇒ no supported
    /// windows in the latest response (distinct from a fetch error).
    pub windows: Vec<UsageWindowView>,
    /// Seconds since this usage was fetched upstream (0 = just now). Lets the UI
    /// show staleness; values refresh on the slow cache TTL, not per request.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub age_secs: u64,
    /// Whether a usage-limit reset can be claimed right now (Codex reset
    /// credits, Claude `juniper_tide`); `None` when the payload has no such block.
    pub limit_reset: Option<crate::api::limit_reset::LimitResetStatus>,
    /// Every reset this credential currently offers, usable first then by
    /// soonest expiry. Empty ⇒ nothing to claim and nothing to list.
    pub limit_resets: Vec<crate::api::limit_reset::LimitResetEntry>,
    /// The upstream family's incident reading, present only when this
    /// credential's provider is degraded: a healthy or unknown upstream is not
    /// news, and a compatible endpoint never inherits a first-party incident.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub provider_status: Option<crate::api::provider_status::ProviderStatus>,
}

/// A normalized window plus its pace against the window's linear budget
/// (`None` when the window has no reset time or no known length).
#[derive(Debug, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UsageWindowView {
    #[serde(flatten)]
    pub window: crate::api::soft_limit::UsageWindow,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub pace: Option<crate::api::pace::Pace>,
}

/// One row of `GET /api/v1/accounts/usage`: a provider credential's usage plus
/// the account it hangs off, so a caller can group without a second request.
#[derive(Debug, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AccountUsageEntry {
    #[serde(flatten)]
    pub usage: AccountUsage,
    pub account: Uuid,
    pub account_name: String,
    /// The account's identity glyph, so the header can group without a second request.
    pub account_emoji: Option<String>,
    /// Whether this credential's gauge shows in the header strip.
    pub header_pin: bool,
}

/// API view of one live share grant on an account. Safe to return —
/// no secrets; just who the account is shared with and since when.
#[derive(Debug, serde::Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "AccountShareInfo"))]
pub struct ShareInfo {
    pub account_id: Uuid,
    pub user_id: Uuid,
    /// The grantee's login (`users.name`), joined for display.
    pub user_name: String,
    pub action: String,
    pub granted_at: DateTime<Utc>,
}

/// `POST /api/v1/accounts/{id}/shares` payload. `user` is the grantee,
/// accepted as either a UUID or a login (`users.name`) so an operator can grant
/// by whichever they have. `action` defaults to `use` (the only action today).
#[derive(Debug, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "AccountGrantShare"))]
pub struct GrantShare {
    pub user: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub action: Option<String>,
}
