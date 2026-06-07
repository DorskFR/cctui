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

use crate::auth::{AuthContext, require_user};
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

/// API view of an account — secrets (tokens) deliberately absent.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct AccountInfo {
    pub id: Uuid,
    pub name: String,
    pub provider: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub request_count: i64,
    pub bytes_transferred: i64,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateAccount {
    pub name: String,
    /// `anthropic` | `openai`.
    pub provider: String,
    /// OAuth refresh token (pasted by the user). Stored encrypted; the gateway
    /// exchanges it for access tokens on demand.
    pub refresh_token: String,
    /// Optional initial access token (skips the first refresh round-trip).
    #[serde(default)]
    pub access_token: Option<String>,
    /// Optional access-token expiry (unix seconds). When absent the gateway
    /// refreshes on first use.
    #[serde(default)]
    pub expires_at: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
pub struct RenameAccount {
    pub name: String,
}

fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(serde_json::json!({ "error": msg })))
}

/// `GET /api/v1/accounts` — the caller's own accounts (tokens never returned).
pub async fn list_accounts(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<AccountInfo>>, (StatusCode, Json<serde_json::Value>)> {
    let uid = require_user(&ctx).map_err(|c| err(c, "user token required"))?;
    let rows: Vec<AccountInfo> = sqlx::query_as(
        "SELECT id, name, provider, expires_at, created_at, last_used_at, \
                request_count, bytes_transferred \
         FROM oauth_accounts WHERE user_id = $1 ORDER BY provider, name",
    )
    .bind(uid)
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
    let uid = require_user(&ctx).map_err(|c| err(c, "user token required"))?;
    if req.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }
    if !matches!(req.provider.as_str(), "anthropic" | "openai") {
        return Err(err(StatusCode::BAD_REQUEST, "provider must be anthropic|openai"));
    }
    if req.refresh_token.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "refresh_token required"));
    }

    let key = crate::crypto::vault_key();
    let enc_refresh = crate::crypto::obfuscate(&req.refresh_token, &key);
    let enc_access = req.access_token.as_deref().map(|t| crate::crypto::obfuscate(t, &key));
    let expires_at = req.expires_at.and_then(|s| DateTime::<Utc>::from_timestamp(s, 0));

    let row: Result<AccountInfo, sqlx::Error> = sqlx::query_as(
        "INSERT INTO oauth_accounts \
            (user_id, name, provider, encrypted_refresh_token, encrypted_access_token, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, name, provider, expires_at, created_at, last_used_at, \
                   request_count, bytes_transferred",
    )
    .bind(uid)
    .bind(req.name.trim())
    .bind(&req.provider)
    .bind(&enc_refresh)
    .bind(&enc_access)
    .bind(expires_at)
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

/// `PATCH /api/v1/accounts/{id}` — rename.
pub async fn rename_account(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<RenameAccount>,
) -> Result<Json<AccountInfo>, (StatusCode, Json<serde_json::Value>)> {
    let uid = require_user(&ctx).map_err(|c| err(c, "user token required"))?;
    if req.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }
    let row: Option<AccountInfo> = sqlx::query_as(
        "UPDATE oauth_accounts SET name = $3 WHERE id = $1 AND user_id = $2 \
         RETURNING id, name, provider, expires_at, created_at, last_used_at, \
                   request_count, bytes_transferred",
    )
    .bind(id)
    .bind(uid)
    .bind(req.name.trim())
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
    let uid = require_user(&ctx).map_err(|c| err(c, "user token required"))?;
    let res = sqlx::query("DELETE FROM oauth_accounts WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(uid)
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
// "Sign in with Claude" OAuth authorize flow (CCT-243)
//
// OAuth 2.1 authorization-code + PKCE in manual code-paste mode (same as
// `claude /login` / better-ccflare): we generate a PKCE verifier/challenge and
// a nonce, the user authorizes at claude.ai which displays a `code#state` pair,
// and they paste it back. We exchange it for tokens and store the account
// exactly like POST /accounts. Pending logins live in memory only, keyed by
// nonce + scoped to the authenticated user, single-use, TTL-bounded.
// ----------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
pub struct OAuthStart {
    /// Only `anthropic` is supported today (Codex stays on the manual path —
    /// CCT-244).
    pub provider: String,
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
    /// The `code#state` pair pasted from claude.ai (the `#state` suffix is
    /// optional — we fall back to our stored verifier as the state).
    pub code: String,
}

#[derive(serde::Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    expires_in: Option<i64>,
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
    let uid = require_user(&ctx).map_err(|c| err(c, "user token required"))?;
    if req.provider != "anthropic" {
        return Err(err(StatusCode::BAD_REQUEST, "provider must be anthropic"));
    }

    sweep_expired(&state.pending_oauth_logins);

    // PKCE verifier (high-entropy, URL-safe) doubles as the OAuth `state`, same
    // as better-ccflare. nonce keys the pending record (distinct from state so
    // the client never has to handle the verifier).
    let code_verifier = crate::auth::mint_secret();
    let nonce = crate::auth::mint_secret();
    let challenge = pkce_challenge(&code_verifier);

    let authorize_url = format!(
        "{}?code=true&client_id={}&response_type=code&redirect_uri={}\
         &scope=org:create_api_key%20user:profile%20user:inference\
         &code_challenge={}&code_challenge_method=S256&state={}",
        gateway::anthropic_authorize_url(),
        urlencoding(&gateway::anthropic_client_id()),
        urlencoding(&gateway::anthropic_oauth_redirect_uri()),
        urlencoding(&challenge),
        urlencoding(&code_verifier),
    );

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
    let uid = require_user(&ctx).map_err(|c| err(c, "user token required"))?;
    if req.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }

    sweep_expired(&state.pending_oauth_logins);

    // Consume the pending record (single-use), but only if it belongs to the
    // caller — never let one user finish another user's login.
    let pending = match state.pending_oauth_logins.get(&req.nonce) {
        Some(p) if p.user_id == uid => p.clone(),
        _ => return Err(err(StatusCode::BAD_REQUEST, "unknown or expired login")),
    };
    state.pending_oauth_logins.remove(&req.nonce);

    let (code, state_part) = split_code_state(&req.code);
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

    let resp =
        state.http_client.post(gateway::anthropic_token_url()).json(&body).send().await.map_err(
            |e| {
                tracing::error!("oauth token exchange transport error: {e}");
                err(StatusCode::BAD_GATEWAY, "token exchange failed")
            },
        )?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        tracing::error!(%status, "oauth token exchange rejected: {detail}");
        return Err(err(
            StatusCode::BAD_REQUEST,
            "token exchange rejected — check the pasted code",
        ));
    }
    let tok: OAuthTokenResponse = resp.json().await.map_err(|e| {
        tracing::error!("oauth token exchange decode error: {e}");
        err(StatusCode::BAD_GATEWAY, "token exchange decode failed")
    })?;

    let key = crate::crypto::vault_key();
    let enc_refresh = crate::crypto::obfuscate(&tok.refresh_token, &key);
    let enc_access = crate::crypto::obfuscate(&tok.access_token, &key);
    let expires_at = tok.expires_in.map(|s| Utc::now() + Duration::seconds(s));

    let row: Result<AccountInfo, sqlx::Error> = sqlx::query_as(
        "INSERT INTO oauth_accounts \
            (user_id, name, provider, encrypted_refresh_token, encrypted_access_token, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, name, provider, expires_at, created_at, last_used_at, \
                   request_count, bytes_transferred",
    )
    .bind(uid)
    .bind(req.name.trim())
    .bind(&pending.provider)
    .bind(&enc_refresh)
    .bind(&enc_access)
    .bind(expires_at)
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
