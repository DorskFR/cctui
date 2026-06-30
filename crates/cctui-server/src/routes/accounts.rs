//! `/api/v1/accounts` — the OAuth account vault (CCT-232).
//!
//! Users register **named OAuth accounts** for Claude Code / Codex (e.g.
//! `personal`, `enterprise`) and pick one per job at spawn/dispatch time. The
//! OAuth refresh token is encrypted at rest with the vault key (`crate::crypto`,
//! same as `api_keys`/`dispatchers`) and is **never** returned over the API —
//! list/get only ever surface name/provider/expiry/last-used + lightweight
//! stats. Accounts belong to the registering user and are visible/usable only by
//! that user (`require_user`).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::{AuthContext, Scope};
use crate::routes::gateway;
use crate::state::AppState;

/// How long a pending "Sign in with Claude" login stays valid (CCT-243).
const PENDING_OAUTH_TTL: Duration = Duration::minutes(10);

/// In-memory store of pending OAuth logins, keyed by nonce (CCT-243).
pub type PendingOAuthLogins = Arc<DashMap<String, PendingOAuthLogin>>;

/// A pending "Sign in with Claude" login: the PKCE verifier we generated and the
/// user it belongs to, with a creation timestamp for TTL expiry. Held only in
/// memory and deleted on finish (single-use).
#[derive(Clone)]
pub struct PendingOAuthLogin {
    pub user_id: Uuid,
    pub provider: String,
    pub code_verifier: String,
    pub created_at: DateTime<Utc>,
}

/// Resolve which user an account operation targets (CCT-251). A user token
/// always acts as itself; the env admin token has no user identity, so it must
/// name the owner explicitly (`user_id` in the request). This is what lets an
/// admin-authed webui run the "Sign in with Claude/ChatGPT" flows instead of
/// bouncing off "user token required".
fn resolve_owner(
    ctx: &AuthContext,
    explicit: Option<Uuid>,
) -> Result<Uuid, (StatusCode, Json<serde_json::Value>)> {
    // A machine key has no business creating accounts (CCT-410): require a
    // human identity (read scope, no machine id). An admin acts cross-user by
    // naming the owner explicitly; a user acts as itself.
    if ctx.machine_id.is_some() || !ctx.has(Scope::Read) {
        return Err(err(StatusCode::FORBIDDEN, "user or admin token required"));
    }
    if ctx.is_admin() {
        explicit.ok_or_else(|| {
            err(StatusCode::BAD_REQUEST, "user_id required when using the admin token")
        })
    } else {
        Ok(ctx.user_id)
    }
}

/// Gate the account read/mutation routes to a human identity (a user or admin
/// token, never a machine key), matching the pre-CCT-410 `require_user`/admin
/// behavior. Admin then sees/acts across all owners via `owner_filter`.
fn require_human(ctx: &AuthContext) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if ctx.machine_id.is_some() || !ctx.has(Scope::Read) {
        return Err(err(StatusCode::FORBIDDEN, "user or admin token required"));
    }
    Ok(())
}

/// One-release back-compat shim (CCT-399): if `CCTUI_CLAUDE_LITELLM_*` is set,
/// synthesize a server-owned **managed** anthropic-compatible account per user so
/// existing deployments keep working after the env-var path is retired. Managed
/// accounts are read-only over the API (rename/delete excluded). Idempotent:
/// re-upserted on every restart against the partial unique index
/// `(user_id, provider) WHERE managed`. A no-op unless both the endpoint and the
/// model list are configured. To be removed in a follow-up release.
pub async fn sync_litellm_shim(pool: &sqlx::PgPool, config: &crate::config::Config) {
    let Some(endpoint) = config.claude_litellm_endpoint.as_deref() else { return };
    let models = config.claude_litellm_visible_models();
    if models.is_empty() {
        return;
    }
    let key = crate::crypto::vault_key();
    let cred = config.claude_litellm_token.as_deref().unwrap_or("sk-dummy");
    let enc_access = crate::crypto::obfuscate(cred, &key);
    let models_json = serde_json::to_value(
        models
            .iter()
            .map(|m| AccountModel { model: m.model.clone(), label: m.label.clone() })
            .collect::<Vec<_>>(),
    )
    .unwrap_or(serde_json::Value::Null);

    // One managed account per user, keyed by (user_id, provider) WHERE managed.
    let users: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE revoked_at IS NULL")
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    for uid in users {
        let res = sqlx::query(
            "INSERT INTO oauth_accounts \
                (user_id, name, provider, encrypted_access_token, base_url, models, \
                 auth_scheme, managed) \
             VALUES ($1, 'litellm (legacy)', 'anthropic-compatible', $2, $3, $4, 'bearer', TRUE) \
             ON CONFLICT (user_id, provider) WHERE managed DO UPDATE \
               SET encrypted_access_token = EXCLUDED.encrypted_access_token, \
                   base_url = EXCLUDED.base_url, models = EXCLUDED.models",
        )
        .bind(uid)
        .bind(&enc_access)
        .bind(endpoint)
        .bind(&models_json)
        .execute(pool)
        .await;
        if let Err(e) = res {
            tracing::warn!(%uid, "litellm shim upsert failed: {e}");
        }
    }
    tracing::info!("CCTUI_CLAUDE_LITELLM_* shim: synced managed compatible accounts (CCT-399)");
}

/// One selectable model on a compatible-endpoint account (CCT-399): `model` is
/// the `--model` code, `label` the display name. Safe to return over the API —
/// model names are not secret (unlike the base URL + credential).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccountModel {
    pub model: String,
    pub label: String,
}

/// API view of an account — secrets (tokens), the base URL, and the auth scheme
/// are deliberately absent (CCT-399). Only `models` (safe) is surfaced.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct AccountInfo {
    pub id: Uuid,
    pub name: String,
    pub provider: String,
    /// Selectable models for a compatible endpoint (CCT-399). `None`/empty for
    /// native subscription accounts (they use the harness's native families).
    /// JSONB at rest; safe to return — names aren't secret.
    pub models: Option<serde_json::Value>,
    /// Per-account logical→concrete model alias map (CCT-406), e.g.
    /// `{"opus": "claude-opus-4-8[1m]"}`. Applies to every provider; resolved
    /// server-side at spawn. JSONB object at rest; safe to return.
    pub model_aliases: Option<serde_json::Value>,
    /// `true` for a server-synthesized (managed) account — read-only over the
    /// API (the back-compat shim for `CCTUI_CLAUDE_LITELLM_*`, CCT-399).
    pub managed: bool,
    /// Owning user (CCT-251) — admins see all accounts, so the owner matters.
    pub user_id: Uuid,
    /// Owner's name for display; only populated on the list query's join.
    pub user_name: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub request_count: i64,
    pub bytes_transferred: i64,
    /// Total tokens (input + output + cache) attributed to this account across
    /// all its sessions (CCT-273). Joined from `session_tokens` →
    /// `session_token_usage` at read time.
    pub total_tokens: i64,
    /// Rough USD cost estimate derived from `total_tokens` using a per-provider
    /// blended rate (CCT-273). An estimate only — OAuth/subscription accounts
    /// aren't metered per token; this is a usage-weight signal, not a bill.
    pub est_cost_usd: f64,
    /// Per-account soft limits on cctui's own share of the subscription windows
    /// (CCT-411). NULL ⇒ no soft limit on that window. Safe to return — they're
    /// config, not secrets.
    pub soft_limit_5h_pct: Option<i32>,
    pub soft_limit_7d_pct: Option<i32>,
    pub soft_limit_bypass_minutes: Option<i32>,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateAccount {
    pub name: String,
    /// `anthropic` | `openai` (native subscription) | `anthropic-compatible` |
    /// `openai-compatible` (CCT-399).
    pub provider: String,
    /// OAuth refresh token (subscription accounts). Optional for compatible
    /// endpoints, which store only a static credential (in `access_token`).
    /// Stored encrypted; the gateway exchanges it for access tokens on demand.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Initial access token (subscription) OR the static credential (a compatible
    /// endpoint's bearer/api key, CCT-399). Stored encrypted; never read back.
    #[serde(default)]
    pub access_token: Option<String>,
    /// Optional access-token expiry (unix seconds). When absent the gateway
    /// refreshes on first use (subscription) / never refreshes (compatible).
    #[serde(default)]
    pub expires_at: Option<i64>,
    /// Compatible-endpoint base URL (CCT-399), e.g. a LiteLLM/vLLM/Ollama-proxy.
    /// Required for `*-compatible` providers; ignored for native ones. Never
    /// returned over the API.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Selectable models for a compatible endpoint (CCT-399).
    #[serde(default)]
    pub models: Option<Vec<AccountModel>>,
    /// Logical→concrete model alias map (CCT-406), e.g.
    /// `{"opus": "claude-opus-4-8[1m]"}`. Honoured for every provider.
    #[serde(default)]
    pub model_aliases: Option<std::collections::HashMap<String, String>>,
    /// Credential scheme for a compatible endpoint: `bearer` | `api_key`
    /// (CCT-399). Defaults to `bearer`. Native accounts are always `oauth`.
    #[serde(default)]
    pub auth_scheme: Option<String>,
    /// Owning user — required (and only honoured) when authenticated with the
    /// admin token, which has no user identity of its own (CCT-251).
    #[serde(default)]
    pub user_id: Option<Uuid>,
    /// Per-account soft limits (CCT-411). All optional; absent ⇒ NULL (no cap).
    #[serde(default)]
    pub soft_limit_5h_pct: Option<i32>,
    #[serde(default)]
    pub soft_limit_7d_pct: Option<i32>,
    #[serde(default)]
    pub soft_limit_bypass_minutes: Option<i32>,
}

/// `PATCH /api/v1/accounts/{id}` payload (CCT-402). A partial update: `name`
/// renames (back-compat — the only field native accounts allow). For a
/// non-managed compatible endpoint the operator may also edit `models`,
/// `base_url`, `auth_scheme`, and rotate the static credential (`access_token`).
/// All optional; an absent field leaves that column unchanged. base_url/credential
/// are never returned, so the editor re-supplies base_url when changing it and
/// leaves the credential blank to keep the stored one.
#[derive(Debug, serde::Deserialize)]
pub struct UpdateAccount {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub auth_scheme: Option<String>,
    #[serde(default)]
    pub models: Option<Vec<AccountModel>>,
    /// Replacement model alias map (CCT-406). Provided → replaces the stored map
    /// wholesale (an empty object clears it); absent → unchanged. Editable for
    /// every provider, unlike the compatible-only fields above.
    #[serde(default)]
    pub model_aliases: Option<std::collections::HashMap<String, String>>,
    /// New static credential for a compatible endpoint; blank/absent keeps the
    /// stored one.
    #[serde(default)]
    pub access_token: Option<String>,
    /// Replacement soft-limit config (CCT-411). Provided → each of the three
    /// columns is set to its value (a null field clears that column); absent →
    /// all three unchanged. Editable for every provider, like model aliases.
    #[serde(default)]
    pub soft_limits: Option<SoftLimitPatch>,
}

/// The three soft-limit columns as a patchable block (CCT-411). A field left
/// `null`/absent inside a provided block clears that column.
#[derive(Debug, serde::Deserialize)]
pub struct SoftLimitPatch {
    #[serde(default)]
    pub soft_limit_5h_pct: Option<i32>,
    #[serde(default)]
    pub soft_limit_7d_pct: Option<i32>,
    #[serde(default)]
    pub soft_limit_bypass_minutes: Option<i32>,
}

fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(serde_json::json!({ "error": msg })))
}

/// `GET /api/v1/accounts` — the caller's own accounts (tokens never returned).
/// Admin sees every account, with the owner's name joined in (CCT-251).
pub async fn list_accounts(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<AccountInfo>>, (StatusCode, Json<serde_json::Value>)> {
    require_human(&ctx)?;
    // Per-account token totals + a rough USD cost estimate (CCT-273). Tokens
    // are recorded per session (`session_token_usage`); `session_tokens` bridges
    // a session to the account it ran under. SUM() over bigint returns NUMERIC,
    // so cast back to bigint for the i64 columns. Cost uses a per-provider
    // blended per-million rate (input/output/cache weighted) — an estimate, not
    // a meter (these are subscription accounts).
    let rows: Vec<AccountInfo> = sqlx::query_as(
        "SELECT a.id, a.name, a.provider, a.models, a.model_aliases, a.managed, a.user_id, u.name AS user_name, \
                a.expires_at, a.created_at, a.last_used_at, \
                a.request_count, a.bytes_transferred, \
                a.soft_limit_5h_pct, a.soft_limit_7d_pct, a.soft_limit_bypass_minutes, \
                (COALESCE(t.input_tokens,0) + COALESCE(t.output_tokens,0) \
                 + COALESCE(t.cache_read_tokens,0) + COALESCE(t.cache_creation_tokens,0))::bigint \
                  AS total_tokens, \
                (CASE a.provider \
                   WHEN 'openai' THEN \
                     COALESCE(t.input_tokens,0)*1.25 + COALESCE(t.output_tokens,0)*10 \
                     + COALESCE(t.cache_read_tokens,0)*0.125 + COALESCE(t.cache_creation_tokens,0)*1.25 \
                   ELSE \
                     COALESCE(t.input_tokens,0)*3 + COALESCE(t.output_tokens,0)*15 \
                     + COALESCE(t.cache_read_tokens,0)*0.3 + COALESCE(t.cache_creation_tokens,0)*3.75 \
                 END / 1000000.0)::double precision AS est_cost_usd \
         FROM oauth_accounts a JOIN users u ON u.id = a.user_id \
         LEFT JOIN ( \
             SELECT st.account_id, \
                    SUM(stu.input_tokens)          AS input_tokens, \
                    SUM(stu.output_tokens)         AS output_tokens, \
                    SUM(stu.cache_read_tokens)     AS cache_read_tokens, \
                    SUM(stu.cache_creation_tokens) AS cache_creation_tokens \
             FROM session_tokens st \
             JOIN session_token_usage stu ON stu.session_id = st.session_id \
             GROUP BY st.account_id \
         ) t ON t.account_id = a.id \
         WHERE $1::uuid IS NULL OR a.user_id = $1 \
         ORDER BY a.provider, a.name",
    )
    .bind(ctx.owner_filter())
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    Ok(Json(rows))
}

/// `POST /api/v1/accounts` — register a named OAuth account.
pub async fn create_account(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateAccount>,
) -> Result<(StatusCode, Json<AccountInfo>), (StatusCode, Json<serde_json::Value>)> {
    let uid = resolve_owner(&ctx, req.user_id)?;
    if req.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }
    let compatible = matches!(req.provider.as_str(), "anthropic-compatible" | "openai-compatible");
    if !matches!(
        req.provider.as_str(),
        "anthropic" | "openai" | "anthropic-compatible" | "openai-compatible"
    ) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "provider must be anthropic|openai|anthropic-compatible|openai-compatible",
        ));
    }

    let key = crate::crypto::vault_key();
    // Native subscription accounts: an OAuth refresh token, auth_scheme = oauth.
    // Compatible endpoints (CCT-399): a base URL + a static credential stored in
    // encrypted_access_token, no refresh token, auth_scheme = bearer|api_key.
    let (enc_refresh, enc_access, base_url, auth_scheme): (
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    );
    if compatible {
        let base = req.base_url.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let Some(base) = base else {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "base_url required for a compatible endpoint",
            ));
        };
        // SSRF is explicitly out of scope for this single-operator, self-hosted
        // deployment (CCT-399 decision); a light scheme check only. Prefer https.
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(err(StatusCode::BAD_REQUEST, "base_url must be an http(s) URL"));
        }
        let scheme = req.auth_scheme.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let scheme = scheme.unwrap_or("bearer");
        if !matches!(scheme, "bearer" | "api_key") {
            return Err(err(StatusCode::BAD_REQUEST, "auth_scheme must be bearer|api_key"));
        }
        // A static credential is optional (an open proxy accepts any value); when
        // absent we still store a dummy so the gateway has a bearer to forward.
        let cred = req
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("sk-dummy");
        enc_refresh = None;
        enc_access = Some(crate::crypto::obfuscate(cred, &key));
        base_url = Some(base.to_owned());
        auth_scheme = scheme.to_owned();
    } else {
        let refresh = req.refresh_token.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let Some(refresh) = refresh else {
            return Err(err(StatusCode::BAD_REQUEST, "refresh_token required"));
        };
        enc_refresh = Some(crate::crypto::obfuscate(refresh, &key));
        enc_access = req.access_token.as_deref().map(|t| crate::crypto::obfuscate(t, &key));
        base_url = None;
        auth_scheme = "oauth".to_owned();
    }
    let expires_at = req.expires_at.and_then(|s| DateTime::<Utc>::from_timestamp(s, 0));
    let models = req
        .models
        .as_ref()
        .filter(|m| !m.is_empty())
        .map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null));
    // Alias map (CCT-406): an empty map stores NULL (no remapping).
    let model_aliases = req
        .model_aliases
        .as_ref()
        .filter(|m| !m.is_empty())
        .map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null));

    let row: Result<AccountInfo, sqlx::Error> = sqlx::query_as(
        "INSERT INTO oauth_accounts \
            (user_id, name, provider, encrypted_refresh_token, encrypted_access_token, \
             expires_at, base_url, models, auth_scheme, model_aliases, \
             soft_limit_5h_pct, soft_limit_7d_pct, soft_limit_bypass_minutes) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
         RETURNING id, name, provider, models, model_aliases, managed, user_id, NULL::text AS user_name, \
                   expires_at, created_at, last_used_at, \
                   request_count, bytes_transferred, \
                   soft_limit_5h_pct, soft_limit_7d_pct, soft_limit_bypass_minutes, \
                   0::bigint AS total_tokens, 0::double precision AS est_cost_usd",
    )
    .bind(uid)
    .bind(req.name.trim())
    .bind(&req.provider)
    .bind(&enc_refresh)
    .bind(&enc_access)
    .bind(expires_at)
    .bind(&base_url)
    .bind(&models)
    .bind(&auth_scheme)
    .bind(&model_aliases)
    .bind(req.soft_limit_5h_pct)
    .bind(req.soft_limit_7d_pct)
    .bind(req.soft_limit_bypass_minutes)
    .fetch_one(&state.pool)
    .await;

    match row {
        Ok(info) => Ok((StatusCode::CREATED, Json(info))),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            Err(err(StatusCode::CONFLICT, "an account with that name+provider already exists"))
        }
        Err(e) => {
            tracing::error!("db error: {e}");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "database error"))
        }
    }
}

/// `PATCH /api/v1/accounts/{id}` — rename, and (CCT-402) edit a non-managed
/// compatible endpoint's models / base URL / auth scheme / credential without
/// recreating it. Native subscription accounts only honour `name`.
pub async fn rename_account(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateAccount>,
) -> Result<Json<AccountInfo>, (StatusCode, Json<serde_json::Value>)> {
    require_human(&ctx)?;
    if let Some(name) = req.name.as_deref()
        && name.trim().is_empty()
    {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }

    // Resolve the target (scoped to the caller; admin sees all) so we can tell a
    // compatible endpoint from a native one and reject editing managed accounts.
    let provider: Option<(String,)> = sqlx::query_as(
        "SELECT provider FROM oauth_accounts \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) AND NOT managed",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    let Some((provider,)) = provider else {
        return Err(err(StatusCode::NOT_FOUND, "no such account"));
    };
    let compatible = matches!(provider.as_str(), "anthropic-compatible" | "openai-compatible");

    // Compatible-only fields are rejected for native accounts so the edit form
    // can't silently no-op against a subscription account.
    if !compatible
        && (req.base_url.is_some()
            || req.auth_scheme.is_some()
            || req.models.is_some()
            || req.access_token.is_some())
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "only the name is editable for a native subscription account",
        ));
    }

    let key = crate::crypto::vault_key();
    // Normalise the optional compatible-endpoint fields. base_url/auth_scheme are
    // validated like create; a blank credential keeps the stored one (NULL bind →
    // COALESCE no-op in SQL). models replaces the list wholesale when provided.
    let base_url = match req.base_url.as_deref().map(str::trim) {
        Some(b) if !b.is_empty() => {
            if !(b.starts_with("http://") || b.starts_with("https://")) {
                return Err(err(StatusCode::BAD_REQUEST, "base_url must be an http(s) URL"));
            }
            Some(b.to_owned())
        }
        _ => None,
    };
    let auth_scheme = match req.auth_scheme.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => {
            if !matches!(s, "bearer" | "api_key") {
                return Err(err(StatusCode::BAD_REQUEST, "auth_scheme must be bearer|api_key"));
            }
            Some(s.to_owned())
        }
        _ => None,
    };
    let models =
        req.models.as_ref().map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null));
    // Aliases (CCT-406): COALESCE can't distinguish "clear" from "unchanged"
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
        .map(|c| crate::crypto::obfuscate(c, &key));
    let name = req.name.as_deref().map(str::trim).map(str::to_owned);
    // Soft limits (CCT-411): like aliases, carry a provided-flag so a provided
    // block replaces all three columns (a null field clears one) while an absent
    // block leaves them untouched.
    let soft_provided = req.soft_limits.is_some();
    let (soft_5h, soft_7d, soft_bypass) = req
        .soft_limits
        .as_ref()
        .map(|s| (s.soft_limit_5h_pct, s.soft_limit_7d_pct, s.soft_limit_bypass_minutes))
        .unwrap_or((None, None, None));

    // COALESCE keeps each column when its bind is NULL, so an absent field is a
    // no-op. Admin (`ctx.user_id` = NULL) may edit any account; a user only its
    // own. Managed accounts are excluded.
    let row: Option<AccountInfo> = sqlx::query_as(
        "UPDATE oauth_accounts SET \
            name = COALESCE($3, name), \
            base_url = COALESCE($4, base_url), \
            auth_scheme = COALESCE($5, auth_scheme), \
            models = COALESCE($6, models), \
            encrypted_access_token = COALESCE($7, encrypted_access_token), \
            model_aliases = CASE WHEN $8 THEN $9 ELSE model_aliases END, \
            soft_limit_5h_pct = CASE WHEN $10 THEN $11 ELSE soft_limit_5h_pct END, \
            soft_limit_7d_pct = CASE WHEN $10 THEN $12 ELSE soft_limit_7d_pct END, \
            soft_limit_bypass_minutes = CASE WHEN $10 THEN $13 ELSE soft_limit_bypass_minutes END \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) AND NOT managed \
         RETURNING id, name, provider, models, model_aliases, managed, user_id, NULL::text AS user_name, \
                   expires_at, created_at, last_used_at, \
                   request_count, bytes_transferred, \
                   soft_limit_5h_pct, soft_limit_7d_pct, soft_limit_bypass_minutes, \
                   0::bigint AS total_tokens, 0::double precision AS est_cost_usd",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .bind(&name)
    .bind(&base_url)
    .bind(&auth_scheme)
    .bind(&models)
    .bind(&enc_access)
    .bind(aliases_provided)
    .bind(&model_aliases)
    .bind(soft_provided)
    .bind(soft_5h)
    .bind(soft_7d)
    .bind(soft_bypass)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    row.map(Json).ok_or_else(|| err(StatusCode::NOT_FOUND, "no such account"))
}

/// `DELETE /api/v1/accounts/{id}` — delete (cascades session_tokens).
pub async fn delete_account(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    require_human(&ctx)?;
    // Admin (`ctx.user_id` = NULL) may delete any account; a user only its own.
    let res = sqlx::query(
        "DELETE FROM oauth_accounts WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) \
         AND NOT managed",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .execute(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    if res.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such account"));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ----------------------------------------------------------------------------
// "Sign in with Claude" / "Sign in with ChatGPT" OAuth authorize flow
// (CCT-243 anthropic, CCT-244 openai/Codex)
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
// TTL-bounded.
// ----------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
pub struct OAuthStart {
    /// `anthropic` ("Sign in with Claude") or `openai` ("Sign in with ChatGPT").
    pub provider: String,
    /// Owning user — required (and only honoured) when authenticated with the
    /// admin token (CCT-251).
    #[serde(default)]
    pub user_id: Option<Uuid>,
}

#[derive(Debug, serde::Serialize)]
pub struct OAuthStartResponse {
    pub nonce: String,
    pub authorize_url: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct OAuthFinish {
    pub nonce: String,
    pub name: String,
    /// anthropic: the `code#state` pair pasted from claude.ai (the `#state`
    /// suffix is optional). Either this or `callback_url` must be present.
    #[serde(default)]
    pub code: Option<String>,
    /// openai/Codex: the full `http://localhost:1455/auth/callback?code=…&state=…`
    /// URL the user copies from the browser address bar after the redirect fails
    /// to load (the fixed redirect can't reach cctui — CCT-244).
    #[serde(default)]
    pub callback_url: Option<String>,
}

#[derive(serde::Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    expires_in: Option<i64>,
    /// OpenAI/Codex returns an OIDC `id_token` whose claims carry the
    /// `chatgpt_account_id` we need for the upstream header (CCT-244).
    #[serde(default)]
    id_token: Option<String>,
}

/// Extract `chatgpt_account_id` from an OpenAI `id_token` JWT without verifying
/// the signature (the token came straight from the trusted token endpoint over
/// TLS). The claim is nested under `https://api.openai.com/auth` (CCT-244).
fn chatgpt_account_id_from_id_token(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .map(str::to_string)
}

/// Parse the `code` out of an OpenAI callback URL (or a bare `code`/`code#state`
/// string). Accepts the full `http://localhost:1455/auth/callback?code=…&state=…`
/// the user pastes, or just the code itself (CCT-244).
fn code_from_callback(input: &str) -> Option<String> {
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
fn urldecode(s: &str) -> String {
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
fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// Split a pasted `code#state` into its parts. claude.ai displays the pair
/// joined by `#`; some clients omit the state, so it is optional.
fn split_code_state(pasted: &str) -> (String, Option<String>) {
    let pasted = pasted.trim();
    match pasted.split_once('#') {
        Some((code, state)) => (code.to_string(), Some(state.to_string())),
        None => (pasted.to_string(), None),
    }
}

/// Drop pending logins older than the TTL (lazy sweep on access).
fn sweep_expired(store: &PendingOAuthLogins) {
    let cutoff = Utc::now() - PENDING_OAUTH_TTL;
    store.retain(|_, v| v.created_at > cutoff);
}

/// `POST /api/v1/accounts/oauth/start` — begin a "Sign in with Claude" login.
/// Generates PKCE + a nonce, stashes a pending record, and returns the
/// authorize URL for the webui to open in a new tab.
pub async fn oauth_start(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<OAuthStart>,
) -> Result<Json<OAuthStartResponse>, (StatusCode, Json<serde_json::Value>)> {
    let uid = resolve_owner(&ctx, req.user_id)?;
    if !matches!(req.provider.as_str(), "anthropic" | "openai") {
        return Err(err(StatusCode::BAD_REQUEST, "provider must be anthropic|openai"));
    }

    sweep_expired(&state.pending_oauth_logins);

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
        // the full callback URL back to us (CCT-244).
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

    state.pending_oauth_logins.insert(
        nonce.clone(),
        PendingOAuthLogin {
            user_id: uid,
            provider: req.provider,
            code_verifier,
            created_at: Utc::now(),
        },
    );

    Ok(Json(OAuthStartResponse { nonce, authorize_url }))
}

/// `POST /api/v1/accounts/oauth/finish` — exchange the pasted `code#state` for
/// tokens and store the account (same shape as POST /accounts). Single-use: the
/// pending record is consumed regardless of exchange outcome.
pub async fn oauth_finish(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<OAuthFinish>,
) -> Result<(StatusCode, Json<AccountInfo>), (StatusCode, Json<serde_json::Value>)> {
    require_human(&ctx)?;
    if req.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }

    sweep_expired(&state.pending_oauth_logins);

    // Consume the pending record (single-use), but only if it belongs to the
    // caller — never let one user finish another user's login. Admin started
    // the flow on behalf of the owner stored in the record, so it may finish
    // any pending login (CCT-251). The account lands on the stored owner.
    let pending = match state.pending_oauth_logins.get(&req.nonce) {
        Some(p) if ctx.is_admin() || ctx.user_id == p.user_id => p.clone(),
        _ => return Err(err(StatusCode::BAD_REQUEST, "unknown or expired login")),
    };
    let uid = pending.user_id;
    state.pending_oauth_logins.remove(&req.nonce);

    // The token exchange differs per provider: anthropic posts JSON with the
    // pasted `code#state`; openai/Codex posts a form-encoded body with the code
    // extracted from the pasted callback URL (CCT-244).
    let resp = if pending.provider == "openai" {
        let raw = req
            .callback_url
            .as_deref()
            .or(req.code.as_deref())
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, "callback_url required"))?;
        let code = code_from_callback(raw)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, "could not find code in callback URL"))?;

        let form = [
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("client_id", &gateway::openai_client_id()),
            ("redirect_uri", &gateway::openai_oauth_redirect_uri()),
            ("code_verifier", pending.code_verifier.as_str()),
        ];
        state.http_client.post(gateway::openai_token_url()).form(&form).send().await
    } else {
        let raw =
            req.code.as_deref().ok_or_else(|| err(StatusCode::BAD_REQUEST, "code required"))?;
        let (code, state_part) = split_code_state(raw);
        if code.is_empty() {
            return Err(err(StatusCode::BAD_REQUEST, "code required"));
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
        state.http_client.post(gateway::anthropic_token_url()).json(&body).send().await
    };

    let resp = resp.map_err(|e| {
        tracing::error!("oauth token exchange transport error: {e}");
        err(StatusCode::BAD_GATEWAY, "token exchange failed")
    })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        tracing::error!(%status, "oauth token exchange rejected: {detail}");
        return Err(err(
            StatusCode::BAD_REQUEST,
            "token exchange rejected — check the pasted code/URL",
        ));
    }
    let tok: OAuthTokenResponse = resp.json().await.map_err(|e| {
        tracing::error!("oauth token exchange decode error: {e}");
        err(StatusCode::BAD_GATEWAY, "token exchange decode failed")
    })?;

    // For Codex, pull the chatgpt account id out of the id_token so the gateway
    // can send the `Chatgpt-Account-Id` header upstream.
    let provider_account_id = tok
        .id_token
        .as_deref()
        .and_then(chatgpt_account_id_from_id_token)
        .filter(|_| pending.provider == "openai");

    let key = crate::crypto::vault_key();
    let enc_refresh = crate::crypto::obfuscate(&tok.refresh_token, &key);
    let enc_access = crate::crypto::obfuscate(&tok.access_token, &key);
    let expires_at = tok.expires_in.map(|s| Utc::now() + Duration::seconds(s));

    let row: Result<AccountInfo, sqlx::Error> = sqlx::query_as(
        "INSERT INTO oauth_accounts \
            (user_id, name, provider, encrypted_refresh_token, encrypted_access_token, \
             expires_at, provider_account_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         RETURNING id, name, provider, models, managed, user_id, NULL::text AS user_name, \
                   expires_at, created_at, last_used_at, \
                   request_count, bytes_transferred, \
                   0::bigint AS total_tokens, 0::double precision AS est_cost_usd",
    )
    .bind(uid)
    .bind(req.name.trim())
    .bind(&pending.provider)
    .bind(&enc_refresh)
    .bind(&enc_access)
    .bind(expires_at)
    .bind(&provider_account_id)
    .fetch_one(&state.pool)
    .await;

    match row {
        Ok(info) => Ok((StatusCode::CREATED, Json(info))),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            Err(err(StatusCode::CONFLICT, "an account with that name+provider already exists"))
        }
        Err(e) => {
            tracing::error!("db error: {e}");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "database error"))
        }
    }
}

/// Minimal percent-encoding for query-string components (no extra deps): encode
/// everything outside the RFC 3986 unreserved set.
fn urlencoding(s: &str) -> String {
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

/// How long a cached usage fetch is served before we re-hit upstream (CCT-306).
/// Anthropic's usage endpoint rate-limits per access token (safe at ~180s); we
/// cache for a few minutes so a viewed accounts page + slow background poll never
/// spams it, and many clients share one entry per account.
pub(crate) const USAGE_CACHE_TTL: Duration = Duration::minutes(3);

/// Usage windows surfaced per account (CCT-306). `usage` mirrors Anthropic's free
/// OAuth usage payload (`five_hour`/`seven_day` utilization + reset timestamps);
/// `None` means the provider has no usage API (Codex) or the account has no
/// active windows — the webui hides the indicator in that case.
#[derive(Debug, serde::Serialize)]
pub struct AccountUsage {
    pub account_id: Uuid,
    pub provider: String,
    /// Raw upstream usage JSON (passed through verbatim) or `null`.
    pub usage: Option<serde_json::Value>,
    /// Seconds since this usage was fetched upstream (0 = just now). Lets the UI
    /// show staleness; values refresh on the slow cache TTL, not per request.
    pub age_secs: u64,
}

/// `GET /api/v1/accounts/{id}/usage` — current subscription usage for an account
/// (CCT-306). Free + tokenless: for anthropic accounts this hits Anthropic's
/// OAuth usage endpoint (5h/7d window utilization), served from a slow-refresh
/// per-account cache so we never spam the rate-limited upstream. OpenAI/codex
/// accounts have no such API, so the 5h/7d windows are metered locally from
/// recorded token usage (CCT-511) — same shape, same cache, same UI chip.
/// Ownership: a user may only read their own accounts; admin may read any.
pub async fn account_usage(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<AccountUsage>, (StatusCode, Json<serde_json::Value>)> {
    require_human(&ctx)?;
    // Authorize + resolve provider in one go. Admin (`ctx.user_id` = NULL) may
    // read any account; a user only its own.
    let provider: Option<String> = sqlx::query_scalar(
        "SELECT provider FROM oauth_accounts \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    let Some(provider) = provider else {
        return Err(err(StatusCode::NOT_FOUND, "no such account"));
    };

    // Serve a fresh-enough cached value without touching upstream.
    if let Some(hit) = state.account_usage_cache.get(&id)
        && hit.fetched_at.elapsed() < USAGE_CACHE_TTL.to_std().unwrap_or_default()
    {
        let age_secs = hit.fetched_at.elapsed().as_secs();
        return Ok(Json(AccountUsage {
            account_id: id,
            provider,
            usage: hit.usage.clone(),
            age_secs,
        }));
    }

    // Stale or absent → fetch upstream (anthropic only; Codex returns None).
    let usage = match gateway::fetch_account_usage(&state, id).await {
        Ok(u) => u,
        Err(_) => {
            // Upstream hiccup (e.g. 429/refresh fail): fall back to the last
            // cached value if we have one rather than erroring the whole row.
            if let Some(hit) = state.account_usage_cache.get(&id) {
                let age_secs = hit.fetched_at.elapsed().as_secs();
                return Ok(Json(AccountUsage {
                    account_id: id,
                    provider,
                    usage: hit.usage.clone(),
                    age_secs,
                }));
            }
            // No prior value — surface as "no usage" so the UI just hides the chip.
            None
        }
    };
    state.account_usage_cache.insert(
        id,
        crate::state::CachedUsage { fetched_at: std::time::Instant::now(), usage: usage.clone() },
    );
    Ok(Json(AccountUsage { account_id: id, provider, usage, age_secs: 0 }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_code_and_state() {
        let (code, state) = split_code_state("abc123#xyz789");
        assert_eq!(code, "abc123");
        assert_eq!(state.as_deref(), Some("xyz789"));
    }

    #[test]
    fn splits_code_without_state() {
        let (code, state) = split_code_state("  abc123  ");
        assert_eq!(code, "abc123");
        assert_eq!(state, None);
    }

    #[test]
    fn pkce_challenge_is_url_safe_b64_of_sha256() {
        // RFC 7636 Appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(pkce_challenge(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn sweep_drops_only_expired() {
        let store: PendingOAuthLogins = Arc::new(DashMap::new());
        store.insert(
            "fresh".into(),
            PendingOAuthLogin {
                user_id: Uuid::new_v4(),
                provider: "anthropic".into(),
                code_verifier: "v".into(),
                created_at: Utc::now(),
            },
        );
        store.insert(
            "stale".into(),
            PendingOAuthLogin {
                user_id: Uuid::new_v4(),
                provider: "anthropic".into(),
                code_verifier: "v".into(),
                created_at: Utc::now() - Duration::minutes(20),
            },
        );
        sweep_expired(&store);
        assert!(store.contains_key("fresh"));
        assert!(!store.contains_key("stale"));
    }

    #[test]
    fn extracts_code_from_callback_url() {
        assert_eq!(
            code_from_callback("http://localhost:1455/auth/callback?code=ABC123&state=xyz")
                .as_deref(),
            Some("ABC123")
        );
        // code may be url-encoded and carry a #state suffix.
        assert_eq!(
            code_from_callback("http://localhost:1455/auth/callback?code=a%2Bb%3Dc#st").as_deref(),
            Some("a+b=c")
        );
        // bare code (no URL).
        assert_eq!(code_from_callback("  rawcode#state ").as_deref(), Some("rawcode"));
        assert_eq!(code_from_callback(""), None);
        // query present but no code param.
        assert_eq!(code_from_callback("http://localhost:1455/auth/callback?state=x"), None);
    }

    #[test]
    fn parses_chatgpt_account_id_from_id_token() {
        // header.payload.signature — only the payload (claims) is read.
        let payload = serde_json::json!({
            "sub": "user-1",
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct-abc-123" }
        });
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        let jwt = format!("header.{payload_b64}.sig");
        assert_eq!(chatgpt_account_id_from_id_token(&jwt).as_deref(), Some("acct-abc-123"));
        // missing claim → None.
        let empty = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{}");
        assert_eq!(chatgpt_account_id_from_id_token(&format!("h.{empty}.s")), None);
        assert_eq!(chatgpt_account_id_from_id_token("not-a-jwt"), None);
    }

    #[test]
    fn encodes_query_components() {
        assert_eq!(
            urlencoding("org:create_api_key user:profile"),
            "org%3Acreate_api_key%20user%3Aprofile"
        );
        assert_eq!(
            urlencoding("https://console.anthropic.com/oauth/code/callback"),
            "https%3A%2F%2Fconsole.anthropic.com%2Foauth%2Fcode%2Fcallback"
        );
    }
}
