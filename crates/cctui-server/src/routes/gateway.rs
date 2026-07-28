//! `/gateway/anthropic/*` (and `/gateway/openai/*`) — the OAuth passthrough
//! gateway.
//!
//! This is a **pure passthrough** that owns only OAuth storage + refresh:
//!
//!   1. The worker carries a session-scoped cctui token (minted at spawn, mapped
//!      to `(session_id, account_id)`), sent as the upstream `Authorization`
//!      bearer (`ANTHROPIC_AUTH_TOKEN`).
//!   2. Per request we map that token → account, swap `Authorization` to the
//!      account's current OAuth access token (refreshing under a per-account
//!      mutex when near expiry), and stream the bytes both ways. Every other
//!      client header is preserved verbatim.
//!   3. Status codes, `retry-after`, overload/streaming reconnects pass through
//!      untouched — the harness handles backoff exactly as if talking upstream
//!      directly. **No retries, no rate-limit handling, no body rewriting.**
//!
//! Stats are opportunistic: request count + byte count, never buffered parsing.
//! Raw OAuth tokens never enter worker env, logs, or session records.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use futures_util::StreamExt;
use uuid::Uuid;

use crate::state::AppState;

/// Refresh proactively once the access token is within this window of expiry.
const REFRESH_SKEW_SECS: i64 = 60;

const SESSION_TOKEN_TTL_HOURS_DEFAULT: i64 = 12;

fn ttl_hours_from(var: Option<String>) -> i64 {
    var.and_then(|v| v.parse::<i64>().ok())
        .filter(|h| *h > 0)
        .unwrap_or(SESSION_TOKEN_TTL_HOURS_DEFAULT)
}

fn session_token_ttl() -> chrono::Duration {
    chrono::Duration::hours(ttl_hours_from(std::env::var("CCTUI_SESSION_TOKEN_TTL_HOURS").ok()))
}

/// Anthropic Claude-Code OAuth token endpoint + client id. These are not stable
/// public APIs (caveat accepted in the ticket); overridable via env so we can
/// track upstream changes without a redeploy of code.
pub fn anthropic_token_url() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_TOKEN_URL")
        .unwrap_or_else(|_| "https://console.anthropic.com/v1/oauth/token".into())
}
pub fn anthropic_client_id() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_CLIENT_ID")
        .unwrap_or_else(|_| "9d1c250a-e61b-44d9-88ed-5944d1962f5e".into())
}
/// claude.ai authorize endpoint for the manual code-paste OAuth login.
/// Overridable so we can track upstream without a redeploy.
pub fn anthropic_authorize_url() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_AUTHORIZE_URL")
        .unwrap_or_else(|_| "https://claude.ai/oauth/authorize".into())
}
/// Redirect URI used for the manual code-paste flow — claude.ai displays the
/// `code#state` pair instead of redirecting. Must match what the token exchange
/// sends back.
pub fn anthropic_oauth_redirect_uri() -> String {
    "https://console.anthropic.com/oauth/code/callback".into()
}
fn anthropic_upstream() -> String {
    std::env::var("CCTUI_ANTHROPIC_UPSTREAM").unwrap_or_else(|_| "https://api.anthropic.com".into())
}
/// OpenAI/Codex OAuth token endpoint. Codex's public client exchanges +
/// refreshes here with **form-encoded** bodies (unlike Anthropic's JSON).
/// Overridable via env to track upstream changes without a code redeploy.
pub fn openai_token_url() -> String {
    std::env::var("CCTUI_OPENAI_OAUTH_TOKEN_URL")
        .unwrap_or_else(|_| "https://auth.openai.com/oauth/token".into())
}
/// Codex's public OAuth client id. Defaults to the well-known `codex` client
/// (`app_EMoamEEZ73f0CkXaXp7hrann`); overridable via env.
pub fn openai_client_id() -> String {
    std::env::var("CCTUI_OPENAI_OAUTH_CLIENT_ID")
        .unwrap_or_else(|_| "app_EMoamEEZ73f0CkXaXp7hrann".into())
}
/// auth.openai.com authorize endpoint for the "Sign in with `ChatGPT`" login.
/// Overridable so we can track upstream without a redeploy.
pub fn openai_authorize_url() -> String {
    std::env::var("CCTUI_OPENAI_OAUTH_AUTHORIZE_URL")
        .unwrap_or_else(|_| "https://auth.openai.com/oauth/authorize".into())
}
/// Fixed redirect URI baked into Codex's public client — we can't point it at
/// our own host. The browser redirect to localhost:1455 fails to load; the
/// user copies the full URL from the address bar and pastes it back.
pub fn openai_oauth_redirect_uri() -> String {
    std::env::var("CCTUI_OPENAI_OAUTH_REDIRECT_URI")
        .unwrap_or_else(|_| "http://localhost:1455/auth/callback".into())
}
fn openai_upstream() -> String {
    // Codex ChatGPT-backed accounts talk to the chatgpt backend, NOT
    // api.openai.com (matches what the codex CLI + CLIProxyAPI do).
    std::env::var("CCTUI_OPENAI_UPSTREAM")
        .unwrap_or_else(|_| "https://chatgpt.com/backend-api/codex".into())
}

/// Fireworks' OpenAI-compatible inference base. A provider row's `base_url`
/// still wins when set; this is the default upstream for the family.
fn fireworks_upstream() -> String {
    std::env::var("CCTUI_FIREWORKS_UPSTREAM")
        .unwrap_or_else(|_| "https://api.fireworks.ai/inference/v1".into())
}

/// Per-provider request-shaping settings for the `fireworks` family, resolved
/// over [`fireworks_default_settings`]. Applied by the gateway on the way
/// upstream so no worker needs to know them (and none can bypass them).
pub struct FireworksSettings {
    /// Injected as the request body's `context_length_exceeded_behavior`
    /// (Fireworks defaults to `truncate`, which silently loses prompt).
    /// `None` (settings key `null`) injects nothing.
    pub context_length_exceeded_behavior: Option<String>,
    /// Pin a conversation's requests to one replica so its prompt prefix stays
    /// cache-warm: the session id goes out as `user` + `x-session-affinity`.
    pub session_affinity: bool,
    /// Extra body keys merged in, none overriding what the client sent.
    pub extra_body: serde_json::Map<String, serde_json::Value>,
}

/// Defaults for a new `fireworks` provider row. Stored as data on the row at
/// create so the accounts UI can edit every knob.
pub fn fireworks_default_settings() -> serde_json::Value {
    serde_json::json!({
        "context_length_exceeded_behavior": "error",
        "session_affinity": true,
        "extra_body": {},
    })
}

impl FireworksSettings {
    /// Resolve a stored `provider_settings` blob over the defaults; an absent or
    /// malformed blob yields the defaults.
    pub fn resolve(stored: Option<&serde_json::Value>) -> Self {
        let mut merged = fireworks_default_settings();
        if let Some(overlay) = stored.filter(|v| v.is_object()) {
            deep_merge_json(&mut merged, overlay.clone());
        }
        Self {
            context_length_exceeded_behavior: merged
                .get("context_length_exceeded_behavior")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned),
            session_affinity: merged
                .get("session_affinity")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            extra_body: merged
                .get("extra_body")
                .and_then(serde_json::Value::as_object)
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// Apply the settings to a JSON request body. Every injection is
    /// "only if absent" — an explicit client value always wins.
    pub fn apply_body(&self, body: &mut serde_json::Value, session_id: Option<&str>) {
        let Some(obj) = body.as_object_mut() else { return };
        if let Some(behavior) = self.context_length_exceeded_behavior.as_ref() {
            obj.entry("context_length_exceeded_behavior")
                .or_insert_with(|| serde_json::Value::String(behavior.clone()));
        }
        for (k, v) in &self.extra_body {
            obj.entry(k.clone()).or_insert_with(|| v.clone());
        }
        if self.session_affinity
            && let Some(sid) = session_id
        {
            obj.entry("user").or_insert_with(|| serde_json::Value::String(sid.to_owned()));
        }
    }
}

/// The provider *family* of an account: which env vars it drives, and the key
/// `UNIQUE (account_id, family)` enforces one credential per. `fireworks` is its
/// own family — despite the `OpenAI` wire protocol — so a Fireworks key can sit
/// next to a codex credential on one account.
///
/// [`label`](Self::label) is the stored value of the generated `family` column;
/// per-family SQL predicates compare against it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    Anthropic,
    Openai,
    Fireworks,
}

impl Family {
    /// Derive the family from a stored `provider` value. Must agree with the
    /// generated `family` column (migration 078).
    pub fn from_provider(provider: &str) -> Self {
        if provider == "fireworks" {
            Self::Fireworks
        } else if provider.contains("openai") {
            Self::Openai
        } else {
            Self::Anthropic
        }
    }
    /// Parse a family label back (the `family` column / API `family` field).
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::Openai),
            "fireworks" => Some(Self::Fireworks),
            _ => None,
        }
    }
    /// Derive the family from a spawn adapter id (`codex*` → openai,
    /// `opencode*` → fireworks, else anthropic). This IS the spawn resolution
    /// key: the adapter names the harness family, and the account identity
    /// carries at most one provider row per family.
    pub fn from_adapter(adapter_id: &str) -> Self {
        if adapter_id.starts_with("opencode") {
            Self::Fireworks
        } else if adapter_id.starts_with("codex") {
            Self::Openai
        } else {
            Self::Anthropic
        }
    }
    /// Human label for error messages, and the stored `family` column value.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Fireworks => "fireworks",
        }
    }
}

/// Why [`mint_session_env`] could not mint, kept apart so callers can 404 with
/// a message naming the actual gap: "no such account" and "account
/// exists but carries no provider for this harness family" need different
/// remedies (fix the name vs. connect a provider).
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
    pub fn family(&self) -> Family {
        Family::from_provider(&self.provider)
    }
}

/// Resolve an account identity by name for a user — either one they OWN or one
/// SHARED to them (`account_shares`), preferring their own on a name
/// clash — and return its provider rows. `Ok(None)` means no such
/// account; `Ok(Some(rows))` may be empty for an identity with no connected
/// providers.
pub async fn account_provider_rows(
    state: &AppState,
    user_id: Uuid,
    account_name: &str,
) -> Result<Option<Vec<ProviderRow>>, sqlx::Error> {
    let account_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT a.id FROM accounts a \
         WHERE a.name = $2 \
           AND (a.user_id = $1 OR EXISTS ( \
               SELECT 1 FROM resource_shares s \
                WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                  AND s.grantee_id = $1 AND s.revoked_at IS NULL)) \
         ORDER BY (a.user_id = $1) DESC LIMIT 1",
    )
    .bind(user_id)
    .bind(account_name)
    .fetch_optional(&state.pool)
    .await?;
    let Some(account_id) = account_id else { return Ok(None) };
    let rows: Vec<(Uuid, String, Option<serde_json::Value>, Option<serde_json::Value>)> =
        sqlx::query_as(
            "SELECT id, provider, model_aliases, models FROM account_providers \
             WHERE account_id = $1 ORDER BY provider",
        )
        .bind(account_id)
        .fetch_all(&state.pool)
        .await?;
    Ok(Some(
        rows.into_iter()
            .map(|(id, provider, model_aliases, models)| ProviderRow {
                id,
                provider,
                model_aliases,
                models,
            })
            .collect(),
    ))
}

/// Resolve a named account for a user and mint a session-scoped gateway token
/// bound to `(session_id, provider row)`, returning the env vars to inject into
/// the worker so its agent traffic flows through this gateway under that
/// account. The raw credentials never leave the server —
/// only the opaque session token does. Resolution is by `(account identity,
/// harness family)`: the account carries at most one provider per
/// family, and `family` — derived from the adapter via
/// [`Family::from_adapter`] — picks that row.
pub async fn mint_session_env(
    state: &AppState,
    user_id: Uuid,
    account_name: &str,
    family: Family,
    session_id: &str,
) -> Result<std::collections::BTreeMap<String, String>, MintSessionEnvError> {
    let rows = account_provider_rows(state, user_id, account_name)
        .await
        .map_err(MintSessionEnvError::Db)?;
    let Some(rows) = rows else {
        // Fail-diagnosable, not silent: the account name resolved to
        // nothing for this user — neither owned nor shared. This is the exact
        // shape of the "404 no account named X" dispatch failures, so name the
        // user + account here rather than leaving a caller to dig through the DB.
        tracing::warn!(
            %user_id,
            account = %account_name,
            "mint_session_env: no account resolved (not owned by, nor shared to, this user)"
        );
        return Err(MintSessionEnvError::NoAccount);
    };
    let Some(row) = rows.iter().find(|r| r.family() == family) else {
        tracing::warn!(
            %user_id,
            account = %account_name,
            family = family.label(),
            "mint_session_env: account has no provider for this family"
        );
        return Err(MintSessionEnvError::NoProviderForFamily(family));
    };
    mint_env_for_account(state, row.id, &row.provider, session_id)
        .await
        .map_err(MintSessionEnvError::Db)
}

/// Re-mint a gateway session token + env for an **already-resolved** provider
/// row. `provider_id` is the `account_providers.id` the
/// session token binds to — NOT the identity-level `accounts.id`. Used on the
/// resume path, where the session already has a bound provider row (persisted
/// via `session_tokens` / `sessions.account_id`) and we just need to re-issue a
/// fresh token + env for the revived worker rather than re-resolving by name,
/// and on the dispatch path after `(account, family)` resolution.
pub async fn mint_session_env_for_account(
    state: &AppState,
    provider_id: Uuid,
    session_id: &str,
) -> Result<Option<std::collections::BTreeMap<String, String>>, sqlx::Error> {
    let provider: Option<String> =
        sqlx::query_scalar("SELECT provider FROM account_providers WHERE id = $1")
            .bind(provider_id)
            .fetch_optional(&state.pool)
            .await?;
    let Some(provider) = provider else { return Ok(None) };
    Ok(Some(mint_env_for_account(state, provider_id, &provider, session_id).await?))
}

/// Resolve a session's bound OAuth account and re-mint its gateway env, ready
/// to hand to the daemon on any wake path (explicit resume *or* reply-driven
/// cold-resume) so a revived worker routes through the gateway with a fresh
/// valid token instead of launching with empty env and 401ing.
///
/// The binding is durable on `sessions.account_id`; falls back to the
/// most-recent non-revoked `session_tokens` row for sessions bound before that
/// column was populated. Returns empty env (never errors) for sessions with no
/// account binding — callers attach it unconditionally.
pub async fn resume_env_for_session(
    state: &AppState,
    session_id: &str,
) -> std::collections::BTreeMap<String, String> {
    // Re-mint EVERY bound family and merge. The two families emit disjoint env
    // keys (`ANTHROPIC_*` vs `OPENAI_*`), so a worker carrying both claude +
    // codex creds gets both restored — not just the last-minted family.
    let mut env = std::collections::BTreeMap::new();
    for aid in resolve_session_accounts(state, session_id).await {
        match mint_session_env_for_account(state, aid, session_id).await {
            Ok(Some(e)) => env.extend(e),
            Ok(None) => {} // this family's account row is gone; others may still mint
            Err(e) => {
                tracing::error!(%session_id, "re-mint gateway env on wake failed: {e}");
            }
        }
    }
    env
}

/// Resolve a session's bound OAuth accounts — **one per provider family**.
/// A session can carry a claude (Anthropic) account *and* a codex
/// (`OpenAI`) account at once; both must be re-minted on wake or the worker
/// launches missing one family's creds and 401s (the multi-account dispatch
/// regression). The durable binding lives on the session's live `session_tokens`
/// rows (one stable token per family, see `mint_env_for_account`); we take the
/// newest live token per family. Falls back to the legacy single
/// `sessions.account_id` column for sessions bound before per-family tokens
/// existed. Empty when the session has no account binding at all.
pub async fn resolve_session_accounts(state: &AppState, session_id: &str) -> Vec<Uuid> {
    // DISTINCT ON the family keeps the newest token per family, preferring live
    // over revoked. Revoked rows COUNT as a binding:
    // session end revokes every token (`revoke_session_tokens`), so a resume
    // after a real — or spurious — end would otherwise find no binding and
    // relaunch the worker with EMPTY gateway env (silently off-gateway on a
    // desktop, a 401 loop on k8s). The revoked row still names the account;
    // `mint_env_for_account` then mints a FRESH live token — the dead token
    // itself is never resurrected.
    let mut ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT ON (oa.family) st.account_id \
         FROM session_tokens st JOIN account_providers oa ON oa.id = st.account_id \
         WHERE st.session_id = $1 \
         ORDER BY oa.family, (st.revoked_at IS NULL) DESC, st.created_at DESC",
    )
    .bind(session_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    if ids.is_empty()
        && let Some(aid) = sqlx::query_scalar::<_, Uuid>(
            "SELECT account_id::uuid FROM sessions WHERE id = $1 AND account_id IS NOT NULL",
        )
        .bind(session_id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
    {
        ids.push(aid);
    }
    ids
}

/// Mint a fresh opaque session token bound to `(session_id, account_id)`,
/// persist the account on the session row so the binding is durable across id
/// rotation / restart, and return the gateway env for the account's
/// provider family.
async fn mint_env_for_account(
    state: &AppState,
    account_id: Uuid,
    provider: &str,
    session_id: &str,
) -> Result<std::collections::BTreeMap<String, String>, sqlx::Error> {
    let family = Family::from_provider(provider);

    // A session gets ONE stable gateway token **per provider family** for its
    // whole life. Reuse the existing live token for THIS
    // family if we have one persisted, rather than minting a fresh row on every
    // resume — re-minting bloated `session_tokens` for no reason and left live
    // workers holding a token the gateway might no longer resolve. The token
    // string is immutable; only its account binding moves (repointed below) on
    // an account switch. Scoping reuse + repoint to the family is what
    // lets a worker carry claude + codex at once: minting the OpenAI account
    // must NOT repoint the Anthropic token to it.
    let key = crate::crypto::vault_key();
    let token = if let Some(existing) =
        existing_session_token(state, session_id, family, &key).await
    {
        // Repoint only THIS family's live token to the requested account (and
        // un-revoke, defensively) so an account switch reuses the same string
        // — the worker's `ANTHROPIC_AUTH_TOKEN`/`OPENAI_API_KEY` never
        // changes, the gateway just resolves it to the new account. The
        // `account_providers` join + family predicate confine the repoint to the
        // same-family token, leaving the other family's token untouched.
        let _ = sqlx::query(
            "UPDATE session_tokens AS st SET account_id = $2, revoked_at = NULL, expires_at = $4 \
                 FROM account_providers AS oa \
                 WHERE st.session_id = $1 AND st.revoked_at IS NULL \
                   AND st.account_id = oa.id \
                   AND oa.family = $3",
        )
        .bind(session_id)
        .bind(account_id)
        .bind(family.label())
        .bind(Utc::now() + session_token_ttl())
        .execute(&state.pool)
        .await;
        // The reused token's fingerprint may have been flagged as a
        // spamming orphan while its binding was broken — the
        // rebind keeps the SAME token string, so clear the block now
        // instead of leaving the just-fixed binding 401ing for the
        // remainder of the (up to 300s) block window.
        clear_orphan_fingerprint(&state.gateway_orphan_spam, &crate::auth::sha256_hex(&existing));
        existing
    } else {
        // First token for this session: mint a fresh opaque token (same
        // shape/entropy as other secrets), store its hash AND its
        // encrypted plaintext so resume can re-supply the same string.
        let token = format!("cctui_s_{}", crate::auth::mint_secret());
        let token_hash = crate::auth::sha256_hex(&token);
        let enc = crate::crypto::encrypt(&token, &key);
        sqlx::query(
            "INSERT INTO session_tokens \
                 (token_hash, session_id, account_id, encrypted_token, expires_at) \
                 VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&token_hash)
        .bind(session_id)
        .bind(account_id)
        .bind(&enc)
        .bind(Utc::now() + session_token_ttl())
        .execute(&state.pool)
        .await?;
        token
    };

    // Persist the account on the session row so it survives id rotation and
    // server restart — the resume path re-mints from here. Best
    // effort: a session row may not exist yet at spawn-time mint (registration
    // races), so a no-op update is fine; the token row is the live binding.
    let _ = sqlx::query("UPDATE sessions SET account_id = $2 WHERE id = $1")
        .bind(session_id)
        .bind(account_id.to_string())
        .execute(&state.pool)
        .await;

    let base = state.config.external_url.trim_end_matches('/');
    let mut env = std::collections::BTreeMap::new();
    // Per-account custom env: decrypt the account's `env_json` blob and
    // merge it in FIRST, so the gateway routing keys inserted by the `match`
    // below always win over any account-supplied key of the same name. Because
    // every worker (re)launch path funnels through here — initial spawn,
    // `resume_env_for_session`, and the daemon's `gateway-env` pull — the account
    // env is re-served on respawn/resume and survives a daemon / claude-daemon
    // restart, not just the initial spawn.
    if let Some(account_env) = account_env_json(state, account_id, &key).await {
        env.extend(account_env);
    }
    apply_gateway_env(&mut env, family, base, token);
    Ok(env)
}

/// Insert the family's gateway routing keys over whatever the account env
/// already carries. The three families emit DISJOINT key pairs, which is what
/// lets one worker hold a claude + a codex + a Fireworks credential at once —
/// `resume_env_for_session` merges every bound family's env blindly.
fn apply_gateway_env(
    env: &mut std::collections::BTreeMap<String, String>,
    family: Family,
    base: &str,
    token: String,
) {
    match family {
        Family::Anthropic => {
            env.insert("ANTHROPIC_BASE_URL".into(), format!("{base}/gateway/anthropic"));
            env.insert("ANTHROPIC_AUTH_TOKEN".into(), token);
            apply_anthropic_cache_defaults(env);
        }
        Family::Openai => {
            env.insert("OPENAI_BASE_URL".into(), format!("{base}/gateway/openai"));
            env.insert("OPENAI_API_KEY".into(), token);
        }
        Family::Fireworks => {
            env.insert("FIREWORKS_BASE_URL".into(), format!("{base}/gateway/fireworks"));
            env.insert("FIREWORKS_API_KEY".into(), token);
        }
    }
}

/// Default-on Anthropic 1-hour prompt-cache flag: `or_insert_with`
/// (not a plain insert) so the account's already-merged resolved env can
/// override it — `ENABLE_PROMPT_CACHING_1H=0` opts back out — while the default
/// preserves the prior always-on behaviour. Curated in the settings catalog.
fn apply_anthropic_cache_defaults(env: &mut std::collections::BTreeMap<String, String>) {
    env.entry("ENABLE_PROMPT_CACHING_1H".to_string()).or_insert_with(|| "1".to_string());
}

/// The session's existing stable gateway token for a given provider family
/// (decrypted), if one was minted and persisted with its plaintext.
/// `family` selects the row so the families' tokens stay independent. `None` for
/// a family with no live token, or pre-migration rows that only stored the
/// one-way hash (those fall through to a one-time fresh mint). Picks the newest
/// live token on the off chance a session accrued several from the old
/// re-mint-on-resume behaviour.
async fn existing_session_token(
    state: &AppState,
    session_id: &str,
    family: Family,
    key: &[u8],
) -> Option<String> {
    let enc: String = sqlx::query_scalar(
        "SELECT st.encrypted_token FROM session_tokens st \
         JOIN account_providers oa ON oa.id = st.account_id \
         WHERE st.session_id = $1 AND st.revoked_at IS NULL \
           AND st.encrypted_token IS NOT NULL \
           AND oa.family = $2 \
         ORDER BY st.created_at DESC LIMIT 1",
    )
    .bind(session_id)
    .bind(family.label())
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()?;
    crate::crypto::decrypt(&enc, key)
}

/// Resolve a logical model name through a named account's alias map.
///
/// Mirrors [`mint_session_env`]'s `(account identity, family)` resolution
/// so spawn maps the *same* provider row the gateway binds the
/// session to, then looks `model` up in that row's `model_aliases` JSON object.
/// Returns the mapped concrete model (e.g. `opus` → `claude-opus-4-8[1m]`) or
/// the input unchanged when there's no account, no family match, no alias map,
/// or no matching key. A DB error degrades gracefully to the unmapped model
/// rather than failing spawn.
///
/// The `fireworks` family additionally resolves through the account's model
/// catalog ([`resolve_catalog_model`]), which is the sole source of its model
/// ids — its harness has no built-in model list to fall back on.
pub async fn resolve_account_model(
    state: &AppState,
    user_id: Uuid,
    account_name: &str,
    family: Family,
    model: &str,
) -> String {
    let Ok(Some(rows)) = account_provider_rows(state, user_id, account_name).await else {
        return model.to_owned();
    };
    let Some(row) = rows.iter().find(|r| r.family() == family) else {
        return model.to_owned();
    };
    let aliased = row
        .model_aliases
        .as_ref()
        .and_then(|v| v.get(model))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map_or_else(|| model.to_owned(), str::to_owned);
    if family != Family::Fireworks {
        return aliased;
    }
    resolve_catalog_model(row.models.as_ref(), &aliased).unwrap_or(aliased)
}

/// Resolve a model against an account's catalog: an exact `model` id passes
/// through, a catalog `label` maps to its id, and anything else (including an
/// empty request) falls back to the first catalog entry. `None` when the catalog
/// is empty — the caller then keeps the requested model verbatim.
pub fn resolve_catalog_model(models: Option<&serde_json::Value>, model: &str) -> Option<String> {
    let entries = models?.as_array().filter(|a| !a.is_empty())?;
    let id_of = |e: &serde_json::Value| {
        e.get("model").and_then(serde_json::Value::as_str).map(str::to_owned)
    };
    let wanted = model.trim();
    if !wanted.is_empty()
        && let Some(hit) = entries.iter().find(|e| {
            id_of(e).as_deref() == Some(wanted)
                || e.get("label").and_then(serde_json::Value::as_str) == Some(wanted)
        })
    {
        return id_of(hit);
    }
    entries.iter().find_map(id_of)
}

/// Decrypt a named account's per-account custom env (`env_json`).
///
/// `env_json` is stored as an encrypted JSON object of `{VAR: value}` (the
/// values may be secrets — daemon-supplied env like a per-account API key — so
/// the column is encrypted at rest and never serialized back out of the accounts
/// model). Returns the decoded map, or `None` when the account has no custom env
/// / the blob can't be decrypted or parsed (degrade gracefully: a bad blob must
/// never fail the mint and strand the worker with no gateway routing).
async fn account_env_json(
    state: &AppState,
    account_id: Uuid,
    key: &[u8],
) -> Option<std::collections::BTreeMap<String, String>> {
    // `env_json` lives on the identity parent (`accounts`); `account_id`
    // here is the provider-row id (session_tokens FK), so join up
    // to the parent to read it.
    let enc: String = sqlx::query_scalar::<_, Option<String>>(
        "SELECT a.env_json FROM accounts a \
         JOIN account_providers ap ON ap.account_id = a.id \
         WHERE ap.id = $1",
    )
    .bind(account_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .flatten()?;
    let json = crate::crypto::decrypt(&enc, key)?;
    serde_json::from_str::<std::collections::BTreeMap<String, String>>(&json).ok()
}

/// The merged per-account `settings_json` for a session's bound account(s)
/// served to the daemon via the gateway-env pull so it is re-derived
/// on every worker (re)launch (spawn/resume/cold-resume/fork) — surviving a
/// daemon / claude-daemon restart the same way gateway env does.
///
/// A session can bind one account per provider family (claude + codex); their
/// `settings_json` blobs are deep-merged (later family wins on a key clash,
/// which in practice never happens — the two harnesses don't share settings
/// keys). Returns `None` when no bound account carries settings.
///
/// The daemon deep-merges this UNDER its own managed hook settings when it
/// writes the worker's `--settings` file, so the managed hooks always win.
/// This function only makes the settings available on the server pull path.
///
/// OPEN QUESTION — MANAGED-only keys (`strictKnownMarketplaces`,
/// `strictPluginOnlyCustomization`, `disableSideloadFlags`,
/// `blockedMarketplaces`): it is NOT verified at runtime whether claude honors
/// these via a plain `--settings` file, or whether they must land in the
/// managed-settings drop-in path (e.g. `/etc/claude-code/managed-settings.json`)
/// to take effect. This is a follow-up to confirm when a real user needs one of
/// these keys. It does NOT block this path: those keys are tagged MANAGED in the
/// settings catalog and rejected by `Catalog::validate_settings`, so no
/// per-account `settings_json` can carry them yet — whatever arrives here is
/// safe/care keys that `--settings` honors.
pub async fn resolve_session_settings(
    state: &AppState,
    session_id: &str,
) -> Option<serde_json::Value> {
    let mut merged: Option<serde_json::Value> = None;
    for account_id in resolve_session_accounts(state, session_id).await {
        let settings: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT settings_json FROM account_providers WHERE id = $1")
                .bind(account_id)
                .fetch_optional(&state.pool)
                .await
                .ok()
                .flatten();
        if let Some(s) = settings.filter(|v| !v.is_null()) {
            match merged.as_mut() {
                Some(base) => deep_merge_json(base, s),
                None => merged = Some(s),
            }
        }
    }
    merged
}

/// Recursively merge `overlay` into `base`. Objects merge key-by-key;
/// any non-object value in `overlay` replaces the value in `base`.
fn deep_merge_json(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(overlay_map)) => {
            for (k, v) in overlay_map {
                match base_map.get_mut(&k) {
                    Some(existing) => deep_merge_json(existing, v),
                    None => {
                        base_map.insert(k, v);
                    }
                }
            }
        }
        (base_slot, overlay) => *base_slot = overlay,
    }
}

/// Revoke every session token bound to a session — called when a
/// session ends so the gateway can no longer be used under that token.
pub async fn revoke_session_tokens(state: &AppState, session_id: &str) {
    let _ = sqlx::query(
        "UPDATE session_tokens SET revoked_at = now() \
         WHERE session_id = $1 AND revoked_at IS NULL",
    )
    .bind(session_id)
    .execute(&state.pool)
    .await;
}

/// Look up the `session_id` bound to a (live) gateway session token — used only
/// to tag Langfuse traces. `None` for unknown/revoked tokens.
async fn session_id_for_token(state: &AppState, session_token: &str) -> Option<String> {
    let hash = crate::auth::sha256_hex(session_token);
    sqlx::query_scalar::<_, String>(
        "SELECT session_id FROM session_tokens WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
}

/// Merge a `CctuiAgent` child's per-session dollar budget into `cap` as a
/// `session_usd` limit. A budget on the child always wins over an account-level
/// `session_usd`: it is the tighter, purpose-set ceiling.
pub fn merge_session_budget(
    cap: &crate::soft_limit::SoftLimits,
    budget_usd: Option<f64>,
) -> crate::soft_limit::SoftLimits {
    let Some(budget) = budget_usd.filter(|b| b.is_finite() && *b > 0.0) else {
        return cap.clone();
    };
    let mut merged = cap.clone();
    let entry = merged.limits.entry(crate::soft_limit::KEY_SESSION_USD.to_owned()).or_default();
    entry.cap_usd = Some(budget);
    merged
}

/// The account's soft limits with any per-session `CctuiAgent` budget applied.
/// Skips the token→session lookup entirely while no child budget is live.
async fn session_budget_limits(
    state: &AppState,
    acct: &Account,
    session_token: &str,
) -> crate::soft_limit::SoftLimits {
    if state.session_usd_budgets.is_empty() {
        return acct.soft_limits.clone();
    }
    let Some(session_id) = session_id_for_token(state, session_token).await else {
        return acct.soft_limits.clone();
    };
    let budget = state.session_usd_budgets.get(&session_id).map(|b| *b);
    merge_session_budget(&acct.soft_limits, budget)
}

/// Resolve a session token to its `(session_id, account_name)` — used by the
/// soft-limit signalling path to tag the per-session WS event with the
/// human account name (the `Account` struct carries no name). `None` for
/// unknown/revoked tokens.
async fn session_and_account_name_for_token(
    state: &AppState,
    session_token: &str,
) -> Option<(String, String)> {
    let hash = crate::auth::sha256_hex(session_token);
    sqlx::query_as::<_, (String, String)>(
        "SELECT t.session_id, a.name \
         FROM session_tokens t \
         JOIN account_providers ap ON ap.id = t.account_id \
         JOIN accounts a ON a.id = ap.account_id \
         WHERE t.token_hash = $1 AND t.revoked_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
}

/// Record a soft-limit block against a session and broadcast it.
///
/// Idempotent per block episode: the first refused passthrough for a session
/// flips `soft_limit_blocked` and emits [`ServerEvent::SoftLimitReached`]; the
/// worker's repeated Retry-After retries (still blocked) are no-ops, so the WS
/// stream isn't spammed. The webui shows the banner; the matching clear arrives
/// from [`clear_soft_limit_block`] on the next success or an account switch.
async fn mark_soft_limit_block(
    state: &AppState,
    session_id: &str,
    account_id: Uuid,
    account_name: &str,
    reason: &str,
    retry_after_secs: i64,
) {
    if session_id.is_empty() {
        return;
    }
    // Persist a durable block on the session row so the classifier drives the
    // session to `Bucket::Blocked` (✋ needs input) and the block survives a
    // resubscribe. The stored reason is an actionable "continue on
    // another account" hint; `list_sessions` reads it. Idempotent (overwrite),
    // and never clobbers the churning daemon `tempo`/`agent_state` signals.
    let needs = format!("switch account: {account_name} rate-limited");
    if let Err(e) = sqlx::query(
        "UPDATE sessions SET soft_limit_reason = $2 WHERE id = $1 AND status != 'archived'",
    )
    .bind(session_id)
    .bind(&needs)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%session_id, error = %e, "failed to persist soft-limit block");
    }
    // Only broadcast on the clear→blocked transition.
    if state.soft_limit_blocked.insert(session_id.to_owned(), ()).is_none() {
        state.bus.publish_server(cctui_proto::ws::ServerEvent::SoftLimitReached {
            session_id: session_id.to_owned(),
            account_id,
            account_name: account_name.to_owned(),
            reason: reason.to_owned(),
            retry_after_secs,
        });
    }
}

/// Clear a session's soft-limit block and broadcast the dismissal.
/// Only emits on the blocked→clear transition (no-op if it wasn't blocked).
pub async fn clear_soft_limit_block(state: &AppState, session_id: &str) {
    if session_id.is_empty() {
        return;
    }
    // Drop the durable block on the session row so the classifier stops forcing
    // `Bucket::Blocked` and the session returns to its real signal-derived
    // bucket. Best-effort; clear it whenever set, even if the
    // in-memory dedup entry was already gone (e.g. after a server restart).
    if let Err(e) = sqlx::query(
        "UPDATE sessions SET soft_limit_reason = NULL \
         WHERE id = $1 AND soft_limit_reason IS NOT NULL",
    )
    .bind(session_id)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%session_id, error = %e, "failed to clear soft-limit block");
    }
    if state.soft_limit_blocked.remove(session_id).is_some() {
        state.bus.publish_server(cctui_proto::ws::ServerEvent::SoftLimitCleared {
            session_id: session_id.into(),
        });
    }
}

/// Record that a session token was just presented at the gateway, so the UI
/// can distinguish an account-bound session whose worker actually routes here
/// from one silently riding ambient creds. Fire-and-forget + self-throttling
/// (skips a write when stamped within the last minute) to stay off the
/// passthrough hot path. `token_fp` is the sha256 hex == `session_tokens.token_hash`.
fn note_token_used(state: &AppState, token_fp: &str) {
    let pool = state.pool.clone();
    let hash = token_fp.to_owned();
    tokio::spawn(async move {
        let _ = sqlx::query(
            "UPDATE session_tokens SET last_used_at = now() \
             WHERE token_hash = $1 \
               AND (last_used_at IS NULL OR last_used_at < now() - interval '60 seconds')",
        )
        .bind(&hash)
        .execute(&pool)
        .await;
    });
}

/// Flag an account as needing reauthentication: the upstream provider
/// rejected its OAuth credentials. Persists `needs_reauth` + the error so the
/// accounts UI can show a "credential rejected — reauthenticate" badge. Gated on
/// the in-memory set so a flapping worker doesn't re-write the row on every 401 —
/// the DB write fires only on the false→true transition.
fn flag_account_reauth(state: &AppState, account_id: Uuid, reason: &str) {
    if state.account_reauth.insert(account_id, ()).is_some() {
        return; // already flagged in memory — no redundant write
    }
    let pool = state.pool.clone();
    let reason = reason.to_string();
    tokio::spawn(async move {
        if let Err(e) = sqlx::query(
            "UPDATE account_providers \
                SET needs_reauth = true, last_auth_error = $2, last_auth_error_at = now() \
             WHERE id = $1",
        )
        .bind(account_id)
        .bind(reason)
        .execute(&pool)
        .await
        {
            tracing::warn!(account = %account_id, error = %e, "failed to flag account reauth");
        }
    });
}

/// Clear an account's reauth flag after a successful upstream call.
/// Gated on the in-memory set so the common case (account healthy) costs nothing;
/// the DB write fires only on the true→false transition.
fn clear_account_reauth(state: &AppState, account_id: Uuid) {
    if state.account_reauth.remove(&account_id).is_none() {
        return; // not flagged — nothing to clear
    }
    let pool = state.pool.clone();
    tokio::spawn(async move {
        if let Err(e) = sqlx::query(
            "UPDATE account_providers \
                SET needs_reauth = false, last_auth_error = NULL, last_auth_error_at = NULL \
             WHERE id = $1 AND needs_reauth",
        )
        .bind(account_id)
        .execute(&pool)
        .await
        {
            tracing::warn!(account = %account_id, error = %e, "failed to clear account reauth");
        }
    });
}

/// Raw account row as selected from the join (before decrypt).
type AccountRow = (
    Uuid,
    String,
    Option<String>,
    Option<String>,
    Option<chrono::DateTime<Utc>>,
    Option<String>,
    Option<String>,
    String,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
);
/// Raw account row by id (no id column, before decrypt).
type ReloadRow = (
    String,
    Option<String>,
    Option<String>,
    Option<chrono::DateTime<Utc>>,
    Option<String>,
    Option<String>,
    String,
);

/// Loaded account row (decrypted in-process; never serialized out).
struct Account {
    id: Uuid,
    provider: String,
    access_token: Option<String>,
    /// `None` for compatible endpoints (static credential, no refresh).
    refresh_token: Option<String>,
    expires_at: Option<chrono::DateTime<Utc>>,
    /// For Codex/OpenAI accounts: the `chatgpt_account_id` claim, sent upstream
    /// as the `Chatgpt-Account-Id` header. NULL for anthropic / manual
    /// refresh-token accounts.
    provider_account_id: Option<String>,
    /// Compatible-endpoint upstream base URL. NULL → built-in upstream.
    base_url: Option<String>,
    /// `oauth` (refreshing subscription) | `bearer` | `api_key` (static).
    auth_scheme: String,
    /// Per-account soft limits on cctui's own share of the usage windows.
    /// Enforced in `passthrough` against the cached usage. All NULL ⇒
    /// no soft limit (prior behaviour).
    soft_limits: crate::soft_limit::SoftLimits,
    /// Per-provider gateway request-shaping settings; see [`FireworksSettings`].
    provider_settings: Option<serde_json::Value>,
}

impl Account {
    /// A static-credential compatible account forwards its stored credential
    /// verbatim and skips the OAuth refresh round-trip.
    fn is_static(&self) -> bool {
        self.auth_scheme != "oauth"
    }
}

/// Resolve the session token (the upstream bearer the worker sent) to its
/// account. Returns `None` for unknown/revoked tokens.
/// Env-tunable thresholds for the orphan-token spam guard. Parsed once.
struct OrphanSpamCfg {
    /// Unresolved 401s within `window` before a fingerprint is blocked.
    threshold: u32,
    /// Counting window.
    window: std::time::Duration,
    /// How long a flagged fingerprint stays blocked (DB lookups skipped).
    block: std::time::Duration,
}

static ORPHAN_SPAM_CFG: std::sync::LazyLock<OrphanSpamCfg> = std::sync::LazyLock::new(|| {
    fn env_u64(name: &str, default: u64) -> u64 {
        std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
    }
    OrphanSpamCfg {
        threshold: u32::try_from(env_u64("CCTUI_GATEWAY_SPAM_THRESHOLD", 10)).unwrap_or(10),
        window: std::time::Duration::from_secs(env_u64("CCTUI_GATEWAY_SPAM_WINDOW_SECS", 60)),
        block: std::time::Duration::from_secs(env_u64("CCTUI_GATEWAY_SPAM_BLOCK_SECS", 300)),
    }
});

type OrphanSpamMap = dashmap::DashMap<String, crate::state::OrphanSpam>;

/// True if this token fingerprint is currently blocked as a spamming orphan.
/// Pure in-memory check — no DB — so blocked orphans cost ~nothing.
fn orphan_is_blocked(state: &AppState, token_fp: &str) -> bool {
    orphan_is_blocked_at(&state.gateway_orphan_spam, token_fp, std::time::Instant::now())
}

fn orphan_is_blocked_at(map: &OrphanSpamMap, token_fp: &str, now: std::time::Instant) -> bool {
    let Some(entry) = map.get(token_fp) else { return false };
    matches!(entry.blocked_until, Some(until) if until > now)
}

/// Drop a token fingerprint from the in-memory orphan-spam state.
///
/// Called after a successful rebind/mint that reuses an existing token string:
/// the fingerprint may have been blocked while the binding was broken (an
/// unresolvable token 401s its way past the threshold), and since a rebind
/// repoints the SAME token string, the block would otherwise keep dropping a
/// NOW-VALID token's requests for the remainder of the block window (up to
/// 300s). Clearing re-enables the DB lookup immediately. Idempotent.
fn clear_orphan_fingerprint(map: &OrphanSpamMap, token_fp: &str) {
    map.remove(token_fp);
}

/// Clear the orphan-spam block for every live token of `session_id`.
///
/// The explicit account-switch path (`sessions::switch_account`) rebinds token
/// rows by session id without the token plaintext in hand;
/// `session_tokens.token_hash` IS the fingerprint the spam guard keys on (both
/// are the sha256 hex of the token string), so clearing by stored hash needs no
/// token material. Best-effort: a failed lookup just leaves the block to
/// expire on its own.
pub async fn clear_orphan_block_for_session(state: &AppState, session_id: &str) {
    let hashes: Vec<String> = sqlx::query_scalar(
        "SELECT token_hash FROM session_tokens WHERE session_id = $1 AND revoked_at IS NULL",
    )
    .bind(session_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    for hash in &hashes {
        clear_orphan_fingerprint(&state.gateway_orphan_spam, hash);
    }
}

/// Record an unresolvable-token 401 and, once a fingerprint crosses the spam
/// threshold within the window, flag it as a blocked orphan and log LOUDLY.
fn note_orphan_401(state: &AppState, token_fp: &str) {
    let cfg = &*ORPHAN_SPAM_CFG;
    let fp_short: String = token_fp.chars().take(12).collect();
    let (count, newly_blocked) = bump_orphan_401(
        &state.gateway_orphan_spam,
        token_fp,
        std::time::Instant::now(),
        cfg.threshold,
        cfg.window,
        cfg.block,
    );

    if newly_blocked {
        tracing::error!(
            stage = "session-token",
            token_fp = %fp_short,
            count,
            block_secs = cfg.block.as_secs(),
            "🔴 GATEWAY ORPHAN SPAM: unresolvable session token exceeded {} 401s in {}s — \
             blocking fingerprint for {}s; subsequent requests dropped before any DB lookup. \
             A zombie worker lost its session→account binding; resume or kill it.",
            cfg.threshold,
            cfg.window.as_secs(),
            cfg.block.as_secs(),
        );
    } else {
        tracing::warn!(
            stage = "session-token",
            token_fp = %fp_short,
            count,
            "gateway 401: session token not resolvable (orphan worker retrying)"
        );
    }
}

/// Pure sliding-window counter. Returns `(count_in_window, newly_blocked)` where
/// `newly_blocked` is true only on the transition that flags the fingerprint.
fn bump_orphan_401(
    map: &OrphanSpamMap,
    token_fp: &str,
    now: std::time::Instant,
    threshold: u32,
    window: std::time::Duration,
    block: std::time::Duration,
) -> (u32, bool) {
    let mut entry = map.entry(token_fp.to_string()).or_insert_with(|| crate::state::OrphanSpam {
        count: 0,
        window_start: now,
        blocked_until: None,
    });

    // Roll the window over once it elapses (also clears an expired block).
    if now.duration_since(entry.window_start) > window {
        entry.count = 0;
        entry.window_start = now;
        entry.blocked_until = None;
    }
    entry.count += 1;
    let count = entry.count;

    let newly_blocked = count >= threshold && entry.blocked_until.is_none();
    if newly_blocked {
        entry.blocked_until = Some(now + block);
    }
    drop(entry);
    (count, newly_blocked)
}

/// Resolve a session token to its bound account.
///
/// Three-valued on purpose: `Ok(Some)` = bound and live;
/// `Ok(None)` = the token is genuinely unknown/revoked/unbound (a real orphan);
/// `Err` = the DB lookup itself failed (cold/starved pool on a server restart,
/// transient network). The caller MUST NOT treat `Err` as an orphan — doing so
/// fed the spam guard during restarts and blocked perfectly valid tokens for
/// 300s (the "401 on every server restart" regression). On `Err` we return a
/// retryable 503 and never touch the orphan block.
async fn resolve_account(
    state: &AppState,
    session_token: &str,
) -> Result<Option<Account>, sqlx::Error> {
    let hash = crate::auth::sha256_hex(session_token);
    let row: Option<AccountRow> = sqlx::query_as(
        "SELECT a.id, a.provider, a.encrypted_access_token, a.encrypted_refresh_token, \
                    a.expires_at, a.provider_account_id, a.base_url, a.auth_scheme, \
                    a.soft_limits_json, a.provider_settings \
             FROM session_tokens t JOIN account_providers a ON a.id = t.account_id \
             WHERE t.token_hash = $1 AND t.revoked_at IS NULL \
               AND (t.expires_at IS NULL OR t.expires_at > now())",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await?;
    let Some((
        id,
        provider,
        enc_access,
        enc_refresh,
        expires_at,
        provider_account_id,
        base_url,
        auth_scheme,
        soft_limits_json,
        provider_settings,
    )) = row
    else {
        return Ok(None);
    };
    let key = crate::crypto::vault_key();
    let access_token = enc_access.and_then(|e| crate::crypto::decrypt(&e, &key));
    let refresh_token = enc_refresh.and_then(|e| crate::crypto::decrypt(&e, &key));
    Ok(Some(Account {
        id,
        provider,
        access_token,
        refresh_token,
        expires_at,
        provider_account_id,
        base_url,
        auth_scheme,
        soft_limits: crate::soft_limit::SoftLimits::from_json(soft_limits_json.as_ref()),
        provider_settings,
    }))
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

/// Exchange the refresh token for a fresh access token and persist the rotated
/// pair. Caller MUST hold the account's refresh mutex. Returns the new access
/// token.
// Linear refresh flow with per-provider branches; complexity is per-branch
// error handling, not nesting.
#[allow(clippy::cognitive_complexity)]
async fn refresh_account(state: &AppState, acct: &Account) -> Result<String, StatusCode> {
    // Static-credential compatible accounts never refresh — the stored access
    // token (the bearer/api key) is forwarded verbatim.
    if acct.is_static() {
        return acct.access_token.clone().ok_or(StatusCode::BAD_GATEWAY);
    }
    let Some(refresh_token) = acct.refresh_token.as_deref() else {
        return Err(StatusCode::BAD_GATEWAY);
    };
    // Anthropic refreshes with a JSON body; OpenAI/Codex with a form-encoded
    // body (matches the codex CLI + CLIProxyAPI).
    let request = match acct.provider.as_str() {
        "anthropic" => {
            let body = serde_json::json!({
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": anthropic_client_id(),
            });
            state.http_client.post(anthropic_token_url()).json(&body)
        }
        "openai" => {
            let form = [
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", &openai_client_id()),
                ("scope", "openid profile email"),
            ];
            state.http_client.post(openai_token_url()).form(&form)
        }
        _ => return Err(StatusCode::BAD_GATEWAY),
    };
    let resp = request.send().await.map_err(|e| {
        tracing::error!(account = %acct.id, "oauth refresh transport error: {e}");
        StatusCode::BAD_GATEWAY
    })?;
    if !resp.status().is_success() {
        tracing::error!(account = %acct.id, status = %resp.status(), "oauth refresh rejected");
        return Err(StatusCode::BAD_GATEWAY);
    }
    let tok: TokenResponse = resp.json().await.map_err(|e| {
        tracing::error!(account = %acct.id, "oauth refresh decode error: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    let key = crate::crypto::vault_key();
    let enc_access = crate::crypto::encrypt(&tok.access_token, &key);
    // Refresh tokens are single-use → persist the rotated one when returned.
    let enc_refresh = tok.refresh_token.as_deref().map(|r| crate::crypto::encrypt(r, &key));
    let expires_at = tok.expires_in.map(|s| Utc::now() + chrono::Duration::seconds(s));

    let result = if let Some(enc_refresh) = enc_refresh {
        sqlx::query(
            "UPDATE account_providers SET encrypted_access_token = $2, \
                    encrypted_refresh_token = $3, expires_at = $4 WHERE id = $1",
        )
        .bind(acct.id)
        .bind(&enc_access)
        .bind(&enc_refresh)
        .bind(expires_at)
        .execute(&state.pool)
        .await
    } else {
        sqlx::query(
            "UPDATE account_providers SET encrypted_access_token = $2, expires_at = $3 WHERE id = $1",
        )
        .bind(acct.id)
        .bind(&enc_access)
        .bind(expires_at)
        .execute(&state.pool)
        .await
    };
    if let Err(e) = result {
        tracing::error!(account = %acct.id, "persist refreshed token failed: {e}");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    tracing::info!(account = %acct.id, "refreshed oauth access token");
    Ok(tok.access_token)
}

/// Return a valid access token for the account, refreshing if absent or within
/// the skew window. Serialized per account so concurrent sessions don't
/// double-refresh a single-use refresh token.
async fn current_access_token(state: &AppState, acct: &Account) -> Result<String, StatusCode> {
    // Static-credential compatible accounts: forward the stored credential
    // verbatim, no expiry tracking, no refresh.
    if acct.is_static() {
        return acct.access_token.clone().filter(|t| !t.is_empty()).ok_or(StatusCode::UNAUTHORIZED);
    }
    let fresh = matches!(&acct.access_token, Some(t) if !t.is_empty())
        && acct
            .expires_at
            // A NULL expires_at means we don't know when the token dies, so treat
            // it as stale and force a refresh — an OAuth access token left without
            // an expiry would otherwise be forwarded forever and die at ~1h,
            // causing account-wide 401s. Static accounts are handled
            // above and never reach here.
            .is_some_and(|exp| exp > Utc::now() + chrono::Duration::seconds(REFRESH_SKEW_SECS));
    if let (true, Some(t)) = (fresh, &acct.access_token) {
        return Ok(t.clone());
    }

    let lock = state
        .account_locks
        .entry(acct.id)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;

    // Re-read under the lock: another task may have just refreshed.
    if let Some(reloaded) = reload_account(state, acct.id).await {
        let still_fresh = matches!(&reloaded.access_token, Some(t) if !t.is_empty())
            && reloaded
                .expires_at
                // NULL expires_at => unknown lifetime => treat as stale and refresh.
                .is_some_and(|exp| exp > Utc::now() + chrono::Duration::seconds(REFRESH_SKEW_SECS));
        if let (true, Some(t)) = (still_fresh, reloaded.access_token.clone()) {
            return Ok(t);
        }
        return refresh_account(state, &reloaded).await;
    }
    refresh_account(state, acct).await
}

async fn reload_account(state: &AppState, id: Uuid) -> Option<Account> {
    let row: Option<ReloadRow> = sqlx::query_as(
        "SELECT provider, encrypted_access_token, encrypted_refresh_token, expires_at, \
                    provider_account_id, base_url, auth_scheme \
             FROM account_providers WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    let (provider, enc_access, enc_refresh, expires_at, provider_account_id, base_url, auth_scheme) =
        row?;
    let key = crate::crypto::vault_key();
    Some(Account {
        id,
        provider,
        access_token: enc_access.and_then(|e| crate::crypto::decrypt(&e, &key)),
        refresh_token: enc_refresh.and_then(|e| crate::crypto::decrypt(&e, &key)),
        expires_at,
        provider_account_id,
        base_url,
        auth_scheme,
        // `reload_account` only services the token-refresh / usage-fetch paths,
        // never the soft-limit gate (which runs off `resolve_account`), so the
        // caps are irrelevant here — default them, as are the gateway settings
        // (request shaping runs off `resolve_account` too).
        soft_limits: crate::soft_limit::SoftLimits::default(),
        provider_settings: None,
    })
}

/// `/gateway/anthropic/*path` — passthrough to api.anthropic.com.
/// Which side of the gateway rejected an authenticated request. The
/// two are easy to confuse from a worker's point of view — both surface as a
/// 401 — but they need opposite remedies, so we label every gateway 401 with
/// one of these in both the body message and the `x-cctui-auth-stage` header.
#[derive(Clone, Copy)]
enum AuthStage {
    /// cctui itself rejected the inbound `cctui_s_…` session token: unknown,
    /// revoked, or not bound to an account. The LLM login is irrelevant here.
    SessionToken,
    /// cctui accepted the session token and mapped it to an account, but the
    /// upstream LLM provider rejected that account's OAuth credentials (expired
    /// / revoked refresh token, failed refresh, upstream 401). The cctui token
    /// is fine; the account needs re-authenticating.
    ProviderOauth,
}

/// Build a labeled 401 response. The body uses the provider's native
/// error envelope so the CLI surfaces the message verbatim, and the
/// `x-cctui-auth-stage` header makes the cause machine-readable in logs/clients.
fn auth_error(stage: AuthStage, is_anthropic: bool) -> Response {
    let (stage_tag, message) = match stage {
        AuthStage::SessionToken => (
            "session-token",
            "cctui gateway rejected the session token: the cctui_s_ credential is \
             unknown, revoked, or not bound to an account. This is a cctui gateway \
             credential problem, NOT an LLM provider login problem — re-create or \
             re-resume the session to mint a fresh token.",
        ),
        AuthStage::ProviderOauth => (
            "provider-oauth",
            "cctui accepted the session token, but the upstream LLM provider returned \
             401 for the bound account's OAuth credentials. The cctui token is valid — \
             re-authenticate the LLM account in cctui.",
        ),
    };
    // Native error envelopes: Anthropic `{type:error, error:{type,message}}`;
    // OpenAI `{error:{message,type}}`. Both render the message in the CLI.
    let body = if is_anthropic {
        serde_json::json!({
            "type": "error",
            "error": { "type": "authentication_error", "message": message },
        })
    } else {
        serde_json::json!({
            "error": { "message": message, "type": "authentication_error" },
        })
    };
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("x-cctui-auth-stage", stage_tag)
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| StatusCode::UNAUTHORIZED.into_response())
}

pub async fn anthropic(
    State(state): State<AppState>,
    req: Request,
) -> Result<Response, StatusCode> {
    passthrough(state, req, "/gateway/anthropic", &anthropic_upstream()).await
}

/// `/gateway/openai/*path` — passthrough to api.openai.com.
pub async fn openai(State(state): State<AppState>, req: Request) -> Result<Response, StatusCode> {
    passthrough(state, req, "/gateway/openai", &openai_upstream()).await
}

/// `/gateway/fireworks/*path` — passthrough to Fireworks' OpenAI-compatible API.
///
/// A sibling route rather than a branch inside [`openai`]: the two differ in
/// upstream, in the worker env pair they are reached by, and in that this one
/// mutates the request (per-account [`FireworksSettings`]). Sharing the openai
/// route would put a per-account conditional on codex's hot path for nothing.
pub async fn fireworks(
    State(state): State<AppState>,
    req: Request,
) -> Result<Response, StatusCode> {
    passthrough(state, req, "/gateway/fireworks", &fireworks_upstream()).await
}

// Linear proxy pipeline (auth, account-resolve, refresh, forward, stream);
// complexity/length are per-stage handling, not nesting.
#[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
async fn passthrough(
    state: AppState,
    req: Request,
    prefix: &str,
    upstream_base: &str,
) -> Result<Response, StatusCode> {
    let is_anthropic = prefix.contains("anthropic");

    // The worker's bearer is the session token; map it to an account. A missing
    // bearer or one that doesn't resolve is a *cctui* rejection — distinguish it
    // from a provider rejection so the worker/operator knows which to fix.
    let Some(session_token) = req
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
    else {
        return Ok(auth_error(AuthStage::SessionToken, is_anthropic));
    };

    // Orphan-spam guard (slow-pool fix): a worker whose session→account binding
    // was lost retries forever, and every retry used to run DB lookups —
    // starving the pool and slowing the whole server. Fingerprint the token and,
    // if it is already flagged as a spamming orphan, drop the request *before*
    // touching the DB.
    let token_fp = crate::auth::sha256_hex(&session_token);
    if orphan_is_blocked(&state, &token_fp) {
        return Ok(auth_error(AuthStage::SessionToken, is_anthropic));
    }

    let acct = match resolve_account(&state, &session_token).await {
        Ok(Some(acct)) => {
            note_token_used(&state, &token_fp);
            acct
        }
        // Genuinely unknown/revoked/unbound token — a real orphan. Count it
        // toward the spam guard and reject as a cctui auth failure.
        Ok(None) => {
            note_orphan_401(&state, &token_fp);
            return Ok(auth_error(AuthStage::SessionToken, is_anthropic));
        }
        // The DB lookup itself failed (cold/starved pool during a server
        // restart, transient network). This is NOT an orphan — a valid bound
        // token can land here while the pool warms up. Returning a retryable
        // 503 (and crucially NOT feeding the orphan-spam block) keeps a server
        // restart from poisoning live tokens for 300s.
        Err(e) => {
            tracing::warn!(
                stage = "session-token",
                error = %e,
                "gateway token resolution failed transiently (DB) — returning 503, not orphaning"
            );
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
    };

    // Soft limit: cap cctui's own share of the account's usage windows
    // so it leaves headroom for the human sharing the subscription. Only the
    // configured windows gate; bypass near reset.
    //
    // The original code read ONLY the usage cache, which is warmed solely by the
    // accounts-page route. Headless dispatch never opens that page, so the cache
    // was perpetually cold → `evaluate_soft_limit(None, …)` → Allow on every
    // request, and a capped account would run all the way to 100% (the regression
    // we hit). So on the dispatch path we refresh the cache from upstream when it
    // is cold/stale (throttled by the same TTL so we never spam Anthropic's
    // rate-limited endpoint), and only then evaluate. Fetch errors fail open.
    // A `CctuiAgent` child carries its own `session_usd` cap, which the account's
    // stored limits know nothing about. Overlay it here; the map is empty on the
    // ordinary path, so this costs a lock-free length check per request.
    let effective_limits = session_budget_limits(&state, &acct, &session_token).await;
    if !effective_limits.is_unset() {
        let cached = usage_for_soft_limit(&state, acct.id).await;
        let mut windows =
            cached.as_ref().map(crate::soft_limit::normalize_usage_windows).unwrap_or_default();
        // The per-session budget is session-scoped, so it can't come from the
        // per-account usage cache — resolve it here, and only when one is set.
        if effective_limits.limits.contains_key(crate::soft_limit::KEY_SESSION_USD)
            && let Some(session_id) = session_id_for_token(&state, &session_token).await
            && let Some(spent) = session_spend_usd(&state, acct.id, &session_id).await
        {
            windows.push(crate::soft_limit::usd_window(
                crate::soft_limit::KEY_SESSION_USD,
                spent,
                None,
            ));
        }
        if let crate::soft_limit::Decision::Block { retry_after_secs, reason, .. } =
            crate::soft_limit::evaluate_soft_limit(&windows, &effective_limits, Utc::now())
        {
            tracing::info!(account = %acct.id, retry_after_secs, "soft limit hit: {reason}");
            // Surface the block as a per-session signal so the webui can offer
            // "continue on another account". Best-effort + dedup'd.
            if let Some((session_id, account_name)) =
                session_and_account_name_for_token(&state, &session_token).await
            {
                mark_soft_limit_block(
                    &state,
                    &session_id,
                    acct.id,
                    &account_name,
                    &reason,
                    retry_after_secs,
                )
                .await;
            }
            let resp = Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header(http::header::RETRY_AFTER, retry_after_secs.to_string())
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::json!({ "error": reason }).to_string()))
                .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
            return Ok(resp);
        }
    }

    // The session token is valid (resolved above); a failure to obtain an
    // upstream access token here is a provider-credential problem (no/expired
    // refresh token, failed refresh) — label it as such.
    let Ok(access_token) = current_access_token(&state, &acct).await else {
        tracing::warn!(account = %acct.id, stage = "provider-oauth", "gateway 401: no upstream access token for account");
        flag_account_reauth(&state, acct.id, "no upstream access token (refresh failed)");
        return Ok(auth_error(AuthStage::ProviderOauth, is_anthropic));
    };

    // Per-account upstream: a compatible endpoint overrides the
    // built-in upstream with its stored `base_url`; native subscription accounts
    // fall back to the built-in `api.anthropic.com`/`chatgpt.com`.
    let upstream =
        acct.base_url.as_deref().filter(|u| !u.trim().is_empty()).unwrap_or(upstream_base);

    // Build the upstream URL: strip the gateway prefix, keep path + query.
    let path = req.uri().path();
    let tail = path.strip_prefix(prefix).unwrap_or(path);
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let url = format!("{}{tail}{query}", upstream.trim_end_matches('/'));

    let method = reqwest::Method::from_bytes(req.method().as_str().as_bytes())
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Preserve every client header verbatim except hop-by-hop + the bearer we
    // are swapping and the Host (reqwest sets it from the upstream URL).
    let mut headers = HeaderMap::new();
    for (name, value) in req.headers() {
        let n = name.as_str().to_ascii_lowercase();
        if matches!(n.as_str(), "authorization" | "host" | "content-length" | "connection") {
            continue;
        }
        if let (Ok(hn), Ok(hv)) = (
            reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            headers.insert(hn, hv);
        }
    }
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&format!("Bearer {access_token}"))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    // ChatGPT-backed Codex requests must carry the account id upstream.
    if let Some(account_id) = acct.provider_account_id.as_deref()
        && let Ok(hv) = reqwest::header::HeaderValue::from_str(account_id)
    {
        headers.insert("chatgpt-account-id", hv);
    }

    // Fireworks: the account's settings shape the request here, where the real
    // key lives — a worker can neither supply nor defeat them.
    let fireworks = (Family::from_provider(&acct.provider) == Family::Fireworks)
        .then(|| FireworksSettings::resolve(acct.provider_settings.as_ref()));
    let affinity_session = match fireworks.as_ref() {
        Some(fw) if fw.session_affinity => session_id_for_token(&state, &session_token).await,
        _ => None,
    };
    if let Some(sid) = affinity_session.as_deref()
        && let Ok(hv) = reqwest::header::HeaderValue::from_str(sid)
    {
        headers.insert("x-session-affinity", hv);
    }

    // Langfuse tracing sink: only when configured AND this call is
    // sampled do we reconstruct the bodies — otherwise the gateway stays a pure
    // zero-copy passthrough (request streamed, response streamed). When tracing,
    // we buffer the request body (it is the prompt, already fully in flight) so it
    // can be both forwarded upstream and used as the generation input.
    let langfuse = state.langfuse.clone().filter(|lf| lf.should_sample());
    let trace_session_id =
        if langfuse.is_some() { session_id_for_token(&state, &session_token).await } else { None };

    // Traced calls must come back identity-encoded: the response tee buffers the
    // raw bytes for SSE reconstruction, and a gzip/zstd body defeats it — every
    // trace then lands in Langfuse without usage and gets mis-costed by the
    // tokenizer fallback. reqwest is built without decompression
    // features, so dropping the client's `accept-encoding` yields a plain body.
    if langfuse.is_some() {
        headers.remove(reqwest::header::ACCEPT_ENCODING);
    }

    // Stream the request body through without buffering (default), OR buffer it
    // once for the trace input when Langfuse is sampling this call.
    let mut request_model: Option<String> = None;
    let (upstream_body, traced_request) = if langfuse.is_some() || fireworks.is_some() {
        let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
            .await
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        let mut parsed = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
        request_model = parsed
            .as_ref()
            .and_then(|r| r.get("model"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let body = match (fireworks.as_ref(), parsed.as_mut()) {
            (Some(fw), Some(json)) => {
                fw.apply_body(json, affinity_session.as_deref());
                reqwest::Body::from(json.to_string())
            }
            _ => reqwest::Body::from(bytes),
        };
        (body, parsed.filter(|_| langfuse.is_some()))
    } else {
        let body_stream = req.into_body().into_data_stream();
        (reqwest::Body::wrap_stream(body_stream), None)
    };

    let upstream = state
        .http_client
        .request(method, &url)
        .headers(headers)
        .body(upstream_body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!(account = %acct.id, "gateway upstream error: {e}");
            StatusCode::BAD_GATEWAY
        })?;

    // Opportunistic stats: request count + response byte count (no buffering).
    let resp_len = i64::try_from(upstream.content_length().unwrap_or(0)).unwrap_or(i64::MAX);
    let acct_id = acct.id;
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = sqlx::query(
            "UPDATE account_providers SET request_count = request_count + 1, \
                    bytes_transferred = bytes_transferred + $2, last_used_at = now() \
             WHERE id = $1",
        )
        .bind(acct_id)
        .bind(resp_len)
        .execute(&pool)
        .await;
    });

    // Mirror status + headers back to the client untouched (retry-after, 429,
    // 529, SSE content-type — all verbatim) and stream the body.
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    // The session token was accepted by cctui (we got this far), so a 401 from
    // the upstream provider means the account's OAuth credentials are bad, not
    // the cctui token. Replace the opaque upstream 401 with a labeled one so the
    // worker/operator re-authenticates the account rather than the session.
    if status == StatusCode::UNAUTHORIZED {
        tracing::warn!(account = %acct.id, stage = "provider-oauth", "gateway 401: upstream provider rejected account credentials");
        flag_account_reauth(&state, acct.id, "upstream provider rejected account credentials");
        return Ok(auth_error(AuthStage::ProviderOauth, is_anthropic));
    }

    // A successful upstream call clears any soft-limit block on this session:
    // after the user switches accounts (or a window resets) the next
    // 2xx dismisses the banner. Only touch the DB when something is actually
    // blocked, and reuse the trace lookup when Langfuse already resolved it.
    if status.is_success() && !state.soft_limit_blocked.is_empty() {
        let session_id = match &trace_session_id {
            Some(sid) => Some(sid.clone()),
            None => session_id_for_token(&state, &session_token).await,
        };
        if let Some(sid) = session_id {
            clear_soft_limit_block(&state, &sid).await;
        }
    }
    // A successful upstream call means the account's credentials are good again —
    // clear any reauth flag. Gated in-memory, so this is free unless the
    // account was actually flagged.
    if status.is_success() {
        clear_account_reauth(&state, acct.id);
    }
    let mut builder = Response::builder().status(status);
    for (name, value) in upstream.headers() {
        let n = name.as_str().to_ascii_lowercase();
        if matches!(n.as_str(), "connection" | "transfer-encoding" | "content-length") {
            continue;
        }
        if let (Ok(hn), Ok(hv)) = (
            axum::http::HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            builder = builder.header(hn, hv);
        }
    }
    // Fireworks meters per token, so its usage must be recorded from the
    // response itself: the `usage` object (JSON body or terminal SSE frame) plus
    // the two headers, which are what the provider bills against. Read the
    // headers now, before the body is consumed.
    let usage_session = match (&fireworks, status.is_success()) {
        (Some(_), true) => match affinity_session.clone().or_else(|| trace_session_id.clone()) {
            Some(sid) => Some(sid),
            None => session_id_for_token(&state, &session_token).await,
        },
        _ => None,
    };
    let usage_headers = usage_session.as_ref().map(|_| {
        let header = |name: &str| {
            upstream.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned)
        };
        (header("fireworks-prompt-tokens"), header("fireworks-cached-prompt-tokens"))
    });

    // Fast path (nothing to observe): stream the response straight through.
    if langfuse.is_none() && usage_session.is_none() {
        let resp_stream = upstream.bytes_stream();
        return builder.body(Body::from_stream(resp_stream)).map_err(|e| {
            tracing::error!("gateway response build error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        });
    }

    // Observed path: tee the response body. Each chunk is forwarded to the client
    // verbatim AND copied into an accumulator task over a bounded channel. The
    // copy is best-effort — if the task lags, `try_send` drops the chunk
    // (we lose the trace/usage, never the proxied bytes). When the upstream stream
    // ends the channel closes and the task reconstructs the trace and the metered
    // usage. Nothing here blocks or delays the client stream.
    let ctx = crate::langfuse::TraceContext {
        session_id: trace_session_id,
        account_id: Some(acct.id.to_string()),
        model: request_model.clone(),
    };
    // Fireworks speaks the OpenAI wire protocol, so it reconstructs as openai.
    let is_openai = Family::from_provider(&acct.provider) != Family::Anthropic;
    let pool = state.pool.clone();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    tokio::spawn(async move {
        let mut buf = Vec::new();
        while let Some(chunk) = rx.recv().await {
            buf.extend_from_slice(&chunk);
        }
        if let (Some(session_id), Some((prompt_hdr, cached_hdr))) = (usage_session, usage_headers)
            && let Some(captured) = crate::cost::parse_fireworks_usage(
                &buf,
                prompt_hdr.as_deref(),
                cached_hdr.as_deref(),
            )
        {
            record_fireworks_usage(pool, session_id, request_model, captured).await;
        }
        if let Some(langfuse) = langfuse {
            let (output, usage) = if is_openai {
                crate::langfuse::reconstruct_openai(&buf)
            } else {
                crate::langfuse::reconstruct_anthropic(&buf)
            };
            langfuse.trace(crate::langfuse::TracePayload {
                ctx,
                request: traced_request,
                output,
                usage,
            });
        }
    });

    let resp_stream = upstream.bytes_stream().map(move |chunk| {
        if let Ok(bytes) = &chunk {
            // Drop on backpressure rather than block the proxied response.
            let _ = tx.try_send(bytes.to_vec());
        }
        chunk
    });
    builder.body(Body::from_stream(resp_stream)).map_err(|e| {
        tracing::error!("gateway response build error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// Anthropic's free OAuth usage endpoint. Returns subscription window
/// utilization (5h session + 7d weekly) WITHOUT consuming any tokens — it is not
/// an inference call. Undocumented + caveat-accepted (same class as the OAuth
/// token endpoints above); overridable via env to track upstream changes.
pub fn anthropic_usage_url() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_USAGE_URL")
        .unwrap_or_else(|_| "https://api.anthropic.com/api/oauth/usage".into())
}

/// `User-Agent` the usage endpoint requires (`claude-code/<version>`). Without a
/// claude-code UA the endpoint drops the caller into an aggressively rate-limited
/// bucket (persistent 429s). Overridable so we can bump the version it expects
/// without a code redeploy.
pub fn anthropic_usage_user_agent() -> String {
    std::env::var("CCTUI_ANTHROPIC_USAGE_USER_AGENT").unwrap_or_else(|_| "claude-code/2.1.0".into())
}

/// OpenAI/codex's real per-account usage endpoint. Returns the `ChatGPT`
/// backend's actual 5h/7d rate-limit windows (`rate_limit.primary_window` /
/// `secondary_window`) — the same numbers `codex /status` shows. Works cookieless
/// with just the account's OAuth Bearer + `chatgpt-account-id` header, both of
/// which we already hold per account. Overridable via env to track upstream moves.
pub fn openai_usage_url() -> String {
    std::env::var("CCTUI_OPENAI_USAGE_URL")
        .unwrap_or_else(|_| "https://chatgpt.com/backend-api/wham/usage".into())
}

/// Map the `ChatGPT` `wham/usage` body to our provider-agnostic `{five_hour,
/// seven_day}` usage shape. `primary_window` is the 5h window,
/// `secondary_window` the 7d one; `used_percent` → `utilization`, `reset_at`
/// (unix epoch seconds) → `resets_at` (rfc3339). Returns `None` if the body has
/// no `rate_limit` or is missing either window, so the caller can fall back to the
/// local tally.
fn map_wham_usage(body: &serde_json::Value) -> Option<serde_json::Value> {
    let rate_limit = body.get("rate_limit")?;
    let window = |w: &serde_json::Value| {
        let utilization = w.get("used_percent").and_then(serde_json::Value::as_f64);
        let resets_at = w
            .get("reset_at")
            .and_then(serde_json::Value::as_i64)
            .and_then(|s| chrono::DateTime::<chrono::Utc>::from_timestamp(s, 0))
            .map(|dt| dt.to_rfc3339());
        serde_json::json!({ "utilization": utilization, "resets_at": resets_at })
    };
    let five_hour = window(rate_limit.get("primary_window")?);
    let seven_day = window(rate_limit.get("secondary_window")?);
    Some(serde_json::json!({ "five_hour": five_hour, "seven_day": seven_day }))
}

/// Fetch an OpenAI/codex account's real 5h/7d usage from `wham/usage`.
///
/// Uses only stored OAuth data: a fresh Bearer via [`current_access_token`] and the
/// `chatgpt-account-id` from `acct.provider_account_id`. Returns `None` (so the
/// caller falls back to the local tally) when the account has no account-id, the
/// token can't be refreshed, the call fails/errs, or the body has no rate-limit
/// windows — logging the reason in each case. No inference request, costs no tokens.
async fn fetch_openai_usage(state: &AppState, acct: &Account) -> Option<serde_json::Value> {
    let account_id = acct.provider_account_id.as_deref()?;
    let access_token = current_access_token(state, acct).await.ok()?;
    let resp = state
        .http_client
        .get(openai_usage_url())
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {access_token}"))
        .header("chatgpt-account-id", account_id)
        .header(reqwest::header::ACCEPT, "*/*")
        .send()
        .await
        .map_err(|e| tracing::warn!(account = %acct.id, "openai usage transport error: {e}"))
        .ok()?;
    if !resp.status().is_success() {
        tracing::warn!(account = %acct.id, status = %resp.status(), "openai usage rejected");
        return None;
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| tracing::warn!(account = %acct.id, "openai usage decode error: {e}"))
        .ok()?;
    map_wham_usage(&body)
}

/// Whether the soft-limit check must refresh usage from upstream before deciding.
///
/// A cold (`None`) or stale cache must trigger a refresh — treating a cold
/// cache as "no data → allow" would let a capped account run to 100% whenever
/// no human was viewing it.
fn usage_cache_stale(entry_age: Option<std::time::Duration>, ttl: std::time::Duration) -> bool {
    entry_age.is_none_or(|age| age >= ttl)
}

/// Usage payload for the soft-limit check on the dispatch hot path.
///
/// Serves the per-account usage cache, refreshing it from upstream when it is
/// cold or older than [`accounts::USAGE_CACHE_TTL`] (≤ once per TTL per account,
/// so the rate-limited endpoint is never spammed). On a fetch error it falls back
/// to the last cached value, else `None` (fail open). Unlike the accounts-page
/// route, this exists so the cap holds even when no human is viewing the account.
pub async fn usage_for_soft_limit(state: &AppState, account_id: Uuid) -> Option<serde_json::Value> {
    let ttl = crate::routes::accounts::USAGE_CACHE_TTL.to_std().unwrap_or_default();
    let entry_age = state.account_usage_cache.get(&account_id).map(|h| h.fetched_at.elapsed());
    if !usage_cache_stale(entry_age, ttl) {
        return state.account_usage_cache.get(&account_id).and_then(|h| h.usage.clone());
    }
    fetch_account_usage(state, account_id).await.map_or_else(
        // Upstream hiccup (429/refresh fail): fall back to the last cached value.
        |_| state.account_usage_cache.get(&account_id).and_then(|h| h.usage.clone()),
        |usage| {
            state.account_usage_cache.insert(
                account_id,
                crate::state::CachedUsage {
                    fetched_at: std::time::Instant::now(),
                    usage: usage.clone(),
                },
            );
            usage
        },
    )
}

/// Fetch the Anthropic OAuth usage windows for an account.
///
/// Reloads + decrypts the account, ensures a fresh access token (refreshing under
/// the per-account mutex if needed), and calls Anthropic's free usage endpoint.
/// Returns:
///   * `Ok(Some(json))` — anthropic account (fetched upstream) or OpenAI/codex
///     account (metered locally from `session_token_usage`)
///   * `Ok(None)` — no such account
///   * `Err(status)` — token refresh failed or upstream rejected (e.g. 429)
///
/// This makes NO inference request and costs no tokens. Callers MUST throttle it
/// (the endpoint rate-limits per access token); see the usage cache in the route.
pub async fn fetch_account_usage(
    state: &AppState,
    account_id: Uuid,
) -> Result<Option<serde_json::Value>, StatusCode> {
    let Some(acct) = reload_account(state, account_id).await else { return Ok(None) };
    // Pay-per-token: dollars, not percent of a subscription window. Never asks
    // the provider's billing API — cctui budgets its own metered spend.
    if Family::from_provider(&acct.provider) == Family::Fireworks {
        return fireworks_usd_windows(state, account_id).await;
    }
    if acct.provider != "anthropic" {
        // OpenAI/codex accounts: read the ChatGPT backend's REAL 5h/7d rate-limit
        // windows — the same numbers `codex /status` shows, keyed on the
        // stored OAuth Bearer + chatgpt-account-id. Fall back to the local token
        // tally only when that call can't produce windows (no account-id,
        // token refresh fail, upstream error, or a body without rate limits), so
        // freshly-enrolled / API-key accounts still render something.
        if acct.provider == "openai"
            && let Some(usage) = fetch_openai_usage(state, &acct).await
        {
            return Ok(Some(usage));
        }
        tracing::info!(account = %account_id, "openai usage unavailable; falling back to local token tally");
        return local_usage_windows(state, account_id).await;
    }
    let access_token = current_access_token(state, &acct).await?;
    let resp = state
        .http_client
        .get(anthropic_usage_url())
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {access_token}"))
        .header(reqwest::header::USER_AGENT, anthropic_usage_user_agent())
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(account = %account_id, "usage fetch transport error: {e}");
            StatusCode::BAD_GATEWAY
        })?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if !status.is_success() {
        tracing::warn!(account = %account_id, %status, "usage fetch rejected");
        return Err(status);
    }
    let json: serde_json::Value = resp.json().await.map_err(|e| {
        tracing::warn!(account = %account_id, "usage decode error: {e}");
        StatusCode::BAD_GATEWAY
    })?;
    Ok(Some(json))
}

/// Per-window token budget an OpenAI/codex account's local utilization is measured
/// against. Codex exposes no free usage endpoint, so we can't read a real
/// quota — utilization is `tokens_used_in_window / budget`. The budgets are
/// arbitrary-but-tunable so the soft-limit % stays meaningful per plan; override
/// via env to match whatever ChatGPT/codex tier the account is on.
fn openai_5h_token_budget() -> i64 {
    std::env::var("CCTUI_OPENAI_5H_TOKEN_BUDGET")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(8_000_000)
}

fn openai_7d_token_budget() -> i64 {
    std::env::var("CCTUI_OPENAI_7D_TOKEN_BUDGET")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(80_000_000)
}

/// Compute provider-agnostic 5h/7d usage windows from cctui's own recorded token
/// usage. Sums every token kind across the account's sessions inside each
/// rolling window and divides by the configured budget for a utilization percent.
/// `resets_at` is when the oldest contributing usage ages out of the window (i.e.
/// when capacity frees up). Emits the same JSON shape as the Anthropic usage
/// endpoint so [`crate::soft_limit`] and the accounts UI consume it unchanged.
async fn local_usage_windows(
    state: &AppState,
    account_id: Uuid,
) -> Result<Option<serde_json::Value>, StatusCode> {
    let five_hour = local_window(state, account_id, "5 hours", openai_5h_token_budget()).await?;
    let seven_day = local_window(state, account_id, "7 days", openai_7d_token_budget()).await?;
    Ok(Some(serde_json::json!({
        "five_hour": five_hour,
        "seven_day": seven_day,
    })))
}

/// One rolling window's `{utilization, resets_at}` from `session_token_usage`.
async fn local_window(
    state: &AppState,
    account_id: Uuid,
    interval: &str,
    budget: i64,
) -> Result<serde_json::Value, StatusCode> {
    // SUM() over bigint returns NUMERIC; cast back to bigint or sqlx fails to
    // decode into i64. `oldest` is the earliest in-window usage row.
    let row: (i64, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT COALESCE(SUM(stu.input_tokens + stu.output_tokens \
                + stu.cache_read_tokens + stu.cache_creation_tokens), 0)::bigint AS tokens, \
                MIN(stu.created_at) AS oldest \
         FROM session_tokens st \
         JOIN session_token_usage stu ON stu.session_id = st.session_id \
         WHERE st.account_id = $1 AND stu.created_at >= now() - $2::interval",
    )
    .bind(account_id)
    .bind(interval)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        tracing::warn!(account = %account_id, "local usage query error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let (tokens, oldest) = row;
    let utilization = (tokens as f64 / budget as f64) * 100.0;
    // The window frees up when the oldest contributing row falls out of it.
    let resets_at = oldest.map(|t| {
        let secs = if interval == "5 hours" { 5 * 3600 } else { 7 * 86400 };
        (t + chrono::Duration::seconds(secs)).to_rfc3339()
    });
    Ok(serde_json::json!({
        "utilization": utilization,
        "resets_at": resets_at,
    }))
}

/// The provider row's model catalog — the only source of prices.
async fn account_catalog(state: &AppState, account_id: Uuid) -> Option<serde_json::Value> {
    sqlx::query_scalar::<_, Option<serde_json::Value>>(
        "SELECT models FROM account_providers WHERE id = $1",
    )
    .bind(account_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .flatten()
}

/// One raw usage-tally row: model, non-cached input, cached input, output,
/// oldest contributing timestamp.
type TallyRow = (Option<String>, i64, i64, i64, Option<chrono::DateTime<Utc>>);

/// One model's tallied usage in a window, with the oldest contributing row.
type ModelTally = (Option<String>, crate::cost::TokenUsage, Option<chrono::DateTime<Utc>>);

/// Per-model token tallies for one account, restricted by an SQL predicate on
/// `stu`/`st` bound to `$2`.
async fn model_tallies(
    state: &AppState,
    account_id: Uuid,
    filter: &str,
    bind: &str,
) -> Vec<ModelTally> {
    let sql = format!(
        "SELECT stu.model, \
                COALESCE(SUM(stu.input_tokens + stu.cache_creation_tokens), 0)::bigint, \
                COALESCE(SUM(stu.cache_read_tokens), 0)::bigint, \
                COALESCE(SUM(stu.output_tokens), 0)::bigint, \
                MIN(stu.created_at) \
         FROM session_tokens st \
         JOIN session_token_usage stu ON stu.session_id = st.session_id \
         WHERE st.account_id = $1 AND {filter} \
         GROUP BY stu.model"
    );
    let rows: Vec<TallyRow> = sqlx::query_as(&sql)
        .bind(account_id)
        .bind(bind)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(account = %account_id, "usd usage query error: {e}");
            Vec::new()
        });
    rows.into_iter()
        .map(|(model, input, cached, output, oldest)| {
            (model, crate::cost::TokenUsage { input, cached_input: cached, output }, oldest)
        })
        .collect()
}

fn priced(catalog: Option<&serde_json::Value>, rows: &[ModelTally]) -> f64 {
    let tallies: Vec<_> = rows.iter().map(|(m, u, _)| (m.clone(), *u)).collect();
    crate::cost::tallies_cost_usd(catalog, &tallies)
}

/// USD spent by one session under this account, priced from the catalog.
async fn session_spend_usd(state: &AppState, account_id: Uuid, session_id: &str) -> Option<f64> {
    let catalog = account_catalog(state, account_id).await;
    let rows = model_tallies(state, account_id, "st.session_id = $2", session_id).await;
    Some(priced(catalog.as_ref(), &rows))
}

/// Rolling dollar-spend windows for a pay-per-token account, computed purely
/// from cctui's own recorded usage priced against the account's catalog. Emitted
/// in the same fixed-field shape the rest of the usage pipeline consumes.
async fn fireworks_usd_windows(
    state: &AppState,
    account_id: Uuid,
) -> Result<Option<serde_json::Value>, StatusCode> {
    let catalog = account_catalog(state, account_id).await;
    let mut out = serde_json::Map::new();
    for (key, interval, secs) in [
        (crate::soft_limit::KEY_USD_5H, "5 hours", 5 * 3600_i64),
        (crate::soft_limit::KEY_USD_7D, "7 days", 7 * 86400),
    ] {
        let rows =
            model_tallies(state, account_id, "stu.created_at >= now() - $2::interval", interval)
                .await;
        let resets_at = rows
            .iter()
            .filter_map(|(_, _, oldest)| *oldest)
            .min()
            .map(|t| (t + chrono::Duration::seconds(secs)).to_rfc3339());
        out.insert(
            key.to_owned(),
            serde_json::json!({
                "amount_usd": priced(catalog.as_ref(), &rows),
                "resets_at": resets_at,
            }),
        );
    }
    Ok(Some(serde_json::Value::Object(out)))
}

/// Persist one Fireworks response's usage. Idempotent on
/// `(session_id, message_id)`; a response without an upstream id gets a
/// synthetic one, so a retry of the same call is counted once per response, not
/// per attempt.
async fn record_fireworks_usage(
    pool: sqlx::PgPool,
    session_id: String,
    model: Option<String>,
    captured: crate::cost::CapturedUsage,
) {
    let message_id =
        captured.message_id.unwrap_or_else(|| format!("fw-{}", uuid::Uuid::new_v4().simple()));
    let u = captured.usage;
    if let Err(e) = sqlx::query(
        "INSERT INTO session_token_usage \
             (session_id, message_id, input_tokens, output_tokens, cache_read_tokens, model) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (session_id, message_id) DO NOTHING",
    )
    .bind(&session_id)
    .bind(&message_id)
    .bind(u.input)
    .bind(u.output)
    .bind(u.cached_input)
    .bind(model)
    .execute(&pool)
    .await
    {
        tracing::warn!(session = %session_id, "fireworks usage record failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthStage, Family, FireworksSettings, OrphanSpamMap, apply_anthropic_cache_defaults,
        apply_gateway_env, auth_error, bump_orphan_401, clear_orphan_fingerprint, map_wham_usage,
        merge_session_budget, orphan_is_blocked_at, resolve_catalog_model, ttl_hours_from,
        usage_cache_stale,
    };
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    #[test]
    fn anthropic_1h_cache_flag_defaults_on_and_is_overridable() {
        let mut env = BTreeMap::new();
        apply_anthropic_cache_defaults(&mut env);
        assert_eq!(env.get("ENABLE_PROMPT_CACHING_1H").map(String::as_str), Some("1"));

        let mut off = BTreeMap::new();
        off.insert("ENABLE_PROMPT_CACHING_1H".to_string(), "0".to_string());
        apply_anthropic_cache_defaults(&mut off);
        assert_eq!(off.get("ENABLE_PROMPT_CACHING_1H").map(String::as_str), Some("0"));
    }

    #[test]
    fn anthropic_1h_cache_flag_is_curated_in_catalog() {
        let e = crate::settings_catalog::catalog()
            .env("ENABLE_PROMPT_CACHING_1H")
            .expect("1h cache flag curated in the catalog");
        assert!(e.tag.account_exposable());
        assert!(!crate::settings_catalog::catalog().env_denylisted("ENABLE_PROMPT_CACHING_1H"));
    }

    #[test]
    fn wham_usage_maps_to_five_and_seven_windows() {
        // Real-shaped `wham/usage` body: primary=5h, secondary=7d.
        let body = serde_json::json!({
            "rate_limit": {
                "primary_window":   { "used_percent": 1,  "limit_window_seconds": 18_000,  "reset_at": 1_782_955_425i64 },
                "secondary_window": { "used_percent": 14, "limit_window_seconds": 604_800, "reset_at": 1_783_403_309i64 },
            }
        });
        let mapped = map_wham_usage(&body).expect("rate_limit present");
        assert_eq!(mapped["five_hour"]["utilization"].as_f64(), Some(1.0));
        assert_eq!(mapped["seven_day"]["utilization"].as_f64(), Some(14.0));
        // Epoch seconds → rfc3339 (stable server reset, not client-drifted).
        assert_eq!(mapped["five_hour"]["resets_at"].as_str(), Some("2026-07-02T01:23:45+00:00"));
        assert_eq!(mapped["seven_day"]["resets_at"].as_str(), Some("2026-07-07T05:48:29+00:00"));
    }

    #[test]
    fn wham_usage_none_without_rate_limit() {
        // No rate_limit (or a partial body) → None so the caller falls back local.
        assert!(map_wham_usage(&serde_json::json!({ "user_id": "u" })).is_none());
        assert!(
            map_wham_usage(&serde_json::json!({ "rate_limit": { "primary_window": {} } }))
                .is_none()
        );
    }

    #[test]
    fn orphan_spam_blocks_after_threshold_and_skips_db() {
        let map = OrphanSpamMap::new();
        let now = Instant::now();
        let window = Duration::from_secs(60);
        let block = Duration::from_secs(300);
        let fp = "deadbeef";

        // Below threshold: counts climb, never blocked.
        for i in 1..3 {
            let (count, newly) = bump_orphan_401(&map, fp, now, 3, window, block);
            assert_eq!(count, i);
            assert!(!newly);
            assert!(!orphan_is_blocked_at(&map, fp, now));
        }
        // Crossing the threshold flags it exactly once.
        let (count, newly) = bump_orphan_401(&map, fp, now, 3, window, block);
        assert_eq!(count, 3);
        assert!(newly, "should flag on the threshold-crossing call");
        assert!(orphan_is_blocked_at(&map, fp, now));

        // Still blocked mid-block-window, and re-flagging does not re-fire.
        let mid = now + Duration::from_secs(120);
        assert!(orphan_is_blocked_at(&map, fp, mid));
        let (_, newly_again) = bump_orphan_401(&map, fp, mid, 3, window, block);
        assert!(!newly_again);

        // After the block expires, the fingerprint is clear again.
        let after = now + block + Duration::from_secs(1);
        assert!(!orphan_is_blocked_at(&map, fp, after));
    }

    #[test]
    fn orphan_spam_unknown_fingerprint_is_not_blocked() {
        let map = OrphanSpamMap::new();
        assert!(!orphan_is_blocked_at(&map, "nope", Instant::now()));
    }

    #[test]
    fn rebind_clears_a_blocked_fingerprint_immediately() {
        // An account rebind reuses the SAME token string, so a
        // fingerprint blocked while the binding was broken must be cleared on
        // rebind — otherwise the just-fixed binding keeps 401ing for the
        // remainder of the (up to 300s) block window.
        let map = OrphanSpamMap::new();
        let now = Instant::now();
        let window = Duration::from_secs(60);
        let block = Duration::from_secs(300);
        let fp = "deadbeef";
        for _ in 0..3 {
            bump_orphan_401(&map, fp, now, 3, window, block);
        }
        assert!(orphan_is_blocked_at(&map, fp, now), "precondition: fp is blocked");

        clear_orphan_fingerprint(&map, fp);
        // No longer blocked — the next gateway request goes back to the DB
        // lookup instead of being dropped.
        assert!(!orphan_is_blocked_at(&map, fp, now));
        // And the window restarts from scratch: one fresh 401 doesn't re-block.
        let (count, newly) = bump_orphan_401(&map, fp, now, 3, window, block);
        assert_eq!(count, 1);
        assert!(!newly);
        assert!(!orphan_is_blocked_at(&map, fp, now));
    }

    #[test]
    fn auth_error_distinguishes_session_token_from_provider_oauth() {
        // the two 401s must be tellable apart — different stage header
        // and a message naming which credential to fix.
        let session = auth_error(AuthStage::SessionToken, true);
        let provider = auth_error(AuthStage::ProviderOauth, true);
        assert_eq!(session.status(), axum::http::StatusCode::UNAUTHORIZED);
        assert_eq!(provider.status(), axum::http::StatusCode::UNAUTHORIZED);
        assert_eq!(session.headers().get("x-cctui-auth-stage").unwrap(), "session-token");
        assert_eq!(provider.headers().get("x-cctui-auth-stage").unwrap(), "provider-oauth");
    }

    #[test]
    fn auth_error_uses_native_error_envelope_per_family() {
        // Anthropic: top-level `type:error`; OpenAI: bare `error` object. The CLI
        // only renders the message when the envelope matches its provider.
        let anthropic = auth_error(AuthStage::SessionToken, true);
        let openai = auth_error(AuthStage::SessionToken, false);
        assert_eq!(
            anthropic.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            openai.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }

    #[test]
    fn cold_usage_cache_is_stale() {
        // No cached usage must force a refresh, not be
        // treated as "no data → allow". This is what let a capped account hit 100%
        // on the headless dispatch path where the accounts page never warms it.
        assert!(usage_cache_stale(None, Duration::from_secs(180)));
    }

    #[test]
    fn fresh_usage_cache_is_not_stale() {
        assert!(!usage_cache_stale(Some(Duration::from_secs(10)), Duration::from_secs(180)));
    }

    #[test]
    fn expired_usage_cache_is_stale() {
        // At/over the TTL → refresh (so a capped account re-checks within one TTL).
        assert!(usage_cache_stale(Some(Duration::from_secs(180)), Duration::from_secs(180)));
        assert!(usage_cache_stale(Some(Duration::from_secs(600)), Duration::from_secs(180)));
    }

    #[test]
    fn family_from_provider_maps_native_and_compatible() {
        // both native and `-compatible` providers collapse to a family.
        assert!(matches!(Family::from_provider("anthropic"), Family::Anthropic));
        assert!(matches!(Family::from_provider("anthropic-compatible"), Family::Anthropic));
        assert!(matches!(Family::from_provider("openai"), Family::Openai));
        assert!(matches!(Family::from_provider("openai-compatible"), Family::Openai));
    }

    #[test]
    fn family_from_adapter_is_the_spawn_resolution_key() {
        // the adapter id names the harness family spawn resolves the
        // account's provider row by.
        assert!(matches!(Family::from_adapter("codex"), Family::Openai));
        assert!(matches!(Family::from_adapter("codex-foo"), Family::Openai));
        assert!(matches!(Family::from_adapter("claude-code"), Family::Anthropic));
    }

    #[test]
    fn provider_row_family_and_label_line_up() {
        // (account, family) resolution picks rows via ProviderRow::family;
        // labels feed the "no <family> provider" 404s.
        let anthropic = super::ProviderRow {
            id: uuid::Uuid::new_v4(),
            provider: "anthropic-compatible".into(),
            model_aliases: None,
            models: None,
        };
        let openai = super::ProviderRow {
            id: uuid::Uuid::new_v4(),
            provider: "openai".into(),
            model_aliases: None,
            models: None,
        };
        let fireworks = super::ProviderRow {
            id: uuid::Uuid::new_v4(),
            provider: "fireworks".into(),
            model_aliases: None,
            models: None,
        };
        assert!(matches!(anthropic.family(), Family::Anthropic));
        assert!(matches!(openai.family(), Family::Openai));
        assert!(matches!(fireworks.family(), Family::Fireworks));
        assert_eq!(Family::Anthropic.label(), "anthropic");
        assert_eq!(Family::Openai.label(), "openai");
        assert_eq!(Family::Fireworks.label(), "fireworks");
    }

    #[test]
    fn fireworks_is_its_own_family_not_openai() {
        // The whole point of the third family: `fireworks` speaks the OpenAI
        // wire protocol but must never collapse onto the openai credential slot,
        // or the unique (account_id, family) index would forbid holding both.
        assert_eq!(Family::from_provider("fireworks"), Family::Fireworks);
        assert_ne!(Family::from_provider("fireworks"), Family::Openai);
        assert_eq!(Family::from_adapter("opencode"), Family::Fireworks);
        assert_eq!(Family::from_adapter("opencode-cli"), Family::Fireworks);
        assert_eq!(Family::from_label("fireworks"), Some(Family::Fireworks));
        assert_eq!(Family::from_label("nope"), None);
    }

    #[test]
    fn gateway_env_keys_are_disjoint_across_families() {
        // A worker may carry all three at once; overlapping keys would make the
        // last mint silently win and 401 the others.
        let env_for = |family| {
            let mut env = std::collections::BTreeMap::new();
            apply_gateway_env(&mut env, family, "https://cctui.example", "cctui_s_tok".into());
            env
        };
        let anthropic = env_for(Family::Anthropic);
        let openai = env_for(Family::Openai);
        let fireworks = env_for(Family::Fireworks);
        assert_eq!(
            fireworks.get("FIREWORKS_BASE_URL").map(String::as_str),
            Some("https://cctui.example/gateway/fireworks")
        );
        assert_eq!(fireworks.get("FIREWORKS_API_KEY").map(String::as_str), Some("cctui_s_tok"));
        for other in [&anthropic, &openai] {
            assert!(other.keys().all(|k| !fireworks.contains_key(k)));
        }
    }

    #[test]
    fn fireworks_settings_default_and_override() {
        let defaults = FireworksSettings::resolve(None);
        assert_eq!(defaults.context_length_exceeded_behavior.as_deref(), Some("error"));
        assert!(defaults.session_affinity);
        assert!(defaults.extra_body.is_empty());

        // A partial stored blob overrides only the keys it names.
        let stored = serde_json::json!({
            "session_affinity": false,
            "extra_body": { "temperature": 0.2 },
        });
        let merged = FireworksSettings::resolve(Some(&stored));
        assert_eq!(merged.context_length_exceeded_behavior.as_deref(), Some("error"));
        assert!(!merged.session_affinity);
        assert_eq!(merged.extra_body.get("temperature"), Some(&serde_json::json!(0.2)));

        // An explicit null opts the injection out entirely.
        let off = serde_json::json!({ "context_length_exceeded_behavior": null });
        assert!(FireworksSettings::resolve(Some(&off)).context_length_exceeded_behavior.is_none());
    }

    #[test]
    fn fireworks_body_injection_never_overrides_the_client() {
        let settings = FireworksSettings::resolve(Some(&serde_json::json!({
            "extra_body": { "temperature": 0.2 },
        })));
        let mut body = serde_json::json!({ "model": "kimi", "messages": [] });
        settings.apply_body(&mut body, Some("sess-1"));
        assert_eq!(body["context_length_exceeded_behavior"], serde_json::json!("error"));
        assert_eq!(body["temperature"], serde_json::json!(0.2));
        assert_eq!(body["user"], serde_json::json!("sess-1"));

        let mut explicit = serde_json::json!({
            "context_length_exceeded_behavior": "truncate",
            "temperature": 1.0,
            "user": "mine",
        });
        settings.apply_body(&mut explicit, Some("sess-1"));
        assert_eq!(explicit["context_length_exceeded_behavior"], serde_json::json!("truncate"));
        assert_eq!(explicit["temperature"], serde_json::json!(1.0));
        assert_eq!(explicit["user"], serde_json::json!("mine"));
    }

    #[test]
    fn fireworks_affinity_off_leaves_user_alone() {
        let settings =
            FireworksSettings::resolve(Some(&serde_json::json!({ "session_affinity": false })));
        let mut body = serde_json::json!({ "model": "kimi" });
        settings.apply_body(&mut body, Some("sess-1"));
        assert!(body.get("user").is_none());
    }

    #[test]
    fn catalog_resolves_id_label_and_falls_back() {
        let catalog = serde_json::json!([
            { "model": "accounts/fireworks/models/kimi-k3", "label": "Kimi K3" },
            { "model": "accounts/fireworks/models/kimi-k2p6", "label": "Kimi K2.6" },
        ]);
        assert_eq!(
            resolve_catalog_model(Some(&catalog), "accounts/fireworks/models/kimi-k2p6").as_deref(),
            Some("accounts/fireworks/models/kimi-k2p6")
        );
        assert_eq!(
            resolve_catalog_model(Some(&catalog), "Kimi K3").as_deref(),
            Some("accounts/fireworks/models/kimi-k3")
        );
        // Unknown / empty falls back to the first entry rather than sending a
        // model id Fireworks would reject.
        assert_eq!(
            resolve_catalog_model(Some(&catalog), "gpt-5").as_deref(),
            Some("accounts/fireworks/models/kimi-k3")
        );
        assert_eq!(
            resolve_catalog_model(Some(&catalog), "").as_deref(),
            Some("accounts/fireworks/models/kimi-k3")
        );
        assert_eq!(resolve_catalog_model(None, "x"), None);
        assert_eq!(resolve_catalog_model(Some(&serde_json::json!([])), "x"), None);
    }

    #[test]
    fn session_token_ttl_defaults_and_honors_positive_override() {
        assert_eq!(ttl_hours_from(None), 12);
        assert_eq!(ttl_hours_from(Some("6".into())), 6);
        // Zero / negative / garbage all fall back to the default rather than
        // minting an already-dead (or never-expiring) token.
        assert_eq!(ttl_hours_from(Some("0".into())), 12);
        assert_eq!(ttl_hours_from(Some("-3".into())), 12);
        assert_eq!(ttl_hours_from(Some("nope".into())), 12);
    }

    /// DB-gated: the gateway auth lookup must refuse an expired session token
    /// (past `expires_at`) while resolving a live one, and a NULL `expires_at`
    /// (legacy row) must still resolve. Runs the exact enforcement predicate the
    /// passthrough / `token-valid` queries share. Skips without a database.
    #[tokio::test]
    async fn expired_session_token_is_not_resolved() {
        let Some(url) =
            std::env::var("DATABASE_URL").ok().or_else(|| std::env::var("TEST_DATABASE_URL").ok())
        else {
            eprintln!("skipping expired_session_token_is_not_resolved: no DATABASE_URL");
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");

        let uid = uuid::Uuid::new_v4();
        let acct = uuid::Uuid::new_v4();
        let prov = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("ttl-test-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
            .bind(acct)
            .bind(uid)
            .bind("ttl-test-acct")
            .execute(&pool)
            .await
            .expect("seed account");
        sqlx::query(
            "INSERT INTO account_providers \
                 (id, user_id, name, provider, encrypted_refresh_token, account_id) \
             VALUES ($1, $2, $3, 'anthropic', 'x', $4)",
        )
        .bind(prov)
        .bind(uid)
        .bind("ttl-test-acct")
        .bind(acct)
        .execute(&pool)
        .await
        .expect("seed provider");

        let resolves = |hash: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS ( \
                        SELECT 1 FROM session_tokens t \
                          JOIN account_providers a ON a.id = t.account_id \
                         WHERE t.token_hash = $1 AND t.revoked_at IS NULL \
                           AND (t.expires_at IS NULL OR t.expires_at > now()))",
                )
                .bind(hash)
                .fetch_one(&pool)
                .await
                .expect("resolve query")
            }
        };
        let seed_tok = |hash: &'static str, expires: Option<chrono::DateTime<chrono::Utc>>| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO session_tokens (token_hash, session_id, account_id, expires_at) \
                     VALUES ($1, $2, $3, $4)",
                )
                .bind(hash)
                .bind(format!("sess-{hash}"))
                .bind(prov)
                .bind(expires)
                .execute(&pool)
                .await
                .expect("seed token");
            }
        };

        seed_tok("ttl-live", Some(chrono::Utc::now() + chrono::Duration::hours(1))).await;
        seed_tok("ttl-dead", Some(chrono::Utc::now() - chrono::Duration::hours(1))).await;
        seed_tok("ttl-null", None).await;

        assert!(resolves("ttl-live").await, "unexpired token must resolve");
        assert!(!resolves("ttl-dead").await, "expired token must NOT resolve");
        assert!(resolves("ttl-null").await, "legacy NULL-expiry token must still resolve");

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// DB-gated: the observed-identity signal — a token stamped `last_used_at`
    /// (as the gateway does on a successful passthrough) flips the session into
    /// the "traffic observed" set the session list derives; an unstamped bound
    /// token stays out of it (the warning state). Skips without a database.
    #[tokio::test]
    async fn last_used_stamp_drives_observed_traffic() {
        let Some(url) =
            std::env::var("DATABASE_URL").ok().or_else(|| std::env::var("TEST_DATABASE_URL").ok())
        else {
            eprintln!("skipping last_used_stamp_drives_observed_traffic: no DATABASE_URL");
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");

        let uid = uuid::Uuid::new_v4();
        let acct = uuid::Uuid::new_v4();
        let prov = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("obs-test-{uid}"))
            .bind(format!("kh-obs-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, 'obs-acct')")
            .bind(acct)
            .bind(uid)
            .execute(&pool)
            .await
            .expect("seed account");
        sqlx::query(
            "INSERT INTO account_providers \
                 (id, user_id, name, provider, encrypted_refresh_token, account_id) \
             VALUES ($1, $2, 'obs-acct', 'anthropic', 'x', $3)",
        )
        .bind(prov)
        .bind(uid)
        .bind(acct)
        .execute(&pool)
        .await
        .expect("seed provider");

        let sid = format!("obs-sess-{uid}");
        sqlx::query(
            "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
        )
        .bind(format!("obs-hash-{uid}"))
        .bind(&sid)
        .bind(prov)
        .execute(&pool)
        .await
        .expect("seed token");

        let observed = |sid: String| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS ( \
                        SELECT 1 FROM session_tokens \
                         WHERE session_id = $1 AND revoked_at IS NULL AND last_used_at IS NOT NULL)",
                )
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .expect("observed query")
            }
        };
        assert!(!observed(sid.clone()).await, "unstamped bound token → no traffic observed (warn)");

        sqlx::query(
            "UPDATE session_tokens SET last_used_at = now() \
             WHERE token_hash = $1 \
               AND (last_used_at IS NULL OR last_used_at < now() - interval '60 seconds')",
        )
        .bind(format!("obs-hash-{uid}"))
        .execute(&pool)
        .await
        .expect("stamp last_used");
        assert!(observed(sid.clone()).await, "stamped token → traffic observed (no warn)");

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    #[test]
    fn child_budget_becomes_a_session_usd_cap() {
        let merged = merge_session_budget(&crate::soft_limit::SoftLimits::default(), Some(0.75));
        assert_eq!(
            merged.limits[crate::soft_limit::KEY_SESSION_USD].cap_usd,
            Some(0.75),
            "the child's budget must enforce as a session_usd cap"
        );
    }

    #[test]
    fn child_budget_overrides_a_looser_account_cap_and_keeps_other_windows() {
        let account = crate::soft_limit::SoftLimits::from_json(Some(&serde_json::json!({
            "session_usd": { "cap_usd": 10.0 },
            "usd_7d": { "cap_usd": 50.0 },
        })));
        let merged = merge_session_budget(&account, Some(2.0));
        assert_eq!(merged.limits[crate::soft_limit::KEY_SESSION_USD].cap_usd, Some(2.0));
        assert_eq!(merged.limits[crate::soft_limit::KEY_USD_7D].cap_usd, Some(50.0));
    }

    #[test]
    fn no_or_invalid_budget_leaves_the_account_limits_untouched() {
        let account = crate::soft_limit::SoftLimits::from_json(Some(&serde_json::json!({
            "usd_5h": { "cap_usd": 3.0 },
        })));
        for budget in [None, Some(0.0), Some(-1.0), Some(f64::NAN)] {
            let merged = merge_session_budget(&account, budget);
            assert_eq!(merged, account, "budget {budget:?} must not alter the account limits");
        }
    }
}
