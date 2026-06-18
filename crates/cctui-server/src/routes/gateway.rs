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
pub fn anthropic_token_url() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_TOKEN_URL")
        .unwrap_or_else(|_| "https://console.anthropic.com/v1/oauth/token".into())
}
pub fn anthropic_client_id() -> String {
    std::env::var("CCTUI_ANTHROPIC_OAUTH_CLIENT_ID")
        .unwrap_or_else(|_| "9d1c250a-e61b-44d9-88ed-5944d1962f5e".into())
}
/// claude.ai authorize endpoint for the manual code-paste OAuth login (CCT-243).
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
/// (`app_EMoamEEZ73f0CkXaXp7hrann`); overridable via env (CCT-244).
pub fn openai_client_id() -> String {
    std::env::var("CCTUI_OPENAI_OAUTH_CLIENT_ID")
        .unwrap_or_else(|_| "app_EMoamEEZ73f0CkXaXp7hrann".into())
}
/// auth.openai.com authorize endpoint for the "Sign in with ChatGPT" login
/// (CCT-244). Overridable so we can track upstream without a redeploy.
pub fn openai_authorize_url() -> String {
    std::env::var("CCTUI_OPENAI_OAUTH_AUTHORIZE_URL")
        .unwrap_or_else(|_| "https://auth.openai.com/oauth/authorize".into())
}
/// Fixed redirect URI baked into Codex's public client — we can't point it at
/// our own host. The browser redirect to localhost:1455 fails to load; the
/// user copies the full URL from the address bar and pastes it back (CCT-244).
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

/// The provider *family* of an account: which harness/env vars it drives.
/// Both native subscription accounts (`anthropic`/`openai`) and compatible
/// endpoints (`anthropic-compatible`/`openai-compatible`) collapse to one of
/// these two families (CCT-399).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Anthropic,
    Openai,
}

impl Family {
    /// Derive the family from a stored `provider` value. Anything containing
    /// `openai` is the OpenAI/Codex family; everything else is Anthropic.
    pub fn from_provider(provider: &str) -> Self {
        if provider.contains("openai") { Self::Openai } else { Self::Anthropic }
    }
    /// Derive the family from a spawn adapter id (`codex*` → openai, else
    /// anthropic). Used only as a fallback disambiguator when the caller names
    /// an account but not its provider.
    pub fn from_adapter(adapter_id: &str) -> Self {
        if adapter_id.starts_with("codex") { Self::Openai } else { Self::Anthropic }
    }
}

/// Resolve a named account for a user and mint a session-scoped gateway token
/// bound to `(session_id, account)`, returning the env vars to inject into the
/// worker so its agent traffic flows through this gateway under that account
/// (CCT-232 / CCT-399). The raw credentials never leave the server — only the
/// opaque session token does. The account drives the base URL + family, NOT the
/// adapter id (CCT-399): an explicit `provider` disambiguates name collisions
/// across providers; absent it, the adapter id is the fallback family hint.
/// Returns:
///   * `Ok(Some(env))` — account found, token minted, env ready
///   * `Ok(None)` — the caller has no matching account
///   * `Err(_)` — a database failure
pub async fn mint_session_env(
    state: &AppState,
    user_id: Uuid,
    account_name: &str,
    provider: Option<&str>,
    adapter_id: &str,
    session_id: &str,
) -> Result<Option<std::collections::BTreeMap<String, String>>, sqlx::Error> {
    // Resolve the account by (user, name), optionally constrained to an explicit
    // provider. With no provider hint we still disambiguate by family (derived
    // from the adapter) so a `personal` anthropic and a `personal` openai don't
    // collide on the machine-spawn path.
    let row: Option<(Uuid, String)> = if let Some(p) = provider {
        sqlx::query_as(
            "SELECT id, provider FROM oauth_accounts \
             WHERE user_id = $1 AND name = $2 AND provider = $3",
        )
        .bind(user_id)
        .bind(account_name)
        .bind(p)
        .fetch_optional(&state.pool)
        .await?
    } else {
        let want = Family::from_adapter(adapter_id);
        let candidates: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, provider FROM oauth_accounts WHERE user_id = $1 AND name = $2",
        )
        .bind(user_id)
        .bind(account_name)
        .fetch_all(&state.pool)
        .await?;
        candidates.into_iter().find(|(_, prov)| Family::from_provider(prov) == want)
    };
    let Some((account_id, prov)) = row else { return Ok(None) };
    let family = Family::from_provider(&prov);

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
    match family {
        Family::Anthropic => {
            env.insert("ANTHROPIC_BASE_URL".into(), format!("{base}/gateway/anthropic"));
            env.insert("ANTHROPIC_AUTH_TOKEN".into(), token);
        }
        Family::Openai => {
            env.insert("OPENAI_BASE_URL".into(), format!("{base}/gateway/openai"));
            env.insert("OPENAI_API_KEY".into(), token);
        }
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
type AccountRow = (
    Uuid,
    String,
    Option<String>,
    Option<String>,
    Option<chrono::DateTime<Utc>>,
    Option<String>,
    Option<String>,
    String,
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
    /// as the `Chatgpt-Account-Id` header (CCT-244). NULL for anthropic / manual
    /// refresh-token accounts.
    provider_account_id: Option<String>,
    /// Compatible-endpoint upstream base URL (CCT-399). NULL → built-in upstream.
    base_url: Option<String>,
    /// `oauth` (refreshing subscription) | `bearer` | `api_key` (static, CCT-399).
    auth_scheme: String,
}

impl Account {
    /// A static-credential compatible account forwards its stored credential
    /// verbatim and skips the OAuth refresh round-trip (CCT-399).
    fn is_static(&self) -> bool {
        self.auth_scheme != "oauth"
    }
}

/// Resolve the session token (the upstream bearer the worker sent) to its
/// account. Returns `None` for unknown/revoked tokens.
async fn resolve_account(state: &AppState, session_token: &str) -> Option<Account> {
    let hash = crate::auth::sha256_hex(session_token);
    let row: Option<AccountRow> = sqlx::query_as(
        "SELECT a.id, a.provider, a.encrypted_access_token, a.encrypted_refresh_token, \
                    a.expires_at, a.provider_account_id, a.base_url, a.auth_scheme \
             FROM session_tokens t JOIN oauth_accounts a ON a.id = t.account_id \
             WHERE t.token_hash = $1 AND t.revoked_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    let (id, provider, enc_access, enc_refresh, expires_at, provider_account_id, base_url, auth_scheme) =
        row?;
    let key = crate::crypto::vault_key();
    let access_token = enc_access.and_then(|e| crate::crypto::deobfuscate(&e, &key));
    let refresh_token = enc_refresh.and_then(|e| crate::crypto::deobfuscate(&e, &key));
    Some(Account {
        id,
        provider,
        access_token,
        refresh_token,
        expires_at,
        provider_account_id,
        base_url,
        auth_scheme,
    })
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
    // Static-credential compatible accounts never refresh — the stored access
    // token (the bearer/api key) is forwarded verbatim (CCT-399).
    if acct.is_static() {
        return acct.access_token.clone().ok_or(StatusCode::BAD_GATEWAY);
    }
    let Some(refresh_token) = acct.refresh_token.as_deref() else {
        return Err(StatusCode::BAD_GATEWAY);
    };
    // Anthropic refreshes with a JSON body; OpenAI/Codex with a form-encoded
    // body (matches the codex CLI + CLIProxyAPI — CCT-244).
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
    // Static-credential compatible accounts: forward the stored credential
    // verbatim, no expiry tracking, no refresh (CCT-399).
    if acct.is_static() {
        return acct
            .access_token
            .clone()
            .filter(|t| !t.is_empty())
            .ok_or(StatusCode::UNAUTHORIZED);
    }
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
        "SELECT provider, encrypted_access_token, encrypted_refresh_token, expires_at, \
                    provider_account_id, base_url, auth_scheme \
             FROM oauth_accounts WHERE id = $1",
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
        access_token: enc_access.and_then(|e| crate::crypto::deobfuscate(&e, &key)),
        refresh_token: enc_refresh.and_then(|e| crate::crypto::deobfuscate(&e, &key)),
        expires_at,
        provider_account_id,
        base_url,
        auth_scheme,
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

    // Per-account upstream (CCT-399): a compatible endpoint overrides the
    // built-in upstream with its stored `base_url`; native subscription accounts
    // fall back to the built-in `api.anthropic.com`/`chatgpt.com`.
    let upstream = acct.base_url.as_deref().filter(|u| !u.trim().is_empty()).unwrap_or(upstream_base);

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
    // ChatGPT-backed Codex requests must carry the account id upstream (CCT-244).
    if let Some(account_id) = acct.provider_account_id.as_deref()
        && let Ok(hv) = reqwest::header::HeaderValue::from_str(account_id)
    {
        headers.insert("chatgpt-account-id", hv);
    }

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

/// Anthropic's free OAuth usage endpoint (CCT-306). Returns subscription window
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

/// Fetch the Anthropic OAuth usage windows for an account (CCT-306).
///
/// Reloads + decrypts the account, ensures a fresh access token (refreshing under
/// the per-account mutex if needed), and calls Anthropic's free usage endpoint.
/// Returns:
///   * `Ok(Some(json))` — anthropic account, usage fetched
///   * `Ok(None)` — no such account, or a non-anthropic provider (no usage API)
///   * `Err(status)` — token refresh failed or upstream rejected (e.g. 429)
///
/// This makes NO inference request and costs no tokens. Callers MUST throttle it
/// (the endpoint rate-limits per access token); see the usage cache in the route.
pub async fn fetch_account_usage(
    state: &AppState,
    account_id: Uuid,
) -> Result<Option<serde_json::Value>, StatusCode> {
    let Some(acct) = reload_account(state, account_id).await else { return Ok(None) };
    if acct.provider != "anthropic" {
        // Codex/OpenAI has no equivalent free usage endpoint — degrade gracefully.
        return Ok(None);
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

#[cfg(test)]
mod tests {
    use super::Family;

    #[test]
    fn family_from_provider_maps_native_and_compatible() {
        // CCT-399: both native and `-compatible` providers collapse to a family.
        assert!(matches!(Family::from_provider("anthropic"), Family::Anthropic));
        assert!(matches!(Family::from_provider("anthropic-compatible"), Family::Anthropic));
        assert!(matches!(Family::from_provider("openai"), Family::Openai));
        assert!(matches!(Family::from_provider("openai-compatible"), Family::Openai));
    }

    #[test]
    fn family_from_adapter_is_the_fallback_hint() {
        assert!(matches!(Family::from_adapter("codex"), Family::Openai));
        assert!(matches!(Family::from_adapter("codex-foo"), Family::Openai));
        assert!(matches!(Family::from_adapter("claude-code"), Family::Anthropic));
    }
}
