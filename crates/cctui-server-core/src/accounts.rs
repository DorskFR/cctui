//! `/api/v1/accounts` — account identities + provider credentials.
//!
//! An **account** is an identity (e.g. `personal`, `enterprise`): a name, an
//! owner, optional extra environment, and sharing grants. Each account holds
//! zero or more **providers** (`account_providers`, née `oauth_accounts`):
//! one credential per provider family (anthropic | openai | fireworks | google) — a
//! native OAuth subscription, a compatible endpoint, or a static provider key.
//! OAuth refresh tokens
//! are encrypted at rest with the vault key (`cctui_crypto`, same as
//! `api_keys`/`dispatchers`) and are **never** returned over the API —
//! list/get only ever surface provider/expiry/last-used + lightweight stats.
//! Accounts belong to the registering user and are visible/usable only by that
//! user (the `Human` route policy + `owner_filter`).

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::http::StatusCode;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub use cctui_proto::api::accounts::{
    AccountInfo, AccountModel, AccountUsage, AccountUsageEntry, CreateAccount, GrantShare,
    OAuthFinish, OAuthStartResponse, ProviderInfo, ProviderSpec, SettingsCatalogResponse,
    ShareInfo, UpdateAccount, UpdateProvider, UsageWindowView,
};

use crate::auth::AuthContext;
use crate::error::{AppError, err};
use crate::gateway;
use crate::gateway::limits::LimitContext;
use crate::gateway::usage::UsageContext;
use crate::resource_owners::{Shareable, shareable_owner};
use crate::state::CoreState;

pub trait AccountsContext: UsageContext + LimitContext {
    fn account_core(&self) -> &CoreState;
    fn oauth_logins(&self) -> &PendingOAuthLogins;
}

/// How long a pending "Sign in with Claude" login stays valid.
pub const PENDING_OAUTH_TTL: Duration = Duration::minutes(10);

/// In-memory store of pending OAuth logins, keyed by nonce.
pub type PendingOAuthLogins = Arc<DashMap<String, PendingOAuthLogin>>;

/// A pending "Sign in with Claude" login: the PKCE verifier we generated and the user it
/// belongs to, with a creation timestamp for TTL expiry.
///
/// Held only in memory and deleted on finish (single-use). `account_id` carries an optional
/// attach target: when set, the finished credential lands as a provider under that existing
/// account instead of creating a new identity.
#[derive(Clone)]
pub struct PendingOAuthLogin {
    pub user_id: Uuid,
    pub provider: String,
    pub code_verifier: String,
    pub created_at: DateTime<Utc>,
    pub account_id: Option<Uuid>,
}

/// Resolve which user an account operation targets.
///
/// A user token always acts as itself; the env admin token has no user identity, so it must
/// name the owner explicitly (`user_id` in the request). This is what lets an admin-authed
/// webui run the "Sign in with Claude/ChatGPT" flows instead of bouncing off "user token
/// required".
pub fn resolve_owner(
    ctx: &AuthContext,
    explicit: Option<Uuid>,
) -> Result<Uuid, (StatusCode, Json<serde_json::Value>)> {
    if ctx.is_admin() {
        explicit.ok_or_else(|| {
            err(StatusCode::BAD_REQUEST, "user_id required when using the admin token")
        })
    } else {
        Ok(ctx.user_id)
    }
}

/// Seed catalog for a new `fireworks` provider.
///
/// DATA, not policy: it is written to the row at create and fully editable afterwards — no
/// spawn, dispatch, entrypoint, or worker path may hardcode a model id.
#[must_use]
pub fn fireworks_default_models() -> serde_json::Value {
    serde_json::json!([{
        "model": "accounts/fireworks/models/kimi-k3",
        "label": "Kimi K3",
        "price_input_per_mtok": 3.0,
        "price_cached_input_per_mtok": 0.3,
        "price_output_per_mtok": 15.0,
        "context_length": 1_048_576,
    }])
}

/// Seed catalog for a new `google` provider: data on the row, fully editable.
#[must_use]
pub fn google_default_models() -> serde_json::Value {
    serde_json::json!([{
        "model": "gemini-2.5-pro",
        "label": "Gemini 2.5 Pro",
        "price_input_per_mtok": 1.25,
        "price_cached_input_per_mtok": 0.31,
        "price_output_per_mtok": 10.0,
        "context_length": 1_048_576,
    }, {
        "model": "gemini-2.5-flash",
        "label": "Gemini 2.5 Flash",
        "price_input_per_mtok": 0.3,
        "price_cached_input_per_mtok": 0.075,
        "price_output_per_mtok": 2.5,
        "context_length": 1_048_576,
    }])
}

/// Identity-row projection (`accounts` + owner name); providers are attached
/// separately from [`PROVIDER_SELECT`].
#[derive(Debug, sqlx::FromRow)]
pub struct AccountRow {
    id: Uuid,
    name: String,
    emoji: Option<String>,
    user_id: Uuid,
    user_name: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    /// Encrypted extra-env blob; decrypted to NAMES only for the API.
    env_json: Option<String>,
    pool_eligible: bool,
    pool_weight: f32,
}

impl AccountRow {
    #[must_use]
    pub fn into_info(self, providers: Vec<ProviderInfo>, key: &[u8]) -> AccountInfo {
        let env_names = env_names_from_enc(self.env_json.as_deref(), key);
        AccountInfo {
            id: self.id,
            name: self.name,
            emoji: self.emoji,
            user_id: self.user_id,
            user_name: self.user_name,
            pool_eligible: self.pool_eligible,
            pool_weight: self.pool_weight,
            created_at: self.created_at,
            updated_at: self.updated_at,
            providers,
            env_names,
        }
    }
}

/// Shared SELECT for [`ProviderInfo`]: per-provider token totals + a rough USD cost
/// estimate.
///
/// Tokens are recorded per session (`session_token_usage`); `session_tokens` bridges a
/// session to the provider row it ran under. `SUM()` over bigint returns NUMERIC, so cast
/// back to bigint for the i64 columns. Cost uses a per-provider blended per-million rate
/// (input/output/cache weighted) — an estimate, not a meter. Append a `WHERE`/`ORDER BY`
/// clause before use. Usage is read from the running per-(session, model) totals in
/// `session_usage_totals`, one row per session and model rather than one per turn. The
/// token totals are a correlated LATERAL rather than a grouped subquery so that selecting
/// one provider aggregates one provider's rows instead of the whole table.
pub const PROVIDER_SELECT: &str = "SELECT p.id, p.account_id, p.provider, p.family, p.models, p.model_aliases, p.managed, \
            p.base_url, p.auth_scheme, p.provider_account_id, \
            p.expires_at, p.created_at, p.last_used_at, \
            p.request_count, p.bytes_transferred, \
            p.soft_limits_json AS soft_limits, p.usage_notices, p.header_pin, \
            p.needs_reauth, p.last_auth_error, p.last_auth_error_at, p.settings_json, \
            p.provider_settings, p.rate_limits_json AS rate_limits, p.usage_probe, \
            (COALESCE(t.input_tokens,0) + COALESCE(t.output_tokens,0) \
             + COALESCE(t.cache_read_tokens,0) + COALESCE(t.cache_creation_tokens,0))::bigint \
              AS total_tokens, \
            (CASE \
               WHEN p.family IN ('fireworks', 'google') THEN COALESCE(fw.usd, 0) * 1000000.0 \
               WHEN p.provider = 'openai' THEN \
                 COALESCE(t.input_tokens,0)*1.25 + COALESCE(t.output_tokens,0)*10 \
                 + COALESCE(t.cache_read_tokens,0)*0.125 + COALESCE(t.cache_creation_tokens,0)*1.25 \
               ELSE \
                 COALESCE(t.input_tokens,0)*3 + COALESCE(t.output_tokens,0)*15 \
                 + COALESCE(t.cache_read_tokens,0)*0.3 + COALESCE(t.cache_creation_tokens,0)*3.75 \
             END / 1000000.0)::double precision AS est_cost_usd \
     FROM account_providers p \
     LEFT JOIN LATERAL ( \
         SELECT SUM(stu.input_tokens)          AS input_tokens, \
                SUM(stu.output_tokens)         AS output_tokens, \
                SUM(stu.cache_read_tokens)     AS cache_read_tokens, \
                SUM(stu.cache_creation_tokens) AS cache_creation_tokens \
         FROM session_tokens st \
         JOIN session_usage_totals stu ON stu.session_id = st.session_id \
         WHERE st.account_id = p.id \
     ) t ON true \
     LEFT JOIN LATERAL ( \
         SELECT SUM(( \
                  (stu.input_tokens + stu.cache_creation_tokens) \
                    * COALESCE((m.value->>'price_input_per_mtok')::double precision, 0) \
                  + stu.cache_read_tokens \
                    * COALESCE((m.value->>'price_cached_input_per_mtok')::double precision, 0) \
                  + stu.output_tokens \
                    * COALESCE((m.value->>'price_output_per_mtok')::double precision, 0) \
                ) / 1000000.0) AS usd \
         FROM session_tokens st \
         JOIN session_usage_totals stu ON stu.session_id = st.session_id \
         JOIN jsonb_array_elements( \
                CASE WHEN jsonb_typeof(p.models) = 'array' THEN p.models ELSE '[]'::jsonb END \
              ) m ON m.value->>'model' = stu.model \
         WHERE st.account_id = p.id \
     ) fw ON p.family IN ('fireworks', 'google')";

/// Identity SELECT for [`AccountRow`]. Append a `WHERE`/`ORDER BY` before use.
pub const ACCOUNT_SELECT: &str = "SELECT a.id, a.name, a.emoji, a.user_id, u.name AS user_name, a.created_at, a.updated_at, \
     a.env_json, a.pool_eligible, a.pool_weight \
     FROM accounts a JOIN users u ON u.id = a.user_id";

/// Fetch one account (owner-scoped: `owner` NULL = admin sees all) with its
/// providers. `Ok(None)` = no such account for that scope.
pub async fn fetch_account_info(
    state: &impl AccountsContext,
    id: Uuid,
    owner: Option<Uuid>,
) -> Result<Option<AccountInfo>, sqlx::Error> {
    let pool = &state.account_core().pool;
    let row: Option<AccountRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ACCOUNT_SELECT} WHERE a.id = $1 AND ($2::uuid IS NULL OR a.user_id = $2)"
    )))
    .bind(id)
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let providers: Vec<ProviderInfo> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PROVIDER_SELECT} WHERE p.account_id = $1 ORDER BY p.family"
    )))
    .bind(id)
    .fetch_all(pool)
    .await?;
    let key = state.vault_key();
    Ok(Some(row.into_info(providers, &key)))
}

/// Fetch one provider row by id. `Ok(None)` = no such provider.
pub async fn fetch_provider_info(
    pool: &sqlx::PgPool,
    id: Uuid,
) -> Result<Option<ProviderInfo>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{PROVIDER_SELECT} WHERE p.id = $1")))
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Request to move a provider between accounts of the same owner.
///
/// `POST /api/v1/accounts/{id}/providers/{provider_id}/move` payload:
/// re-parent a provider credential onto another account owned by the same user
/// (the manual merge path for the migration's one-account-per-old-row backfill).
#[derive(Debug, serde::Deserialize)]
pub struct MoveProvider {
    pub target_account_id: Uuid,
}

/// Validate a canonical-key soft-limit map into the JSONB blob to store, folding in any
/// legacy scalar fields.
///
/// Returns `Ok(None)` when the result is empty (clears the column). Rejects out-of-range
/// caps/bypasses and non-canonical keys (which could otherwise inject markup or collide).
pub fn build_soft_limits_json(
    map: Option<&serde_json::Value>,
    legacy_session_cap: Option<i32>,
    legacy_weekly_cap: Option<i32>,
    legacy_session_bypass: Option<i32>,
    legacy_weekly_bypass: Option<i32>,
) -> Result<Option<serde_json::Value>, AppError> {
    use std::collections::BTreeMap;
    let mut out: BTreeMap<String, serde_json::Value> = BTreeMap::new();

    let mut insert = |key: &str,
                      cap: Option<i32>,
                      cap_usd: Option<f64>,
                      bypass: Option<i32>,
                      pace_cap: Option<f64>| {
        if let Some(c) = cap
            && !(0..=100).contains(&c)
        {
            return Err(AppError::new(StatusCode::BAD_REQUEST, "soft-limit cap must be 0-100"));
        }
        // Below 1x is a cap tighter than an even spend, which would refuse an
        // idle-then-work account forever; above 100x can never trigger.
        if let Some(p) = pace_cap
            && (!p.is_finite() || !(1.0..=100.0).contains(&p))
        {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "soft-limit pace_cap must be 1-100",
            ));
        }
        if let Some(c) = cap_usd
            && (!c.is_finite() || c < 0.0)
        {
            return Err(AppError::new(StatusCode::BAD_REQUEST, "soft-limit cap_usd must be >= 0"));
        }
        if let Some(b) = bypass
            && b < 0
        {
            return Err(AppError::new(StatusCode::BAD_REQUEST, "soft-limit bypass must be >= 0"));
        }
        if cap.is_none() && cap_usd.is_none() && bypass.is_none() && pace_cap.is_none() {
            return Ok(());
        }
        let Some(canon) = cctui_proto::api::soft_limit::canonicalize_key(key) else {
            return Err(AppError::new(StatusCode::BAD_REQUEST, "unknown soft-limit window key"));
        };
        // A dollar cap belongs only to a dollar window, and vice versa: storing
        // the wrong one would read as an unenforceable cap in the UI.
        let usd_window = cctui_proto::api::soft_limit::is_usd_key(&canon);
        let mut entry = serde_json::Map::new();
        if let Some(c) = cap.filter(|_| !usd_window) {
            entry.insert("cap_pct".into(), serde_json::json!(c));
        }
        if let Some(c) = cap_usd.filter(|_| usd_window) {
            entry.insert("cap_usd".into(), serde_json::json!(c));
        }
        if let Some(b) = bypass {
            entry.insert("bypass_minutes".into(), serde_json::json!(b));
        }
        if let Some(p) = pace_cap.filter(|_| !usd_window) {
            entry.insert("pace_cap".into(), serde_json::json!(p));
        }
        if entry.is_empty() {
            return Ok(());
        }
        out.insert(canon, serde_json::Value::Object(entry));
        Ok(())
    };

    if let Some(obj) = map.and_then(serde_json::Value::as_object) {
        for (key, v) in obj {
            let cap = v.get("cap_pct").and_then(serde_json::Value::as_i64).map(|n| n as i32);
            let cap_usd = v.get("cap_usd").and_then(serde_json::Value::as_f64);
            let bypass =
                v.get("bypass_minutes").and_then(serde_json::Value::as_i64).map(|n| n as i32);
            let pace_cap = v.get("pace_cap").and_then(serde_json::Value::as_f64);
            insert(key, cap, cap_usd, bypass, pace_cap)?;
        }
    } else if map.is_some_and(|v| !v.is_null()) {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "soft_limits must be an object"));
    } else {
        // No map supplied — fold the legacy scalar fields.
        insert(
            cctui_proto::api::soft_limit::KEY_SESSION,
            legacy_session_cap,
            None,
            legacy_session_bypass,
            None,
        )?;
        insert(
            cctui_proto::api::soft_limit::KEY_WEEKLY_ALL,
            legacy_weekly_cap,
            None,
            legacy_weekly_bypass,
            None,
        )?;
    }

    Ok((!out.is_empty()).then(|| serde_json::to_value(out).unwrap_or(serde_json::Value::Null)))
}

/// Validate a `{ rpm?, tpm? }` rate-limit object into the JSONB blob to store.
///
/// A zero or omitted dimension clears that limit; `Ok(None)` when neither is set (clears
/// the column). Rejects negative / non-integer values and a non-object.
pub fn build_rate_limits_json(
    map: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>, AppError> {
    let Some(map) = map.filter(|v| !v.is_null()) else { return Ok(None) };
    let Some(obj) = map.as_object() else {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "rate_limits must be an object"));
    };
    let mut out = serde_json::Map::new();
    for key in ["rpm", "tpm"] {
        match obj.get(key) {
            None | Some(serde_json::Value::Null) => {}
            Some(v) => {
                let Some(n) = v.as_u64() else {
                    return Err(AppError::new(
                        StatusCode::BAD_REQUEST,
                        "rate limit must be a whole number >= 0",
                    ));
                };
                if n > 0 {
                    out.insert(key.to_owned(), serde_json::json!(n));
                }
            }
        }
    }
    Ok((!out.is_empty()).then_some(serde_json::Value::Object(out)))
}

/// A compatible endpoint's `base_url` must not reach internal addresses.
pub async fn check_base_url(raw: &str) -> Result<(), AppError> {
    crate::outbound::validate_upstream_url(raw).await.map_err(|e| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "base_url {e}; an admin can allow a trusted host in Settings \
                     (allowed upstream hosts) or with CCTUI_UPSTREAM_ALLOWED_HOSTS"
            ),
        )
    })
}

/// Which family's catalog to serve. Absent = anthropic (Claude Code).
#[derive(serde::Deserialize)]
pub struct SettingsCatalogQuery {
    pub family: Option<String>,
}

/// `GET /accounts/settings-catalog` — serve the per-account settings catalog for `?family=`
/// (anthropic | openai).
///
/// Read-only, embedded data; no tenant scoping needed (the catalog is the same for everyone
/// and contains no secrets).
pub fn settings_catalog(q: &SettingsCatalogQuery) -> Json<SettingsCatalogResponse> {
    let family = q
        .family
        .as_deref()
        .and_then(crate::gateway::Family::from_label)
        .unwrap_or(crate::gateway::Family::Anthropic);
    let c = crate::settings_catalog::for_family(family);
    let mut preset = c.quiet_defaults().clone();
    preset.settings.retain(|name, _| {
        c.key(name).is_some_and(crate::settings_catalog::SettingKey::account_exposable)
    });
    Json(SettingsCatalogResponse {
        keys: c.exposable_keys().cloned().collect(),
        env: c.env_allowlist().to_vec(),
        preset,
        blocked: c.blocked_key_names().map(str::to_owned).collect(),
    })
}

/// Longest accepted glyph in scalar values: a family ZWJ sequence with skin
/// tones sits well inside this, a pasted sentence does not.
pub const EMOJI_MAX_SCALARS: usize = 8;

#[must_use]
pub const fn is_emoji_base(c: char) -> bool {
    matches!(c as u32,
        0x203C | 0x2049 | 0x2122 | 0x2139
        | 0x2190..=0x21FF
        | 0x2300..=0x23FF
        | 0x25AA..=0x25FF
        | 0x2600..=0x27BF
        | 0x2934 | 0x2935
        | 0x2B00..=0x2BFF
        | 0x3030 | 0x303D | 0x3297 | 0x3299
        | 0x1F000..=0x1FAFF)
}

/// Modifiers that decorate a base without starting a new glyph: variation
/// selectors, skin tones, the keycap enclosure and tag characters.
#[must_use]
pub const fn is_emoji_modifier(c: char) -> bool {
    matches!(c as u32, 0xFE0E | 0xFE0F | 0x20E3 | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F)
}

#[must_use]
pub const fn is_regional_indicator(c: char) -> bool {
    matches!(c as u32, 0x1F1E6..=0x1F1FF)
}

/// Normalize an account's identity glyph: trimmed, and either a single emoji grapheme or
/// nothing.
///
/// Accepts ZWJ sequences (👨‍👩‍👧), skin-tone modifiers and two-scalar regional-indicator
/// flags; rejects plain text, two glyphs stuck together, and anything longer than
/// [`EMOJI_MAX_SCALARS`] scalar values. A blank input is [`None`] — "clear it".
pub fn normalize_emoji(raw: &str) -> Result<Option<String>, &'static str> {
    const BAD: &str = "emoji must be a single emoji character";
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() > EMOJI_MAX_SCALARS {
        return Err(BAD);
    }
    if is_regional_indicator(chars[0]) {
        return if chars.len() == 2 && is_regional_indicator(chars[1]) {
            Ok(Some(trimmed.to_owned()))
        } else {
            Err(BAD)
        };
    }
    let mut want_base = true;
    for &c in &chars {
        if want_base {
            if !is_emoji_base(c) {
                return Err(BAD);
            }
            want_base = false;
        } else if c == '\u{200D}' {
            want_base = true;
        } else if !is_emoji_modifier(c) {
            return Err(BAD);
        }
    }
    if want_base {
        return Err(BAD);
    }
    Ok(Some(trimmed.to_owned()))
}

/// [`normalize_emoji`] as an API-level check.
pub fn emoji_field(raw: Option<&str>) -> Result<Option<String>, AppError> {
    raw.map_or(Ok(None), |v| {
        normalize_emoji(v).map_err(|msg| AppError::new(StatusCode::BAD_REQUEST, msg))
    })
}

/// Validate a pasted `settings_json` blob before persisting it via the settings catalog:
/// MANAGED and SYSTEM keys are rejected, keys the catalog does not know pass through as out
/// of schema.
///
/// Fail-closed — any violation aborts the whole write.
pub fn validate_settings_json(provider: &str, value: &serde_json::Value) -> Result<(), AppError> {
    let report = crate::settings_catalog::for_provider(provider).validate_settings(value);
    if report.ok() {
        return Ok(());
    }
    let detail = report.violations.iter().map(|v| v.key.as_str()).collect::<Vec<_>>().join(", ");
    Err(AppError::new(
        StatusCode::BAD_REQUEST,
        format!("settings_json rejected — not settable per-account: {detail}"),
    ))
}

/// Validate account environment variables before encryption.
///
/// Validate the account-level free-form extra-env map before it is
/// encrypted and stored: any well-formed env var name is accepted EXCEPT a
/// denylist of session-critical / gateway-managed vars (values are arbitrary and
/// may be secrets). Fail-closed with a per-name reason.
pub fn validate_env_json<S: std::hash::BuildHasher>(
    env: &std::collections::HashMap<String, String, S>,
) -> Result<(), AppError> {
    let map: std::collections::BTreeMap<String, String> =
        env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let report = crate::settings_catalog::catalog().validate_free_env(&map);
    if report.ok() {
        return Ok(());
    }
    let detail = report
        .violations
        .iter()
        .map(|v| format!("{}: {}", v.key, v.reason))
        .collect::<Vec<_>>()
        .join("; ");
    Err(AppError::new(StatusCode::BAD_REQUEST, format!("env_json rejected — {detail}")))
}

/// Decrypt a stored `env_json` blob to its var NAMES only (sorted), never the values.
///
/// A missing/undecryptable/malformed blob yields an empty list — the names are a display
/// convenience, never a hard dependency.
#[must_use]
pub fn env_names_from_enc(enc: Option<&str>, key: &[u8]) -> Vec<String> {
    env_map_from_enc(enc, key).into_keys().collect()
}

/// Decrypt a stored `env_json` blob to its full map, server-side only.
/// Missing/undecryptable/malformed ⇒ empty map.
#[must_use]
pub fn env_map_from_enc(
    enc: Option<&str>,
    key: &[u8],
) -> std::collections::BTreeMap<String, String> {
    let Some(enc) = enc else { return std::collections::BTreeMap::new() };
    let Some(json) = cctui_crypto::decrypt(enc, key) else {
        return std::collections::BTreeMap::new();
    };
    serde_json::from_str(&json).unwrap_or_default()
}

/// Validate + encrypt an extra-env map. Empty ⇒ `None` (clears).
pub fn encrypt_env<S: std::hash::BuildHasher>(
    env: Option<&std::collections::HashMap<String, String, S>>,
    key: &[u8],
) -> Result<Option<String>, AppError> {
    let Some(map) = env.filter(|m| !m.is_empty()) else { return Ok(None) };
    validate_env_json(map)?;
    Ok(Some(cctui_crypto::encrypt(&serde_json::to_string(map).unwrap_or_default(), key)))
}

/// A validated, encrypted provider payload ready to INSERT.
pub struct ProviderWrite {
    pub provider: String,
    pub enc_refresh: Option<String>,
    pub enc_access: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub base_url: Option<String>,
    pub auth_scheme: String,
    pub models: Option<serde_json::Value>,
    pub model_aliases: Option<serde_json::Value>,
    pub soft_limits_json: Option<serde_json::Value>,
    pub settings_json: Option<serde_json::Value>,
    pub provider_settings: Option<serde_json::Value>,
    pub rate_limits_json: Option<serde_json::Value>,
    pub usage_probe: Option<String>,
}

/// Validate a [`ProviderSpec`] into a [`ProviderWrite`] (shared by the one-shot account
/// create and `POST /accounts/{id}/providers`).
///
/// Native subscription providers require an OAuth refresh token (`auth_scheme` = oauth);
/// compatible endpoints a base URL + a static credential stored in
/// `encrypted_access_token`, no refresh token, `auth_scheme` = `bearer|api_key`.
// Linear validator: one branch per optional field, no nesting.
#[allow(clippy::too_many_lines)]
pub async fn prepare_provider_write(
    spec: &ProviderSpec,
    key: &[u8],
) -> Result<ProviderWrite, AppError> {
    let Some(provider) = spec.provider.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "provider required"));
    };
    if !matches!(
        provider,
        "anthropic"
            | "openai"
            | "anthropic-compatible"
            | "openai-compatible"
            | "fireworks"
            | "google"
    ) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "provider must be anthropic|openai|anthropic-compatible|openai-compatible|fireworks|google",
        ));
    }
    let fireworks = provider == "fireworks";
    let google = provider == "google";
    // `fireworks` carries a static `fw_...` bearer and `google` a static API
    // key like a compatible endpoint, but their upstream is built in — a base
    // URL is an override, not a requirement.
    let builtin_upstream = fireworks || google;
    let compatible =
        builtin_upstream || matches!(provider, "anthropic-compatible" | "openai-compatible");

    let (enc_refresh, enc_access, base_url, auth_scheme): (
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    );
    if compatible {
        let base = spec.base_url.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let base = match base {
            Some(b) => Some(b),
            None if builtin_upstream => None,
            None => {
                return Err(AppError::new(
                    StatusCode::BAD_REQUEST,
                    "base_url required for a compatible endpoint",
                ));
            }
        };
        if let Some(b) = base {
            check_base_url(b).await?;
        }
        let scheme = spec.auth_scheme.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let scheme = scheme.unwrap_or(if google { "api_key" } else { "bearer" });
        if !matches!(scheme, "bearer" | "api_key") {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "auth_scheme must be bearer|api_key",
            ));
        }
        // A static credential is optional (an open proxy accepts any value); when
        // absent we still store a dummy so the gateway has a bearer to forward.
        let cred = spec
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("sk-dummy");
        enc_refresh = None;
        enc_access = Some(cctui_crypto::encrypt(cred, key));
        base_url = base.map(str::to_owned);
        auth_scheme = scheme.to_owned();
    } else {
        let refresh = spec.refresh_token.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let Some(refresh) = refresh else {
            return Err(AppError::new(StatusCode::BAD_REQUEST, "refresh_token required"));
        };
        enc_refresh = Some(cctui_crypto::encrypt(refresh, key));
        enc_access = spec.access_token.as_deref().map(|t| cctui_crypto::encrypt(t, key));
        base_url = None;
        auth_scheme = "oauth".to_owned();
    }
    let expires_at = spec.expires_at.and_then(|s| DateTime::<Utc>::from_timestamp(s, 0));
    let models = spec
        .models
        .as_ref()
        .filter(|m| !m.is_empty())
        .map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null))
        .or_else(|| fireworks.then(fireworks_default_models))
        .or_else(|| google.then(google_default_models));
    // Alias map: an empty map stores NULL (no remapping).
    let model_aliases = spec
        .model_aliases
        .as_ref()
        .filter(|m| !m.is_empty())
        .map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null));
    if let Some(s) = spec.settings_json.as_ref().filter(|v| !v.is_null()) {
        validate_settings_json(provider, s)?;
    }
    let settings_json = spec.settings_json.as_ref().filter(|v| !v.is_null()).cloned();
    let provider_settings = match spec.provider_settings.as_ref().filter(|v| !v.is_null()) {
        Some(v) => Some(validate_provider_settings(v)?),
        None => fireworks.then(crate::gateway::fireworks_default_settings),
    };

    Ok(ProviderWrite {
        provider: provider.to_owned(),
        enc_refresh,
        enc_access,
        expires_at,
        base_url,
        auth_scheme,
        models,
        model_aliases,
        soft_limits_json: build_soft_limits_json(
            spec.soft_limits.as_ref(),
            spec.soft_limit_5h_pct,
            spec.soft_limit_7d_pct,
            spec.soft_limit_bypass_5h_minutes.or(spec.soft_limit_bypass_minutes),
            spec.soft_limit_bypass_7d_minutes.or(spec.soft_limit_bypass_minutes),
        )?,
        settings_json,
        provider_settings,
        rate_limits_json: build_rate_limits_json(spec.rate_limits.as_ref())?,
        usage_probe: validate_usage_probe(spec.usage_probe.as_deref(), compatible)?,
    })
}

/// Validate a compatible endpoint's usage probe.
///
/// A stored `usage_probe` must name a registered probe, and only a
/// compatible-endpoint credential can carry one — a native subscription has a
/// real usage API and a probe there would silently shadow it. An empty string
/// clears the column.
pub fn validate_usage_probe(
    raw: Option<&str>,
    compatible: bool,
) -> Result<Option<String>, AppError> {
    let Some(id) = raw.map(str::trim) else { return Ok(None) };
    if id.is_empty() {
        return Ok(None);
    }
    if !compatible {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "usage_probe is only settable on a compatible provider",
        ));
    }
    if crate::usage_probe::probe(id).is_none() {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            format!("unknown usage_probe; expected one of {}", crate::usage_probe::ids().join("|")),
        ));
    }
    Ok(Some(id.to_owned()))
}

/// Gateway settings must be a JSON object — the gateway deep-merges them over the family
/// defaults, and a scalar/array would silently replace the whole blob instead of overriding
/// one knob.
///
/// `thinking_display` is dropped: the gateway forwards bytes verbatim and cannot apply it.
pub fn validate_provider_settings(v: &serde_json::Value) -> Result<serde_json::Value, AppError> {
    v.as_object().map_or_else(
        || Err(AppError::new(StatusCode::BAD_REQUEST, "provider_settings must be a JSON object")),
        |obj| {
            let mut obj = obj.clone();
            obj.remove("thinking_display");
            Ok(serde_json::Value::Object(obj))
        },
    )
}

/// INSERT one provider row under an account. Bubbles the raw `sqlx::Error` so
/// callers can map a unique violation on `(account_id, family)` to a 409.
pub async fn insert_provider(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
    account_id: Uuid,
    w: &ProviderWrite,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO account_providers \
            (user_id, account_id, provider, encrypted_refresh_token, encrypted_access_token, \
             expires_at, base_url, models, auth_scheme, model_aliases, \
             soft_limits_json, settings_json, provider_settings, rate_limits_json, usage_probe) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15) \
         RETURNING id",
    )
    .bind(user_id)
    .bind(account_id)
    .bind(&w.provider)
    .bind(&w.enc_refresh)
    .bind(&w.enc_access)
    .bind(w.expires_at)
    .bind(&w.base_url)
    .bind(&w.models)
    .bind(&w.auth_scheme)
    .bind(&w.model_aliases)
    .bind(&w.soft_limits_json)
    .bind(&w.settings_json)
    .bind(&w.provider_settings)
    .bind(&w.rate_limits_json)
    .bind(&w.usage_probe)
    .fetch_one(conn)
    .await
}

/// `GET /api/v1/accounts` — the caller's own accounts with their providers
/// (tokens never returned). Admin sees every account, with the owner's name
/// joined in.
pub async fn list_accounts(
    state: &impl AccountsContext,
    ctx: &AuthContext,
) -> Result<Json<Vec<AccountInfo>>, AppError> {
    let accounts: Vec<AccountRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ACCOUNT_SELECT} WHERE $1::uuid IS NULL OR a.user_id = $1 \
           OR EXISTS (SELECT 1 FROM resource_shares s \
                      WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                        AND s.grantee_id = $1 AND s.revoked_at IS NULL) \
         ORDER BY a.name"
    )))
    .bind(ctx.owner_filter())
    .fetch_all(&state.account_core().pool)
    .await?;
    let providers: Vec<ProviderInfo> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PROVIDER_SELECT} WHERE $1::uuid IS NULL OR p.user_id = $1 \
           OR EXISTS (SELECT 1 FROM resource_shares s \
                      WHERE s.resource_type = 'account' AND s.resource_id = p.account_id \
                        AND s.grantee_id = $1 AND s.revoked_at IS NULL) \
         ORDER BY p.family"
    )))
    .bind(ctx.owner_filter())
    .fetch_all(&state.account_core().pool)
    .await?;

    let mut by_account: HashMap<Uuid, Vec<ProviderInfo>> = HashMap::new();
    for p in providers {
        by_account.entry(p.account_id).or_default().push(p);
    }
    let key = state.vault_key();
    let rows = accounts
        .into_iter()
        .map(|a| {
            let providers = by_account.remove(&a.id).unwrap_or_default();
            a.into_info(providers, &key)
        })
        .collect();
    Ok(Json(rows))
}

/// `GET /api/v1/accounts/{id}` — one account with its providers (owner-scoped;
/// admin sees any).
pub async fn get_account(
    state: &impl AccountsContext,
    ctx: &AuthContext,
    id: Uuid,
) -> Result<Json<AccountInfo>, AppError> {
    fetch_account_info(state, id, ctx.owner_filter())
        .await?
        .map(Json)
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such account"))
}

/// `POST /api/v1/accounts` — register an account identity, optionally with its
/// first provider credential in the same call (the legacy one-shot shape: a
/// body carrying `provider` + credential fields).
pub async fn create_account(
    state: &impl AccountsContext,
    ctx: &AuthContext,
    req: CreateAccount,
) -> Result<(StatusCode, Json<AccountInfo>), AppError> {
    let uid = resolve_owner(ctx, req.user_id)
        .map_err(|(s, Json(v))| AppError::new(s, v["error"].as_str().unwrap_or_default()))?;
    if req.name.trim().is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "name required"));
    }
    let emoji = emoji_field(req.emoji.as_deref())?;
    let enc_env = encrypt_env(req.env_json.as_ref(), &state.vault_key())?;
    // Validate the optional provider payload BEFORE creating the identity so a
    // bad credential body doesn't leave an empty account behind.
    let provider_write = if req.provider.provider.is_some() {
        Some(prepare_provider_write(&req.provider, &state.vault_key()).await?)
    } else {
        None
    };

    let mut tx = state.account_core().pool.begin().await?;
    let account_id: Uuid = match sqlx::query_scalar(
        "INSERT INTO accounts (user_id, name, env_json, emoji) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(uid)
    .bind(req.name.trim())
    .bind(&enc_env)
    .bind(&emoji)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(id) => id,
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            return Err(AppError::new(
                StatusCode::CONFLICT,
                "an account with that name already exists",
            ));
        }
        Err(e) => return Err(AppError::from(e)),
    };
    if let Some(w) = &provider_write
        && let Err(e) = insert_provider(&mut tx, uid, account_id, w).await
    {
        // A fresh account can't collide on (account_id, family); any error here
        // is a genuine DB failure. The tx rollback drops the parent too.
        return Err(AppError::from(e));
    }
    tx.commit().await?;

    let info = fetch_account_info(state, account_id, None).await?.ok_or_else(|| {
        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "account vanished after create")
    })?;
    Ok((StatusCode::CREATED, Json(info)))
}

/// `PATCH /api/v1/accounts/{id}` — rename the identity and/or replace its extra env.
///
/// Provider fields moved to the provider routes; sending them here 400s with a pointer.
/// Accounts holding a managed provider (the litellm shim) are read-only.
pub async fn update_account(
    state: &impl AccountsContext,
    ctx: &AuthContext,
    id: Uuid,
    req: UpdateAccount,
) -> Result<Json<AccountInfo>, AppError> {
    if req.base_url.is_some()
        || req.auth_scheme.is_some()
        || req.models.is_some()
        || req.model_aliases.is_some()
        || req.access_token.is_some()
        || req.soft_limits.is_some()
        || req.settings_json.is_some()
        || req.defaults.is_some()
    {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "provider fields moved: use PATCH /api/v1/accounts/:id/providers/:provider_id",
        ));
    }
    if let Some(name) = req.name.as_deref()
        && name.trim().is_empty()
    {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "name required"));
    }
    let name = req.name.as_deref().map(str::trim).map(str::to_owned);
    if let Some(w) = req.pool_weight
        && !(w.is_finite() && w > 0.0)
    {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "pool_weight must be a positive number",
        ));
    }
    // emoji: provided → set (a blank string clears it); absent → unchanged.
    let emoji_provided = req.emoji.is_some();
    let emoji = emoji_field(req.emoji.as_deref())?;
    // env_json: provided → re-encrypt + replace (empty map clears); absent →
    // unchanged. COALESCE can't distinguish those, so carry a provided-flag.
    let (env_provided, enc_env) = if req.env_json.is_none()
        && let Some(remove) = req.env_remove.as_ref().filter(|r| !r.is_empty())
    {
        let stored: Option<Option<String>> = sqlx::query_scalar(
            "SELECT env_json FROM accounts              WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
        )
        .bind(id)
        .bind(ctx.owner_filter())
        .fetch_optional(&state.account_core().pool)
        .await?;
        let key = state.vault_key();
        let mut map = env_map_from_enc(stored.flatten().as_deref(), &key);
        map.retain(|name, _| !remove.contains(name));
        let enc = (!map.is_empty())
            .then(|| cctui_crypto::encrypt(&serde_json::to_string(&map).unwrap_or_default(), &key));
        (true, enc)
    } else {
        (req.env_json.is_some(), encrypt_env(req.env_json.as_ref(), &state.vault_key())?)
    };

    // The owner's pool veto is set on its own: it is a property of lending the
    // account out, not of its credentials, so it stays editable on a managed
    // account (which the identity update below refuses to touch).
    if let Some(eligible) = req.pool_eligible {
        set_pool_eligible(state, id, ctx.owner_filter(), eligible).await?;
    }
    // Same for the pool weight: a statement about the plan, not the credential.
    if let Some(weight) = req.pool_weight {
        set_pool_weight(state, id, ctx.owner_filter(), weight).await?;
    }
    if req.pool_eligible.is_some() || req.pool_weight.is_some() {
        // Nothing else to change: don't run the identity update, whose managed
        // guard would 404 an account the pool fields just applied to cleanly.
        if name.is_none() && !env_provided && !emoji_provided {
            let info = fetch_account_info(state, id, None)
                .await?
                .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such account"))?;
            return Ok(Json(info));
        }
    }

    let updated: Option<Uuid> = sqlx::query_scalar(
        "UPDATE accounts SET \
            name = COALESCE($3, name), \
            env_json = CASE WHEN $4 THEN $5 ELSE env_json END, \
            emoji = CASE WHEN $6 THEN $7 ELSE emoji END, \
            updated_at = now() \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) \
           AND NOT EXISTS (SELECT 1 FROM account_providers p \
                           WHERE p.account_id = accounts.id AND p.managed) \
         RETURNING id",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .bind(&name)
    .bind(env_provided)
    .bind(&enc_env)
    .bind(emoji_provided)
    .bind(&emoji)
    .fetch_optional(&state.account_core().pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::new(StatusCode::CONFLICT, "an account with that name already exists")
        }
        other => AppError::from(other),
    })?;
    if updated.is_none() {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such account"));
    }
    let info = fetch_account_info(state, id, None)
        .await?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such account"))?;
    Ok(Json(info))
}

/// Set an account's relative plan size for pool aggregates (owner-scoped, like
/// the veto). Validated by the caller: finite and positive.
pub async fn set_pool_weight(
    state: &impl AccountsContext,
    id: Uuid,
    owner: Option<Uuid>,
    weight: f32,
) -> Result<(), AppError> {
    let touched = sqlx::query(
        "UPDATE accounts SET pool_weight = $3, updated_at = now() \
          WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(owner)
    .bind(weight)
    .execute(&state.account_core().pool)
    .await?;
    if touched.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such account"));
    }
    Ok(())
}

/// Set an account's pool veto, and prune the memberships it withdraws.
///
/// Clearing the flag retires the account from every pool it does not belong to
/// by ownership: leaving those rows would let it come back the moment the flag
/// flipped, which is not what "stop lending this out" means. The prune is
/// best-effort — [`crate::store::account_pools::usable_members`] re-checks the
/// flag at every election, so a failed prune costs tidiness, not safety.
pub async fn set_pool_eligible(
    state: &impl AccountsContext,
    id: Uuid,
    owner: Option<Uuid>,
    eligible: bool,
) -> Result<(), AppError> {
    let touched = sqlx::query(
        "UPDATE accounts SET pool_eligible = $3, updated_at = now() \
          WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(owner)
    .bind(eligible)
    .execute(&state.account_core().pool)
    .await?;
    if touched.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such account"));
    }
    if !eligible
        && let Err(e) = sqlx::query(
            "DELETE FROM account_pool_members m \
              USING account_pools p, accounts a \
              WHERE m.pool_id = p.id AND a.id = m.account_id \
                AND m.account_id = $1 AND p.user_id <> a.user_id",
        )
        .bind(id)
        .execute(&state.account_core().pool)
        .await
    {
        tracing::warn!(account_id = %id, error = %e, "pruning pool memberships failed");
    }
    Ok(())
}

/// `DELETE /api/v1/accounts/{id}` — delete the identity (cascades its providers
/// and their `session_tokens`).
pub async fn delete_account(
    state: &impl AccountsContext,
    ctx: &AuthContext,
    id: Uuid,
) -> Result<StatusCode, AppError> {
    // Admin (`ctx.user_id` = NULL) may delete any account; a user only its own.
    // Accounts holding a managed provider (litellm shim) are read-only.
    let res = sqlx::query(
        "DELETE FROM accounts WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) \
           AND NOT EXISTS (SELECT 1 FROM account_providers p \
                           WHERE p.account_id = accounts.id AND p.managed)",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .execute(&state.account_core().pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such account"));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ----------------------------------------------------------------------------
// Provider routes: add / edit / remove / move one credential under an
// account identity.
// ----------------------------------------------------------------------------

/// Attach a provider credential to an existing account.
///
/// `POST /api/v1/accounts/{id}/providers` — attach a provider credential to an
/// existing account (pasted-token / compatible-endpoint path; the OAuth flows
/// attach via `oauth/start`'s `account_id`). 409 if the account already has a
/// provider of that family.
pub async fn add_provider(
    state: &impl AccountsContext,
    ctx: &AuthContext,
    id: Uuid,
    req: ProviderSpec,
) -> Result<(StatusCode, Json<ProviderInfo>), AppError> {
    let owner = require_account_owner(state, ctx, id).await?;
    let w = prepare_provider_write(&req, &state.vault_key()).await?;

    let mut conn = state.account_core().pool.acquire().await?;
    let pid = match insert_provider(&mut conn, owner, id, &w).await {
        Ok(pid) => pid,
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            return Err(AppError::new(
                StatusCode::CONFLICT,
                "the account already has a provider of that family",
            ));
        }
        Err(e) => return Err(AppError::from(e)),
    };
    drop(conn);
    let info = fetch_provider_info(&state.account_core().pool, pid).await?.ok_or_else(|| {
        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "provider vanished after create")
    })?;
    Ok((StatusCode::CREATED, Json(info)))
}

/// Whether the payload touches a field only a compatible endpoint has.
/// `models` is deliberately not one: a declared model list is honoured for
/// every provider kind, native subscriptions included.
#[must_use]
pub const fn endpoint_fields_present(req: &UpdateProvider) -> bool {
    req.base_url.is_some() || req.auth_scheme.is_some() || req.access_token.is_some()
}

/// The provider PATCH's single UPDATE.
///
/// COALESCE keeps each column when its bind is NULL, so an absent field is a no-op; the
/// `CASE WHEN $n` pairs carry an explicit provided-flag for the columns whose "clear" is
/// also NULL. Admin (`ctx.user_id` = NULL) may edit any provider, a user only its own;
/// managed rows are excluded.
pub const UPDATE_PROVIDER_SQL: &str = "UPDATE account_providers SET \
            base_url = COALESCE($4, base_url), \
            auth_scheme = COALESCE($5, auth_scheme), \
            models = COALESCE($6, models), \
            encrypted_access_token = COALESCE($7, encrypted_access_token), \
            model_aliases = CASE WHEN $8 THEN $9 ELSE model_aliases END, \
            soft_limits_json = CASE WHEN $10 THEN $11 ELSE soft_limits_json END, \
            settings_json = CASE WHEN $12 THEN $13 ELSE settings_json END, \
            provider_settings = CASE WHEN $14 THEN $15 ELSE provider_settings END, \
            rate_limits_json = CASE WHEN $16 THEN $17 ELSE rate_limits_json END, \
            usage_notices = CASE WHEN $18 THEN $19 ELSE usage_notices END, \
            header_pin = COALESCE($20, header_pin), \
            usage_probe = CASE WHEN $21 THEN $22 ELSE usage_probe END \
         WHERE id = $1 AND account_id = $2 \
           AND ($3::uuid IS NULL OR user_id = $3) AND NOT managed \
         RETURNING id";

/// Edit a provider credential and its execution settings.
///
/// `PATCH /api/v1/accounts/{id}/providers/{provider_id}` — edit a provider
/// — compatible endpoints may change
/// base URL / auth scheme / credential; the declared model list, aliases, soft
/// limits, and settings are editable for every provider. Managed providers are read-only.
// Linear handler: per-field optional updates built into one dynamic UPDATE.
#[allow(clippy::too_many_lines)]
pub async fn update_provider(
    state: &impl AccountsContext,
    ctx: &AuthContext,
    (id, provider_id): (Uuid, Uuid),
    req: UpdateProvider,
) -> Result<Json<ProviderInfo>, AppError> {
    // Resolve the target (scoped to the caller; admin sees all) so we can tell a
    // compatible endpoint from a native one and reject editing managed rows.
    let provider = crate::store::account_providers::provider_owner_scoped(
        &state.account_core().pool,
        provider_id,
        id,
        ctx.owner_filter(),
    )
    .await?;
    let Some(provider) = provider else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such provider"));
    };
    let compatible = cctui_proto::provider::is_static_credential(&provider);

    // Endpoint fields are rejected for native providers so the edit form can't
    // silently no-op against a subscription credential.
    if !compatible && endpoint_fields_present(&req) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "endpoint fields are only editable for a compatible provider",
        ));
    }

    let key = state.vault_key();
    // Normalise the optional compatible-endpoint fields. base_url/auth_scheme are
    // validated like create; a blank credential keeps the stored one (NULL bind →
    // COALESCE no-op in SQL). models replaces the list wholesale when provided.
    let base_url = match req.base_url.as_deref().map(str::trim) {
        Some(b) if !b.is_empty() => {
            check_base_url(b).await?;
            Some(b.to_owned())
        }
        _ => None,
    };
    let auth_scheme = match req.auth_scheme.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => {
            if !matches!(s, "bearer" | "api_key") {
                return Err(AppError::new(
                    StatusCode::BAD_REQUEST,
                    "auth_scheme must be bearer|api_key",
                ));
            }
            Some(s.to_owned())
        }
        _ => None,
    };
    let models =
        req.models.as_ref().map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null));
    // Aliases: COALESCE can't distinguish "clear" from "unchanged"
    // (both would bind NULL), so carry an explicit provided-flag — provided +
    // empty clears the column, provided + non-empty replaces it.
    let aliases_provided = req.model_aliases.is_some();
    let model_aliases = req
        .model_aliases
        .as_ref()
        .filter(|m| !m.is_empty())
        .map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null));
    let enc_access = req
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|c| cctui_crypto::encrypt(c, &key));
    // Soft limits: a provided map replaces the whole JSONB column (an
    // empty object clears it); absent leaves it untouched. Validated before
    // persist. Carry a provided-flag so CASE-WHEN distinguishes clear/unchanged.
    let soft_provided = req.soft_limits.is_some();
    let soft_limits_json =
        build_soft_limits_json(req.soft_limits.as_ref(), None, None, None, None)?;
    // Settings: provided replaces (empty clears), absent untouched;
    // validated against the catalog allowlist before persist.
    let settings_provided = req.settings_json.is_some();
    if let Some(s) = req.settings_json.as_ref().filter(|v| !v.is_null()) {
        validate_settings_json(&provider, s)?;
    }
    let settings_json = req.settings_json.as_ref().filter(|v| !v.is_null()).cloned();
    let gateway_settings_provided = req.provider_settings.is_some();
    let provider_settings = match req.provider_settings.as_ref().filter(|v| !v.is_null()) {
        Some(v) => Some(validate_provider_settings(v)?),
        None => None,
    };
    let rate_limits_provided = req.rate_limits.is_some();
    let rate_limits_json = build_rate_limits_json(req.rate_limits.as_ref())?;
    let usage_probe_provided = req.usage_probe.is_some();
    let usage_probe = validate_usage_probe(req.usage_probe.as_deref(), compatible)?;
    let usage_notices_provided = req.usage_notices.is_some();
    let usage_notices =
        cctui_proto::api::gateway::UsageNotices::build_json(req.usage_notices.as_ref())
            .map_err(|m| AppError::new(StatusCode::BAD_REQUEST, m))?;

    let updated: Option<Uuid> = sqlx::query_scalar(UPDATE_PROVIDER_SQL)
        .bind(provider_id)
        .bind(id)
        .bind(ctx.owner_filter())
        .bind(&base_url)
        .bind(&auth_scheme)
        .bind(&models)
        .bind(&enc_access)
        .bind(aliases_provided)
        .bind(&model_aliases)
        .bind(soft_provided)
        .bind(&soft_limits_json)
        .bind(settings_provided)
        .bind(&settings_json)
        .bind(gateway_settings_provided)
        .bind(&provider_settings)
        .bind(rate_limits_provided)
        .bind(&rate_limits_json)
        .bind(usage_notices_provided)
        .bind(&usage_notices)
        .bind(req.header_pin)
        .bind(usage_probe_provided)
        .bind(&usage_probe)
        .fetch_optional(&state.account_core().pool)
        .await?;
    if updated.is_none() {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such provider"));
    }
    if soft_provided {
        let caps = cctui_proto::api::soft_limit::SoftLimits::from_json(soft_limits_json.as_ref());
        reevaluate_soft_limit_block(state, provider_id, &caps).await;
    }
    let info = fetch_provider_info(&state.account_core().pool, provider_id)
        .await?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such provider"))?;
    Ok(Json(info))
}

/// Sessions bound to `provider_id` that currently carry a soft-limit block. The
/// session row owns the block, so a block another replica set is visible here.
pub async fn soft_limit_blocked_sessions(
    pool: &sqlx::PgPool,
    provider_id: Uuid,
) -> Result<Vec<(String, Option<String>)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT DISTINCT s.id, s.soft_limit_key FROM sessions s \
         JOIN session_tokens t ON t.session_id = s.id AND t.revoked_at IS NULL \
         WHERE t.account_id = $1 AND s.soft_limit_reason IS NOT NULL",
    )
    .bind(provider_id)
    .fetch_all(pool)
    .await
}

/// Blocked sessions among `candidates` whose own block `caps` now lifts.
///
/// Each candidate is judged against the limit that blocked it, so a session over
/// its own `session_usd` budget stays blocked however high the account cap goes.
/// A `None` key is judged against the whole config. Clear-only (no re-block); pure so it is unit-testable.
#[must_use]
pub fn soft_limit_blocks_to_clear(
    candidates: &[(String, Option<String>)],
    windows: &[cctui_proto::api::soft_limit::UsageWindow],
    caps: &cctui_proto::api::soft_limit::SoftLimits,
    now: DateTime<Utc>,
) -> Vec<String> {
    let all_allow = matches!(
        cctui_proto::api::soft_limit::evaluate_soft_limit(windows, caps, None, now),
        cctui_proto::api::soft_limit::Decision::Allow
    );
    candidates
        .iter()
        .filter(|(_, key)| {
            key.as_ref().map_or(all_allow, |k| {
                cctui_proto::api::soft_limit::block_lifted_by(k, windows, caps, now)
            })
        })
        .map(|(id, _)| id.clone())
        .collect()
}

/// After a provider's soft-limit config is raised, lift the blocks it holds that
/// are now under cap. Best-effort: any DB/usage error is swallowed so
/// the surrounding PATCH still succeeds.
pub async fn reevaluate_soft_limit_block(
    state: &impl AccountsContext,
    provider_id: Uuid,
    caps: &cctui_proto::api::soft_limit::SoftLimits,
) {
    let candidates = match soft_limit_blocked_sessions(&state.account_core().pool, provider_id)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(%provider_id, error = %e, "soft-limit re-eval: session lookup failed");
            return;
        }
    };
    if candidates.is_empty() {
        return;
    }
    let usage = gateway::usage::usage_for_soft_limit(state, provider_id).await;
    let windows = usage
        .as_ref()
        .map(cctui_proto::api::soft_limit::normalize_usage_windows)
        .unwrap_or_default();
    let to_clear = soft_limit_blocks_to_clear(&candidates, &windows, caps, Utc::now());
    for session_id in to_clear {
        gateway::limits::clear_soft_limit_block(state, &session_id).await;
    }
}

/// `DELETE /api/v1/accounts/{id}/providers/{provider_id}` — remove one provider
/// credential (cascades its `session_tokens`). The identity and its other
/// providers stay.
pub async fn delete_provider(
    state: &impl AccountsContext,
    ctx: &AuthContext,
    (id, provider_id): (Uuid, Uuid),
) -> Result<StatusCode, AppError> {
    let removed = crate::store::account_providers::delete_owner_scoped(
        &state.account_core().pool,
        provider_id,
        id,
        ctx.owner_filter(),
    )
    .await?;
    if removed == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such provider"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/accounts/{id}/providers/{provider_id}/move` — re-parent a provider onto
/// another account of the SAME owner — manual merge for e.g.
///
/// "alice (anthropic)" + "alice (openai)" → one "alice". 409 if the target already has a
/// provider of that family.
pub async fn move_provider(
    state: &impl AccountsContext,
    ctx: &AuthContext,
    (id, provider_id): (Uuid, Uuid),
    req: MoveProvider,
) -> Result<Json<ProviderInfo>, AppError> {
    let src_owner = require_account_owner(state, ctx, id).await?;
    let tgt_owner = require_account_owner(state, ctx, req.target_account_id)
        .await
        .map_err(|_| AppError::new(StatusCode::NOT_FOUND, "no such target account"))?;
    // Same-owner only: shares confer `use`, never re-homing a credential onto
    // another user's identity (which would also flip whose sessions bill it).
    if src_owner != tgt_owner {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "provider can only move between accounts of the same owner",
        ));
    }
    let moved = sqlx::query(
        "UPDATE account_providers SET account_id = $1 \
         WHERE id = $2 AND account_id = $3 AND NOT managed",
    )
    .bind(req.target_account_id)
    .bind(provider_id)
    .bind(id)
    .execute(&state.account_core().pool)
    .await;
    match moved {
        Ok(done) if done.rows_affected() == 0 => {
            Err(AppError::new(StatusCode::NOT_FOUND, "no such provider"))
        }
        Ok(_) => {
            let info = fetch_provider_info(&state.account_core().pool, provider_id)
                .await?
                .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such provider"))?;
            Ok(Json(info))
        }
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => Err(AppError::new(
            StatusCode::CONFLICT,
            "the target account already has a provider of that family",
        )),
        Err(e) => Err(AppError::from(e)),
    }
}

// ----------------------------------------------------------------------------
// "Sign in with Claude" / "Sign in with ChatGPT" OAuth authorize flow
//
// OAuth 2.1 authorization-code + PKCE in manual paste mode: we generate a PKCE
// verifier/challenge and a nonce, the user authorizes upstream, and pastes the
// result back. Anthropic (claude.ai, `claude /login` / better-ccflare) displays
// a `code#state` pair the user copies. Codex's public client has a FIXED
// localhost:1455 redirect we can't change, so the browser redirect fails to
// load and the user copies the full callback URL from the address bar; we parse
// the `code` out of it. Token exchange differs per provider (anthropic JSON,
// openai form-encoded) and Codex's id_token carries the chatgpt_account_id we
// persist for the gateway's upstream header. Pending logins live in memory
// only, keyed by nonce + scoped to the authenticated user, single-use,
// TTL-bounded. `start` may carry an `account_id` attach target so the
// finished credential lands as a provider under an existing account.
// ----------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
pub struct OAuthStart {
    /// `anthropic` ("Sign in with Claude") or `openai` ("Sign in with `ChatGPT`").
    pub provider: String,
    /// Owning user — required (and only honoured) when authenticated with the
    /// admin token. Ignored when `account_id` names the attach target
    /// (the target's owner wins).
    #[serde(default)]
    pub user_id: Option<Uuid>,
    /// Optional attach target: finish the flow as a provider under
    /// this existing account instead of creating a new identity.
    #[serde(default)]
    pub account_id: Option<Uuid>,
}

#[derive(serde::Deserialize)]
pub struct OAuthTokenResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    expires_in: Option<i64>,
    /// OpenAI/Codex returns an OIDC `id_token` whose claims carry the
    /// `chatgpt_account_id` we need for the upstream header.
    #[serde(default)]
    id_token: Option<String>,
}

/// Extract `chatgpt_account_id` from an `OpenAI` `id_token` JWT without verifying the
/// signature (the token came straight from the trusted token endpoint over TLS).
///
/// The claim is nested under `https://api.openai.com/auth`.
pub fn chatgpt_account_id_from_id_token(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .map(str::to_string)
}

/// Parse the `code` out of an `OpenAI` callback URL (or a bare `code`/`code#state`
/// string). Accepts the full `http://localhost:1455/auth/callback?code=…&state=…`
/// the user pastes, or just the code itself.
#[must_use]
pub fn code_from_callback(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if let Some(qpos) = input.find('?') {
        for pair in input[qpos + 1..].split('&') {
            if let Some(code) = pair.strip_prefix("code=") {
                let code = code.split('#').next().unwrap_or(code);
                return Some(urldecode(code));
            }
        }
        return None;
    }
    // Bare value: strip any `#state` suffix and use it verbatim.
    Some(input.split('#').next().unwrap_or(input).to_string())
}

/// Minimal percent-decoding for the `code` query param.
#[must_use]
pub fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// PKCE S256 challenge for a verifier: base64url(sha256(verifier)), no padding.
#[must_use]
pub fn account_name_missing(name: Option<&str>) -> bool {
    name.is_none_or(|s| s.trim().is_empty())
}

#[must_use]
pub fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// Split a pasted `code#state` into its parts. claude.ai displays the pair
/// joined by `#`; some clients omit the state, so it is optional.
#[must_use]
pub fn split_code_state(pasted: &str) -> (String, Option<String>) {
    let pasted = pasted.trim();
    match pasted.split_once('#') {
        Some((code, state)) => (code.to_string(), Some(state.to_string())),
        None => (pasted.to_string(), None),
    }
}

/// Drop pending logins older than the TTL (lazy sweep on access).
pub fn sweep_expired(store: &PendingOAuthLogins) {
    let cutoff = Utc::now() - PENDING_OAUTH_TTL;
    store.retain(|_, v| v.created_at > cutoff);
}

/// `POST /api/v1/accounts/oauth/start` — begin a "Sign in with Claude" login.
///
/// Generates PKCE + a nonce, stashes a pending record, and returns the authorize URL for
/// the webui to open in a new tab. With `account_id` the finish attaches to that existing
/// account.
pub async fn oauth_start(
    state: &impl AccountsContext,
    ctx: &AuthContext,
    req: OAuthStart,
) -> Result<Json<OAuthStartResponse>, AppError> {
    // The attach target names its owner; otherwise the caller does.
    let uid = if let Some(account_id) = req.account_id {
        let owner =
            shareable_owner(Shareable::Account, account_id, &state.account_core().pool).await?;
        owner
            .filter(|o| ctx.owner_filter().is_none_or(|f| f == *o))
            .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such account"))?
    } else {
        resolve_owner(ctx, req.user_id)
            .map_err(|(s, Json(v))| AppError::new(s, v["error"].as_str().unwrap_or_default()))?
    };
    if !matches!(req.provider.as_str(), "anthropic" | "openai") {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "provider must be anthropic|openai"));
    }

    sweep_expired(state.oauth_logins());

    // PKCE verifier (high-entropy, URL-safe) doubles as the OAuth `state`, same
    // as better-ccflare. nonce keys the pending record (distinct from state so
    // the client never has to handle the verifier).
    let code_verifier = crate::auth::mint_secret();
    let nonce = crate::auth::mint_secret();
    let challenge = pkce_challenge(&code_verifier);

    let authorize_url = if req.provider == "openai" {
        // "Sign in with ChatGPT": auth.openai.com authorize with the codex
        // public client. The redirect is fixed to localhost:1455 (can't be
        // changed), so the browser redirect fails to load and the user pastes
        // the full callback URL back to us.
        format!(
            "{}?response_type=code&client_id={}&redirect_uri={}\
             &scope=openid%20profile%20email%20offline_access\
             &code_challenge={}&code_challenge_method=S256&state={}\
             &id_token_add_organizations=true&codex_cli_simplified_flow=true&prompt=login",
            gateway::openai_authorize_url(),
            urlencoding(&gateway::openai_client_id()),
            urlencoding(&gateway::openai_oauth_redirect_uri()),
            urlencoding(&challenge),
            urlencoding(&code_verifier),
        )
    } else {
        format!(
            "{}?code=true&client_id={}&response_type=code&redirect_uri={}\
             &scope=org:create_api_key%20user:profile%20user:inference\
             &code_challenge={}&code_challenge_method=S256&state={}",
            gateway::anthropic_authorize_url(),
            urlencoding(&gateway::anthropic_client_id()),
            urlencoding(&gateway::anthropic_oauth_redirect_uri()),
            urlencoding(&challenge),
            urlencoding(&code_verifier),
        )
    };

    state.oauth_logins().insert(
        nonce.clone(),
        PendingOAuthLogin {
            user_id: uid,
            provider: req.provider,
            code_verifier,
            created_at: Utc::now(),
            account_id: req.account_id,
        },
    );

    Ok(Json(OAuthStartResponse { nonce, authorize_url }))
}

/// `POST /api/v1/accounts/oauth/finish` — exchange the pasted `code#state` for tokens and
/// store the credential.
///
/// Single-use: the pending record is consumed regardless of exchange outcome. Lands as a
/// provider under the `start` attach target when one was given; otherwise finds-or-creates
/// an account identity by `name` (re-running the flow for an existing name refreshes the
/// same-family credential in place — the Reauthenticate button).
// Linear handler: consume pending record, exchange code, store per provider.
#[allow(clippy::too_many_lines)]
pub async fn oauth_finish(
    state: &impl AccountsContext,
    ctx: &AuthContext,
    req: OAuthFinish,
) -> Result<(StatusCode, Json<AccountInfo>), AppError> {
    sweep_expired(state.oauth_logins());

    // Consume the pending record (single-use), but only if it belongs to the
    // caller — never let one user finish another user's login. Admin started
    // the flow on behalf of the owner stored in the record, so it may finish
    // any pending login. The account lands on the stored owner.
    let pending = match state.oauth_logins().get(&req.nonce) {
        Some(p) if ctx.is_admin() || ctx.user_id == p.user_id => p.clone(),
        _ => return Err(AppError::new(StatusCode::BAD_REQUEST, "unknown or expired login")),
    };
    let uid = pending.user_id;
    state.oauth_logins().remove(&req.nonce);

    // Resolve the target identity BEFORE the token exchange so a bad target
    // fails fast: an attach target must still exist and belong to the pending
    // owner; otherwise `name` finds-or-creates an identity after the exchange.
    let attach_target = if let Some(account_id) = pending.account_id {
        let owner =
            shareable_owner(Shareable::Account, account_id, &state.account_core().pool).await?;
        if owner != Some(uid) {
            return Err(AppError::new(StatusCode::NOT_FOUND, "no such account"));
        }
        Some(account_id)
    } else {
        if account_name_missing(req.name.as_deref()) {
            return Err(AppError::new(StatusCode::BAD_REQUEST, "name required"));
        }
        None
    };

    // The token exchange differs per provider: anthropic posts JSON with the
    // pasted `code#state`; openai/Codex posts a form-encoded body with the code
    // extracted from the pasted callback URL.
    let resp = if pending.provider == "openai" {
        let raw = req
            .callback_url
            .as_deref()
            .or(req.code.as_deref())
            .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "callback_url required"))?;
        let code = code_from_callback(raw).filter(|c| !c.is_empty()).ok_or_else(|| {
            AppError::new(StatusCode::BAD_REQUEST, "could not find code in callback URL")
        })?;

        let form = [
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("client_id", &gateway::openai_client_id()),
            ("redirect_uri", &gateway::openai_oauth_redirect_uri()),
            ("code_verifier", pending.code_verifier.as_str()),
        ];
        state.account_core().http_client.post(gateway::openai_token_url()).form(&form).send().await
    } else {
        let raw = req
            .code
            .as_deref()
            .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "code required"))?;
        let (code, state_part) = split_code_state(raw);
        if code.is_empty() {
            return Err(AppError::new(StatusCode::BAD_REQUEST, "code required"));
        }
        // claude.ai sends `code#state`; the state must equal the verifier we issued.
        let oauth_state = state_part.unwrap_or_else(|| pending.code_verifier.clone());
        let body = serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "state": oauth_state,
            "client_id": gateway::anthropic_client_id(),
            "redirect_uri": gateway::anthropic_oauth_redirect_uri(),
            "code_verifier": pending.code_verifier,
        });
        state
            .account_core()
            .http_client
            .post(gateway::anthropic_token_url())
            .json(&body)
            .send()
            .await
    };

    let resp = resp.map_err(|e| {
        tracing::error!("oauth token exchange transport error: {e}");
        AppError::new(StatusCode::BAD_GATEWAY, "token exchange failed")
    })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        tracing::error!(%status, "oauth token exchange rejected: {detail}");
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "token exchange rejected — check the pasted code/URL",
        ));
    }
    let tok: OAuthTokenResponse = resp.json().await.map_err(|e| {
        tracing::error!("oauth token exchange decode error: {e}");
        AppError::new(StatusCode::BAD_GATEWAY, "token exchange decode failed")
    })?;

    // For Codex, pull the chatgpt account id out of the id_token so the gateway
    // can send the `Chatgpt-Account-Id` header upstream.
    let provider_account_id = tok
        .id_token
        .as_deref()
        .and_then(chatgpt_account_id_from_id_token)
        .filter(|_| pending.provider == "openai");

    let key = state.vault_key();
    let enc_refresh = cctui_crypto::encrypt(&tok.refresh_token, &key);
    let enc_access = cctui_crypto::encrypt(&tok.access_token, &key);
    let expires_at = tok.expires_in.map(|s| Utc::now() + Duration::seconds(s));

    // Resolve the identity: the attach target, or find-or-create by name.
    let account_id: Uuid = if let Some(aid) = attach_target {
        aid
    } else {
        // Safe unwrap-ish: validated non-empty above when attach_target is None.
        let name = req.name.as_deref().map(str::trim).unwrap_or_default();
        sqlx::query_scalar(
            "INSERT INTO accounts (user_id, name) VALUES ($1, $2) \
             ON CONFLICT (user_id, name) DO UPDATE SET updated_at = now() \
             RETURNING id",
        )
        .bind(uid)
        .bind(name)
        .fetch_one(&state.account_core().pool)
        .await?
    };

    // Upsert on (account_id, family): a first login inserts; re-running the flow
    // against the same account (the Reauthenticate button) refreshes the
    // same-family credential in place and clears any `needs_reauth` flag, instead
    // of 409ing on the unique index.
    let pid: Result<Uuid, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO account_providers \
            (user_id, account_id, provider, encrypted_refresh_token, encrypted_access_token, \
             expires_at, provider_account_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (account_id, family) DO UPDATE SET \
             provider                 = EXCLUDED.provider, \
             encrypted_refresh_token  = EXCLUDED.encrypted_refresh_token, \
             encrypted_access_token   = EXCLUDED.encrypted_access_token, \
             expires_at               = EXCLUDED.expires_at, \
             provider_account_id      = EXCLUDED.provider_account_id, \
             needs_reauth = false, last_auth_error = NULL, last_auth_error_at = NULL \
         RETURNING id",
    )
    .bind(uid)
    .bind(account_id)
    .bind(&pending.provider)
    .bind(&enc_refresh)
    .bind(&enc_access)
    .bind(expires_at)
    .bind(&provider_account_id)
    .fetch_one(&state.account_core().pool)
    .await;

    match pid {
        Ok(pid) => {
            // Fresh credentials → drop the in-memory reauth gate too, so
            // the gateway's success path doesn't think it still needs clearing.
            state.account_core().account_reauth.remove(&pid);
            let info = fetch_account_info(state, account_id, None).await?.ok_or_else(|| {
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "account vanished after login")
            })?;
            Ok((StatusCode::CREATED, Json(info)))
        }
        Err(e) => Err(AppError::from(e)),
    }
}

/// Minimal percent-encoding for query-string components (no extra deps): encode
/// everything outside the RFC 3986 unreserved set.
#[must_use]
pub fn urlencoding(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

pub async fn require_account_owner(
    state: &impl AccountsContext,
    ctx: &AuthContext,
    id: Uuid,
) -> Result<Uuid, AppError> {
    let owner: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM accounts \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .fetch_optional(&state.account_core().pool)
    .await?;
    owner.ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such account"))
}
