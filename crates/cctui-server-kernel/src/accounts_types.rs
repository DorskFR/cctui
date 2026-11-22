//! Account types and host contexts shared with the dispatch and session domains.
use std::sync::Arc;

use axum::http::StatusCode;
use cctui_proto::adapter::SessionSpec;
use cctui_proto::api::account_redirects::AccountRedirect;
use cctui_proto::api::soft_limit::{SoftLimits, UsageWindow};
use cctui_proto::hook_policy::{
    AddOnlyError, DISPATCH_ADDITIONS_FIELD, HookPolicy, HookPolicyStatus, HookRule,
    SessionHookPolicy, effective_rules, no_direct_harness_rule, normalize_rules,
    supports_hook_policy,
};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;

pub mod pick;

/// Anthropic Claude-Code OAuth token endpoint + client id.
///
/// These are not stable public APIs (caveat accepted in the ticket); overridable via env
/// so we can track upstream changes without a redeploy of code.
#[must_use]
pub fn anthropic_token_url() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_TOKEN_URL")
        .unwrap_or_else(|_| "https://console.anthropic.com/v1/oauth/token".into())
}

/// OpenAI/Codex OAuth token endpoint.
///
/// Codex's public client exchanges + refreshes here with **form-encoded** bodies (unlike
/// Anthropic's JSON). Overridable via env to track upstream changes without a code
/// redeploy.
#[must_use]
pub fn openai_token_url() -> String {
    std::env::var("CCTUI_OPENAI_OAUTH_TOKEN_URL")
        .unwrap_or_else(|_| "https://auth.openai.com/oauth/token".into())
}

/// The provider *family* of an account: which env vars it drives, and the key `UNIQUE
/// (account_id, family)` enforces one credential per.
///
/// `fireworks` is its own family — despite the `OpenAI` wire protocol — so a Fireworks
/// key can sit next to a codex credential on one account.
///
/// [`label`](Self::label) is the stored value of the generated `family` column;
/// per-family SQL predicates compare against it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    Anthropic,
    Openai,
    Fireworks,
    Google,
}

/// An adapter id the harness table does not know.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UnknownHarness(pub String);

impl std::fmt::Display for UnknownHarness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown harness {:?}", self.0)
    }
}

impl std::error::Error for UnknownHarness {}

impl Family {
    /// Derive the family from a stored `provider` value. Must agree with the
    /// generated `family` column (migration 177).
    #[must_use]
    pub fn from_provider(provider: &str) -> Self {
        if provider == "fireworks" {
            Self::Fireworks
        } else if provider == "google" {
            Self::Google
        } else if provider.contains("openai") {
            Self::Openai
        } else {
            Self::Anthropic
        }
    }
    /// Parse a family label back (the `family` column / API `family` field).
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::Openai),
            "fireworks" => Some(Self::Fireworks),
            "google" => Some(Self::Google),
            _ => None,
        }
    }
    /// Pay-per-token families: usage is metered from the responses the gateway
    /// proxies and priced from the account's catalog.
    #[must_use]
    pub const fn is_metered(self) -> bool {
        matches!(self, Self::Fireworks | Self::Google)
    }
    #[must_use]
    pub const fn from_provider_family(family: cctui_proto::provider::ProviderFamily) -> Self {
        match family {
            cctui_proto::provider::ProviderFamily::Anthropic => Self::Anthropic,
            cctui_proto::provider::ProviderFamily::Openai => Self::Openai,
            cctui_proto::provider::ProviderFamily::Fireworks => Self::Fireworks,
            cctui_proto::provider::ProviderFamily::Google => Self::Google,
        }
    }
    /// Derive the family from a spawn adapter id through the harness table.
    ///
    /// This IS the spawn resolution key: the adapter names the harness family,
    /// and the account identity carries at most one provider row per family.
    ///
    /// Fail-closed: an adapter id that names no known harness yields `None`
    /// rather than silently binding an Anthropic credential to it.
    #[must_use]
    pub fn try_from_adapter(adapter_id: &str) -> Option<Self> {
        cctui_proto::adapter::harness_for_adapter(adapter_id)
            .map(|h| Self::from_provider_family(h.family))
    }

    /// [`try_from_adapter`](Self::try_from_adapter) as an error a request
    /// handler can return: a gateway bind for an unknown harness never launches.
    pub fn from_adapter(adapter_id: &str) -> Result<Self, UnknownHarness> {
        Self::try_from_adapter(adapter_id)
            .ok_or_else(|| UnknownHarness(adapter_id.trim().to_owned()))
    }
    /// Human label for error messages, and the stored `family` column value.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Fireworks => "fireworks",
            Self::Google => "google",
        }
    }
}

/// Execution resources needed to resolve and rotate provider credentials.
///
/// The host supplies its startup-resolved vault key lazily, preserving both
/// installed legacy keys and static-credential paths that need no decryption.
pub trait CredentialContext: Sync {
    fn pool(&self) -> &sqlx::PgPool;
    fn http_client(&self) -> &reqwest::Client;
    fn account_locks(&self) -> &DashMap<Uuid, Arc<tokio::sync::Mutex<()>>>;
    fn account_reauth(&self) -> &DashMap<Uuid, ()>;
    fn vault_key(&self) -> Vec<u8>;

    /// Off, an account service token resolves exactly like an unknown token.
    fn service_tokens(&self) -> bool {
        true
    }

    fn anthropic_token_url(&self) -> String {
        anthropic_token_url()
    }
    fn openai_token_url(&self) -> String {
        openai_token_url()
    }
}

/// A fetched payload, including an upstream response with no usage windows.
#[derive(Clone)]
pub struct CachedUsage {
    pub fetched_at: std::time::Instant,
    pub usage: Option<serde_json::Value>,
}

pub type AccountUsageCache = std::sync::Arc<dashmap::DashMap<Uuid, CachedUsage>>;

pub const USAGE_CACHE_TTL: chrono::Duration = chrono::Duration::minutes(3);

/// Execution resources and an awaited notification after a fresh quota reading.
pub trait UsageContext: CredentialContext {
    fn usage_cache(&self) -> &dashmap::DashMap<Uuid, CachedUsage>;
    fn probe_client(&self) -> &reqwest::Client;
    fn probe_url_permitted(&self, url: &str) -> Result<(), String>;
    fn usage_updated(
        &self,
        provider_id: Uuid,
        provider: String,
        usage: Option<&serde_json::Value>,
    ) -> impl std::future::Future<Output = ()> + Send;

    fn anthropic_usage_url(&self) -> String {
        anthropic_usage_url()
    }
    fn anthropic_reset_status_url(&self) -> String {
        anthropic_reset_status_url()
    }
    fn openai_usage_url(&self) -> String {
        openai_usage_url()
    }
    fn openai_reset_credits_url(&self) -> String {
        openai_reset_credits_url()
    }
}

/// Anthropic's free OAuth usage endpoint.
///
/// Returns subscription window utilization (5h session + 7d weekly) WITHOUT consuming any tokens —
/// it is not an inference call. Undocumented + caveat-accepted (same class as the OAuth token
/// endpoints above); overridable via env to track upstream changes.
#[must_use]
pub fn anthropic_usage_url() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_USAGE_URL")
        .unwrap_or_else(|_| "https://api.anthropic.com/api/oauth/usage".into())
}

/// The same usage endpoint, asked for the limit-reset programs: upstream only populates
/// `juniper_tide`/`cedar_ember` when a read flag is on the query string.
///
/// `skip_spend=1` drops the `spend` block, so this must stay a separate call from the usage poll.
/// Read-only `GET`: it never claims anything.
#[must_use]
pub fn anthropic_reset_status_url() -> String {
    std::env::var("CCTUI_ANTHROPIC_RESET_STATUS_URL").unwrap_or_else(|_| {
        let usage = anthropic_usage_url();
        let sep = if usage.contains('?') { '&' } else { '?' };
        format!("{usage}{sep}cedar_ember=1&skip_spend=1")
    })
}

/// OpenAI/codex's real per-account usage endpoint.
///
/// Returns the `ChatGPT` backend's actual 5h/7d rate-limit windows (`rate_limit.primary_window` /
/// `secondary_window`) — the same numbers `codex /status` shows. Works cookieless with just the
/// account's OAuth Bearer + `chatgpt-account-id` header, both of which we already hold per account.
/// Overridable via env to track upstream moves.
#[must_use]
pub fn openai_usage_url() -> String {
    std::env::var("CCTUI_OPENAI_USAGE_URL")
        .unwrap_or_else(|_| "https://chatgpt.com/backend-api/wham/usage".into())
}

/// Codex's "Redeem usage limit reset" credits (`GET`), consumed via
/// `POST {url}/consume {redeem_request_id, credit_id?}`.
#[must_use]
pub fn openai_reset_credits_url() -> String {
    std::env::var("CCTUI_OPENAI_RESET_CREDITS_URL")
        .unwrap_or_else(|_| "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits".into())
}

/// Resources required to mint and restore execution credentials.
///
/// The host supplies its installed vault key and clears any transient orphan
/// block when an existing credential is rebound. Account share policy is
/// enforced here, including the settings and environment allowlists.
pub trait MintContext: Sync {
    fn pool(&self) -> &sqlx::PgPool;
    fn vault_key(&self) -> Vec<u8>;
    fn external_url(&self) -> &str;
    fn clear_orphan_fingerprint(&self, fingerprint: &str);
}

/// Why session credential minting failed.
///
/// Why [`mint_session_env`] could not mint, kept apart so callers can 404 with
/// a message naming the actual gap: "no such account" and "account
/// exists but carries no provider for this harness family" need different
/// remedies (fix the name vs. connect a provider).
#[derive(Debug)]
pub enum MintSessionEnvError {
    /// The account name resolved to nothing for this user (neither owned nor
    /// shared).
    NoAccount,
    /// The account identity exists but has no provider row in the requested
    /// family.
    NoProviderForFamily(Family),
    /// The database failed.
    Db(sqlx::Error),
}

/// One provider row of a resolved account identity: the credential-level
/// `account_providers` row the gateway binds session tokens to. At most one
/// per family per account.
pub struct ProviderRow {
    pub id: Uuid,
    pub provider: String,
    /// Per-provider logical→concrete model alias map.
    pub model_aliases: Option<serde_json::Value>,
    /// Account-owned model catalog (`[{model, label, pricing…}]`). The only
    /// source of concrete model ids for a `fireworks` provider — nothing in a
    /// worker image, entrypoint, or dispatch payload hardcodes one.
    pub models: Option<serde_json::Value>,
}

impl ProviderRow {
    #[must_use]
    pub fn family(&self) -> Family {
        Family::from_provider(&self.provider)
    }
}

/// Dependencies supplied by a server host, without workspace state or routes.
/// Usage reads retain the host's refresh, cache and provider error handling.
pub trait AccountResolutionContext: Sync {
    fn pool(&self) -> &PgPool;
    fn cached_needs_reauth(&self, provider: Uuid) -> bool;
    fn usage_for_soft_limit(
        &self,
        provider: Uuid,
    ) -> impl std::future::Future<Output = Option<serde_json::Value>> + Send;
}

/// The active gateway window also used by clients' session liveness display.
pub const LIVENESS_ACTIVE_SECS: i64 = 5 * 60;

/// Why a name could not be bound.
#[derive(Debug)]
pub enum ResolveError {
    /// The request cannot be served as asked — a 400 with a caller-readable
    /// reason (which accounts were considered and why each was skipped).
    Rejected(String),
    Db,
}

/// One account the spawn could bind to, as the ranker sees it.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Account name — what a spawn binds and what the error/toast shows.
    pub name: String,
    /// Normalized usage windows for the credential that will actually serve.
    /// Empty when usage could not be read; see `usage_known`.
    pub windows: Vec<UsageWindow>,
    /// The account's configured caps.
    pub limits: SoftLimits,
    /// Whether `windows` reflects a real reading. An account whose usage
    /// endpoint failed has no windows, which must NOT be read as "wide open"
    /// nor as "exhausted" — only as unknown.
    pub usage_known: bool,
    /// Sessions already bound to (and served by) this account that are still
    /// live, or were bound too recently to show up in its usage yet. Counted
    /// by the caller; see [`RESERVATION`].
    pub in_flight: u32,
    /// The serving credential was refused upstream (`account_providers.needs_reauth`) and
    /// must be signed in again.
    ///
    /// Such an account is out of every election, whatever its usage says: re-electing it
    /// would only replay the 401.
    pub needs_reauth: bool,
}

/// Why one candidate is out of the running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    pub name: String,
    /// Human-readable cause, naming the window and its reset.
    pub reason: String,
}

/// The outcome of ranking.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// Bind this account.
    ///
    /// `headroom_pct` is the raw margin of the window that decided its score, `score` the
    /// sustainable rate it was ranked on and `resets_at` that window's reset. All `None`
    /// when the pick was made without usage data.
    Chosen {
        name: String,
        headroom_pct: Option<f64>,
        score: Option<f64>,
        resets_at: Option<DateTime<Utc>>,
    },
    /// Every candidate was readable AND out of allocation, or must be
    /// reconnected before it can serve anything.
    Exhausted(Vec<Blocked>),
    /// No candidate at all.
    None,
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Rejected(String),
}

impl From<PolicyError> for AppError {
    fn from(error: PolicyError) -> Self {
        match error {
            PolicyError::Db(error) => Self::Db(error),
            PolicyError::Rejected(reason) => Self::new(StatusCode::BAD_REQUEST, reason),
        }
    }
}

impl From<AddOnlyError> for PolicyError {
    fn from(error: AddOnlyError) -> Self {
        Self::Rejected(error.to_string())
    }
}

/// The account to bind, plus the pool it must stay inside (if any).
#[derive(Debug, PartialEq, Eq)]
pub struct Bound {
    pub account: String,
    pub pool_id: Option<Uuid>,
}

/// Loaded account row (decrypted in-process; never serialized out).
pub struct Account {
    pub id: Uuid,
    pub provider: String,
    pub access_token: Option<String>,
    /// `None` for compatible endpoints (static credential, no refresh).
    pub refresh_token: Option<String>,
    pub expires_at: Option<chrono::DateTime<Utc>>,
    /// For Codex/OpenAI accounts: the `chatgpt_account_id` claim, sent upstream
    /// as the `Chatgpt-Account-Id` header. NULL for anthropic / manual
    /// refresh-token accounts.
    pub provider_account_id: Option<String>,
    /// Compatible-endpoint upstream base URL. NULL → built-in upstream.
    pub base_url: Option<String>,
    /// `oauth` (refreshing subscription) | `bearer` | `api_key` (static).
    pub auth_scheme: String,
    /// Per-account soft limits on cctui's own share of the usage windows.
    /// Enforced in `passthrough` against the cached usage. All NULL ⇒
    /// no soft limit (prior behaviour).
    pub soft_limits: cctui_proto::api::soft_limit::SoftLimits,
    /// Per-provider usage ticker setting; off ⇒ the proxy skips it.
    pub usage_notices: cctui_proto::api::gateway::UsageNotices,
    /// Per-provider gateway request-shaping settings; see `FireworksSettings`.
    pub provider_settings: Option<serde_json::Value>,
    /// Per-(account, provider) RPM/TPM ceilings enforced in the proxy path.
    /// Unset ⇒ no throttling.
    pub rate_limits: cctui_proto::api::gateway::RateLimits,
}

impl Account {
    /// A static-credential compatible account forwards its stored credential
    /// verbatim and skips the OAuth refresh round-trip.
    #[must_use]
    pub fn is_static(&self) -> bool {
        self.auth_scheme != "oauth"
    }
}

/// Merge a `CctuiAgent` child's per-session dollar budget into `cap` as a
/// `session_usd` limit.
///
/// A budget on the child always wins over an account-level
/// `session_usd`: it is the tighter, purpose-set ceiling.
#[must_use]
pub fn merge_session_budget(
    cap: &cctui_proto::api::soft_limit::SoftLimits,
    budget_usd: Option<f64>,
) -> cctui_proto::api::soft_limit::SoftLimits {
    let Some(budget) = budget_usd.filter(|b| b.is_finite() && *b > 0.0) else {
        return cap.clone();
    };
    let mut merged = cap.clone();
    let entry =
        merged.limits.entry(cctui_proto::api::soft_limit::KEY_SESSION_USD.to_owned()).or_default();
    entry.cap_usd = Some(budget);
    merged
}

/// Whether the soft-limit check must refresh usage from upstream before deciding.
///
/// A cold (`None`) or stale cache must trigger a refresh — treating a cold
/// cache as "no data → allow" would let a capped account run to 100% whenever
/// no human was viewing it.
#[must_use]
pub fn usage_cache_stale(entry_age: Option<std::time::Duration>, ttl: std::time::Duration) -> bool {
    entry_age.is_none_or(|age| age >= ttl)
}

/// Map `model` through one provider row's aliases, then its catalog.
///
/// `catalog_fallback` lets a `fireworks` catalog miss resolve to the first
/// entry, as a spawn needs; without it only an exact id or label match counts.
#[must_use]
pub fn resolve_row_model(row: &ProviderRow, model: &str, catalog_fallback: bool) -> String {
    let aliased = row
        .model_aliases
        .as_ref()
        .and_then(|v| v.get(model))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map_or_else(|| model.to_owned(), str::to_owned);
    if row.family() != Family::Fireworks {
        return aliased;
    }
    let resolved = if catalog_fallback {
        resolve_catalog_model(row.models.as_ref(), &aliased)
    } else {
        resolve_exact_catalog_model(row.models.as_ref(), &aliased)
    };
    resolved.unwrap_or(aliased)
}

/// Resolve a model ID or label against an account catalog.
///
/// Resolve a model against an account's catalog: an exact `model` id passes
/// through, a catalog `label` maps to its id, and anything else (including an
/// empty request) falls back to the first catalog entry. `None` when the catalog
/// is empty — the caller then keeps the requested model verbatim.
#[must_use]
pub fn resolve_catalog_model(models: Option<&serde_json::Value>, model: &str) -> Option<String> {
    resolve_exact_catalog_model(models, model)
        .or_else(|| models?.as_array()?.iter().find_map(catalog_entry_id))
}

/// [`resolve_catalog_model`] without the first-entry fallback: only an exact
/// catalog id or label resolves.
#[must_use]
pub fn resolve_exact_catalog_model(
    models: Option<&serde_json::Value>,
    model: &str,
) -> Option<String> {
    let wanted = model.trim();
    if wanted.is_empty() {
        return None;
    }
    models?
        .as_array()?
        .iter()
        .find(|e| {
            catalog_entry_id(e).as_deref() == Some(wanted)
                || e.get("label").and_then(serde_json::Value::as_str) == Some(wanted)
        })
        .and_then(catalog_entry_id)
}

fn catalog_entry_id(entry: &serde_json::Value) -> Option<String> {
    entry.get("model").and_then(serde_json::Value::as_str).map(str::to_owned)
}

/// The side-door rule, then `rules`, as the launch of `adapter` carries them.
///
/// # Errors
/// The union is over the limits: the launch must not go out without it.
pub fn carry(adapter: &str, rules: Vec<HookRule>) -> Result<SessionHookPolicy, String> {
    let mut all = vec![no_direct_harness_rule()];
    all.extend(rules);
    let rules = normalize_rules(all)?;
    let status = if supports_hook_policy(adapter) {
        HookPolicyStatus::Applied
    } else {
        HookPolicyStatus::Unsupported
    };
    Ok(SessionHookPolicy { rules, status: Some(status) })
}

/// Add the account rules to what the spec already carries.
///
/// # Errors
/// See [`carry`].
pub fn attach(spec: &mut SessionSpec, rules: Vec<HookRule>) -> Result<SessionHookPolicy, String> {
    let mut all = spec.hook_policy.take().map(|p| p.rules).unwrap_or_default();
    all.extend(rules);
    let carried = carry(spec.adapter_id.as_str(), all)?;
    spec.hook_policy = Some(HookPolicy { rules: carried.rules.clone() });
    Ok(carried)
}

/// Parse and check a dispatch payload's `hook_rules` additions.
///
/// They are checked against the side-door rule every worker inherits, so a
/// dispatch weakening it is refused before anything is minted for the worker.
///
/// # Errors
/// [`PolicyError::Rejected`] for unreadable or weakening additions.
pub fn dispatch_additions(payload: &serde_json::Value) -> Result<Vec<HookRule>, PolicyError> {
    let additions = match payload.get(DISPATCH_ADDITIONS_FIELD) {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(raw) => serde_json::from_value::<Vec<HookRule>>(raw.clone())
            .map_err(|e| PolicyError::Rejected(format!("invalid payload.hook_rules: {e}")))?,
    };
    effective_rules(&[no_direct_harness_rule()], additions.clone())?;
    Ok(additions)
}

/// Follow account-level rules (`to_account` set, `match_model` NULL) from `from`
/// transitively.
///
/// The visited set ends a cycle at its last new hop and the cap bounds pathological chains;
/// both degrade to "stop here", never an error — a broken rule must not fail a launch. `None`
/// when no rule moves `from` at all.
#[must_use]
pub fn follow_account_chain(rules: &[AccountRedirect], from: Uuid, family: &str) -> Option<Uuid> {
    let mut visited = vec![from];
    let mut cur = from;
    for _ in 0..4 {
        let Some(rule) = rules
            .iter()
            .find(|r| r.from_account == cur && r.family == family && r.to_account.is_some())
        else {
            break;
        };
        let next = rule.to_account.expect("filtered on to_account.is_some()");
        if visited.contains(&next) {
            break;
        }
        visited.push(next);
        cur = next;
    }
    (cur != from).then_some(cur)
}
