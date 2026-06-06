//! `/gateway/anthropic/*` (and `/gateway/openai/*`) — the OAuth passthrough
//! gateway (CCT-232).
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
use axum::response::Response;
use chrono::Utc;
use uuid::Uuid;

use crate::state::AppState;

/// Refresh proactively once the access token is within this window of expiry.
const REFRESH_SKEW_SECS: i64 = 60;

/// Anthropic Claude-Code OAuth token endpoint + client id. These are not stable
/// public APIs (caveat accepted in the ticket); overridable via env so we can
/// track upstream changes without a redeploy of code.
fn anthropic_token_url() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_TOKEN_URL")
        .unwrap_or_else(|_| "https://console.anthropic.com/v1/oauth/token".into())
}
fn anthropic_client_id() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_CLIENT_ID")
        .unwrap_or_else(|_| "9d1c250a-e61b-44d9-88ed-5944d1962f5e".into())
}
fn anthropic_upstream() -> String {
    std::env::var("CCTUI_ANTHROPIC_UPSTREAM").unwrap_or_else(|_| "https://api.anthropic.com".into())
}
fn openai_token_url() -> String {
    std::env::var("CCTUI_OPENAI_OAUTH_TOKEN_URL")
        .unwrap_or_else(|_| "https://auth.openai.com/oauth/token".into())
}
fn openai_client_id() -> String {
    std::env::var("CCTUI_OPENAI_OAUTH_CLIENT_ID").unwrap_or_default()
}
fn openai_upstream() -> String {
    std::env::var("CCTUI_OPENAI_UPSTREAM").unwrap_or_else(|_| "https://api.openai.com".into())
}

/// Resolve a named account for a user and mint a session-scoped gateway token
/// bound to `(session_id, account)`, returning the env vars to inject into the
/// worker so its agent traffic flows through this gateway under that account
/// (CCT-232). The raw OAuth tokens never leave the server — only the opaque
/// session token does. Returns:
///   * `Ok(Some(env))` — account found, token minted, env ready
///   * `Ok(None)` — the caller has no account by that `(name, provider)`
///   * `Err(_)` — a database failure
pub async fn mint_session_env(
    state: &AppState,
    user_id: Uuid,
    account_name: &str,
    adapter_id: &str,
    session_id: &str,
) -> Result<Option<std::collections::BTreeMap<String, String>>, sqlx::Error> {
    let provider = match adapter_id {
        a if a.starts_with("codex") => "openai",
        _ => "anthropic",
    };
    let account_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM oauth_accounts WHERE user_id = $1 AND name = $2 AND provider = $3",
    )
    .bind(user_id)
    .bind(account_name)
    .bind(provider)
    .fetch_optional(&state.pool)
    .await?;
    let Some(account_id) = account_id else { return Ok(None) };

    // Mint a fresh opaque session token (same shape/entropy as other secrets)
    // and store only its hash, mapped to the session + account.
    let token = format!("cctui_s_{}", crate::auth::mint_secret());
    let token_hash = crate::auth::sha256_hex(&token);
    sqlx::query(
        "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
    )
    .bind(&token_hash)
    .bind(session_id)
    .bind(account_id)
    .execute(&state.pool)
    .await?;

    let base = state.config.external_url.trim_end_matches('/');
    let mut env = std::collections::BTreeMap::new();
    if provider == "anthropic" {
        env.insert("ANTHROPIC_BASE_URL".into(), format!("{base}/gateway/anthropic"));
        env.insert("ANTHROPIC_AUTH_TOKEN".into(), token);
    } else {
        env.insert("OPENAI_BASE_URL".into(), format!("{base}/gateway/openai"));
        env.insert("OPENAI_API_KEY".into(), token);
    }
    Ok(Some(env))
}

/// Revoke every session token bound to a session (CCT-232) — called when a
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

/// Raw account row as selected from the join (before decrypt).
type AccountRow = (Uuid, String, Option<String>, String, Option<chrono::DateTime<Utc>>);
/// Raw account row by id (no id column, before decrypt).
type ReloadRow = (String, Option<String>, String, Option<chrono::DateTime<Utc>>);

/// Loaded account row (decrypted in-process; never serialized out).
struct Account {
    id: Uuid,
    provider: String,
    access_token: Option<String>,
    refresh_token: String,
    expires_at: Option<chrono::DateTime<Utc>>,
}

/// Resolve the session token (the upstream bearer the worker sent) to its
/// account. Returns `None` for unknown/revoked tokens.
async fn resolve_account(state: &AppState, session_token: &str) -> Option<Account> {
    let hash = crate::auth::sha256_hex(session_token);
    let row: Option<AccountRow> = sqlx::query_as(
        "SELECT a.id, a.provider, a.encrypted_access_token, a.encrypted_refresh_token, \
                    a.expires_at \
             FROM session_tokens t JOIN oauth_accounts a ON a.id = t.account_id \
             WHERE t.token_hash = $1 AND t.revoked_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    let (id, provider, enc_access, enc_refresh, expires_at) = row?;
    let key = crate::crypto::vault_key();
    let access_token = enc_access.and_then(|e| crate::crypto::deobfuscate(&e, &key));
    let refresh_token = crate::crypto::deobfuscate(&enc_refresh, &key)?;
    Some(Account { id, provider, access_token, refresh_token, expires_at })
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
async fn refresh_account(state: &AppState, acct: &Account) -> Result<String, StatusCode> {
    let (token_url, client_id) = match acct.provider.as_str() {
        "anthropic" => (anthropic_token_url(), anthropic_client_id()),
        "openai" => (openai_token_url(), openai_client_id()),
        _ => return Err(StatusCode::BAD_GATEWAY),
    };

    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": acct.refresh_token,
        "client_id": client_id,
    });
    let resp = state.http_client.post(&token_url).json(&body).send().await.map_err(|e| {
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
    let enc_access = crate::crypto::obfuscate(&tok.access_token, &key);
    // Refresh tokens are single-use → persist the rotated one when returned.
    let enc_refresh = tok.refresh_token.as_deref().map(|r| crate::crypto::obfuscate(r, &key));
    let expires_at = tok.expires_in.map(|s| Utc::now() + chrono::Duration::seconds(s));

    let result = if let Some(enc_refresh) = enc_refresh {
        sqlx::query(
            "UPDATE oauth_accounts SET encrypted_access_token = $2, \
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
            "UPDATE oauth_accounts SET encrypted_access_token = $2, expires_at = $3 WHERE id = $1",
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
    let fresh = matches!(&acct.access_token, Some(t) if !t.is_empty())
        && acct
            .expires_at
            .is_none_or(|exp| exp > Utc::now() + chrono::Duration::seconds(REFRESH_SKEW_SECS));
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
                .is_none_or(|exp| exp > Utc::now() + chrono::Duration::seconds(REFRESH_SKEW_SECS));
        if let (true, Some(t)) = (still_fresh, reloaded.access_token.clone()) {
            return Ok(t);
        }
        return refresh_account(state, &reloaded).await;
    }
    refresh_account(state, acct).await
}

async fn reload_account(state: &AppState, id: Uuid) -> Option<Account> {
    let row: Option<ReloadRow> = sqlx::query_as(
        "SELECT provider, encrypted_access_token, encrypted_refresh_token, expires_at \
             FROM oauth_accounts WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    let (provider, enc_access, enc_refresh, expires_at) = row?;
    let key = crate::crypto::vault_key();
    Some(Account {
        id,
        provider,
        access_token: enc_access.and_then(|e| crate::crypto::deobfuscate(&e, &key)),
        refresh_token: crate::crypto::deobfuscate(&enc_refresh, &key)?,
        expires_at,
    })
}

/// `/gateway/anthropic/*path` — passthrough to api.anthropic.com.
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

async fn passthrough(
    state: AppState,
    req: Request,
    prefix: &str,
    upstream_base: &str,
) -> Result<Response, StatusCode> {
    // The worker's bearer is the session token; map it to an account.
    let session_token = req
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_string();

    let acct = resolve_account(&state, &session_token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    let access_token = current_access_token(&state, &acct).await?;

    // Build the upstream URL: strip the gateway prefix, keep path + query.
    let path = req.uri().path();
    let tail = path.strip_prefix(prefix).unwrap_or(path);
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let url = format!("{}{tail}{query}", upstream_base.trim_end_matches('/'));

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

    // Stream the request body through without buffering.
    let body_stream = req.into_body().into_data_stream();
    let upstream_body = reqwest::Body::wrap_stream(body_stream);

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
            "UPDATE oauth_accounts SET request_count = request_count + 1, \
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
    let resp_stream = upstream.bytes_stream();
    builder.body(Body::from_stream(resp_stream)).map_err(|e| {
        tracing::error!("gateway response build error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}
