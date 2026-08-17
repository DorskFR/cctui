use super::{
    REFRESH_SKEW_SECS, anthropic_client_id, anthropic_token_url, openai_client_id, openai_token_url,
};
use std::sync::Arc;

use axum::http::StatusCode;
use chrono::Utc;
use uuid::Uuid;

use crate::state::AppState;

/// Raw account row as selected from the join (before decrypt).
pub type AccountRow = (
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
    Option<serde_json::Value>,
);
/// Raw account row by id (no id column, before decrypt).
pub type ReloadRow = (
    String,
    Option<String>,
    Option<String>,
    Option<chrono::DateTime<Utc>>,
    Option<String>,
    Option<String>,
    String,
);

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
    pub soft_limits: crate::soft_limit::SoftLimits,
    /// Per-provider gateway request-shaping settings; see [`FireworksSettings`].
    pub provider_settings: Option<serde_json::Value>,
    /// Per-(account, provider) RPM/TPM ceilings enforced in the proxy path.
    /// Unset ⇒ no throttling.
    pub rate_limits: crate::routes::gateway::RateLimits,
}

impl Account {
    /// A static-credential compatible account forwards its stored credential
    /// verbatim and skips the OAuth refresh round-trip.
    fn is_static(&self) -> bool {
        self.auth_scheme != "oauth"
    }
}

/// Resolve a session token to its bound account.
///
/// Three-valued on purpose: `Ok(Some)` = bound and live;
/// `Ok(None)` = the token is genuinely unknown/revoked/unbound (a real orphan);
/// `Err` = the DB lookup itself failed (cold/starved pool on a server restart,
/// transient network). The caller MUST NOT treat `Err` as an orphan: feeding the
/// spam guard on a transient error would block valid tokens for 300s. On `Err`
/// we return a retryable 503 and never touch the orphan block.
pub async fn resolve_account(
    state: &AppState,
    session_token: &str,
) -> Result<Option<Account>, sqlx::Error> {
    let hash = crate::auth::sha256_hex(session_token);
    let row: Option<AccountRow> = sqlx::query_as(
        "SELECT a.id, a.provider, a.encrypted_access_token, a.encrypted_refresh_token, \
                    a.expires_at, a.provider_account_id, a.base_url, a.auth_scheme, \
                    a.soft_limits_json, a.provider_settings, a.rate_limits_json \
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
        rate_limits_json,
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
        rate_limits: crate::routes::gateway::RateLimits::from_json(rate_limits_json.as_ref()),
    }))
}

#[derive(serde::Deserialize)]
pub struct TokenResponse {
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
pub async fn refresh_account(state: &AppState, acct: &Account) -> Result<String, StatusCode> {
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

/// A NULL expiry counts as stale (not fresh-forever) so an unbounded OAuth
/// token is refreshed rather than forwarded until it dies upstream and 401s the
/// whole account.
pub fn access_token_is_fresh(
    access_token: Option<&str>,
    expires_at: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
) -> bool {
    access_token.is_some_and(|t| !t.is_empty())
        && expires_at.is_some_and(|exp| exp > now + chrono::Duration::seconds(REFRESH_SKEW_SECS))
}

/// Return a valid access token for the account, refreshing if absent or within
/// the skew window. Serialized per account so concurrent sessions don't
/// double-refresh a single-use refresh token.
pub async fn current_access_token(state: &AppState, acct: &Account) -> Result<String, StatusCode> {
    // Static-credential compatible accounts: forward the stored credential
    // verbatim, no expiry tracking, no refresh.
    if acct.is_static() {
        return acct.access_token.clone().filter(|t| !t.is_empty()).ok_or(StatusCode::UNAUTHORIZED);
    }
    let fresh = access_token_is_fresh(acct.access_token.as_deref(), acct.expires_at, Utc::now());
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
        let still_fresh = access_token_is_fresh(
            reloaded.access_token.as_deref(),
            reloaded.expires_at,
            Utc::now(),
        );
        if let (true, Some(t)) = (still_fresh, reloaded.access_token.clone()) {
            return Ok(t);
        }
        return refresh_account(state, &reloaded).await;
    }
    refresh_account(state, acct).await
}

pub async fn reload_account(state: &AppState, id: Uuid) -> Option<Account> {
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
        rate_limits: crate::routes::gateway::RateLimits::default(),
    })
}
