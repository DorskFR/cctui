use super::{
    REFRESH_SKEW_SECS, anthropic_client_id, anthropic_token_url, openai_client_id, openai_token_url,
};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use chrono::Utc;
use dashmap::DashMap;
use uuid::Uuid;

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

    fn anthropic_token_url(&self) -> String {
        anthropic_token_url()
    }
    fn openai_token_url(&self) -> String {
        openai_token_url()
    }
}

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
    pub soft_limits: cctui_proto::api::soft_limit::SoftLimits,
    /// Per-provider usage ticker setting; off ⇒ the proxy skips it.
    pub usage_notices: cctui_proto::api::gateway::UsageNotices,
    /// Per-provider gateway request-shaping settings; see [`super::FireworksSettings`].
    pub provider_settings: Option<serde_json::Value>,
    /// Per-(account, provider) RPM/TPM ceilings enforced in the proxy path.
    /// Unset ⇒ no throttling.
    pub rate_limits: cctui_proto::api::gateway::RateLimits,
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
    state: &impl CredentialContext,
    session_token: &str,
) -> Result<Option<Account>, sqlx::Error> {
    let hash = crate::auth::sha256_hex(session_token);
    let row = account_row(state.pool(), &hash).await?;
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
        usage_notices,
    )) = row
    else {
        return Ok(None);
    };
    let key = state.vault_key();
    let access_token = enc_access.and_then(|e| cctui_crypto::decrypt(&e, &key));
    let refresh_token = enc_refresh.and_then(|e| cctui_crypto::decrypt(&e, &key));
    Ok(Some(Account {
        id,
        provider,
        access_token,
        refresh_token,
        expires_at,
        provider_account_id,
        base_url,
        auth_scheme,
        soft_limits: cctui_proto::api::soft_limit::SoftLimits::from_json(soft_limits_json.as_ref()),
        usage_notices: cctui_proto::api::gateway::UsageNotices::from_json(usage_notices.as_ref()),
        provider_settings,
        rate_limits: cctui_proto::api::gateway::RateLimits::from_json(rate_limits_json.as_ref()),
    }))
}

/// A live session token's account, unless the session's owner or the user the
/// token was minted for is revoked or disabled, or that user may no longer use
/// the account — a share revoked since the mint.
async fn account_row(pool: &sqlx::PgPool, hash: &str) -> Result<Option<AccountRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.provider, a.encrypted_access_token, a.encrypted_refresh_token, \
                    a.expires_at, a.provider_account_id, a.base_url, a.auth_scheme, \
                    a.soft_limits_json, a.provider_settings, a.rate_limits_json, \
                    a.usage_notices \
             FROM session_tokens t JOIN account_providers a ON a.id = t.account_id \
             WHERE t.token_hash = $1 AND t.revoked_at IS NULL \
               AND (t.expires_at IS NULL OR t.expires_at > now()) \
               AND NOT EXISTS ( \
                 SELECT 1 FROM sessions s \
                 LEFT JOIN machines m ON m.id = s.machine_uuid \
                 JOIN users u ON u.id = COALESCE(s.user_id, m.user_id) \
                 WHERE s.id = t.session_id \
                   AND (u.revoked_at IS NOT NULL OR u.disabled_at IS NOT NULL)) \
               AND NOT EXISTS ( \
                 SELECT 1 FROM users tu WHERE tu.id = t.user_id \
                   AND (tu.revoked_at IS NOT NULL OR tu.disabled_at IS NOT NULL)) \
               AND (t.user_id IS NULL OR EXISTS ( \
                 SELECT 1 FROM accounts acc \
                  WHERE acc.id = a.account_id \
                    AND (acc.user_id = t.user_id OR EXISTS ( \
                        SELECT 1 FROM resource_shares rs \
                         WHERE rs.resource_type = 'account' AND rs.resource_id = acc.id \
                           AND rs.grantee_id = t.user_id AND rs.revoked_at IS NULL))))",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await
}

#[derive(serde::Deserialize)]
pub struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    /// Anthropic's token response names the organization the credential
    /// belongs to; the limit-reset endpoint is addressed by that uuid.
    #[serde(default)]
    organization: Option<OrganizationRef>,
}

#[derive(serde::Deserialize)]
pub struct OrganizationRef {
    #[serde(default)]
    pub uuid: Option<String>,
}

/// Exchange the refresh token for a fresh access token and persist the rotated
/// pair. Caller MUST hold the account's refresh mutex. Returns the new access
/// token.
// Linear refresh flow with per-provider branches; complexity is per-branch
// error handling, not nesting.
#[allow(clippy::cognitive_complexity)]
pub async fn refresh_account(
    state: &impl CredentialContext,
    acct: &Account,
) -> Result<String, StatusCode> {
    // Static-credential compatible accounts never refresh — the stored access
    // token (the bearer/api key) is forwarded verbatim.
    if acct.is_static() {
        return acct.access_token.clone().ok_or(StatusCode::BAD_GATEWAY);
    }
    let Some(refresh_token) = acct.refresh_token.as_deref() else {
        return Err(StatusCode::BAD_GATEWAY);
    };
    let fingerprint = crate::auth::sha256_hex(refresh_token);
    if REFRESH_REJECTIONS.recently_rejected(acct.id, &fingerprint, Instant::now()) {
        return Err(StatusCode::BAD_GATEWAY);
    }
    // Anthropic refreshes with a JSON body; OpenAI/Codex with a form-encoded
    // body (matches the codex CLI + CLIProxyAPI).
    let request = match acct.provider.as_str() {
        "anthropic" => {
            let body = serde_json::json!({
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": anthropic_client_id(),
            });
            state.http_client().post(state.anthropic_token_url()).json(&body)
        }
        "openai" => {
            let form = [
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", &openai_client_id()),
                ("scope", "openid profile email"),
            ];
            state.http_client().post(state.openai_token_url()).form(&form)
        }
        _ => return Err(StatusCode::BAD_GATEWAY),
    };
    let resp = request.send().await.map_err(|e| {
        tracing::error!(account = %acct.id, "oauth refresh transport error: {e}");
        StatusCode::BAD_GATEWAY
    })?;
    if !resp.status().is_success() {
        let status = resp.status();
        // Only the OAuth error code is read from the body, never echoed whole.
        let error = resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|b| b.get("error").and_then(|e| e.as_str()).map(str::to_owned));
        let error = error.as_deref().unwrap_or("unknown");
        tracing::error!(account = %acct.id, %status, error, "oauth refresh rejected");
        if refresh_rejection_is_terminal(status.as_u16()) {
            // The provider refused this refresh token: it will refuse it again,
            // so flag the account for a human reconnect and stop re-sending it
            // until the backoff lapses or a new token is stored.
            REFRESH_REJECTIONS.record(acct.id, fingerprint, Instant::now());
            super::health::flag_account_reauth(
                state,
                acct.id,
                &format!(
                    "OAuth refresh rejected (HTTP {}, {error}): reconnect the account",
                    status.as_u16()
                ),
            );
        }
        return Err(StatusCode::BAD_GATEWAY);
    }
    let tok: TokenResponse = resp.json().await.map_err(|e| {
        tracing::error!(account = %acct.id, "oauth refresh decode error: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    let key = state.vault_key();
    let enc_access = cctui_crypto::encrypt(&tok.access_token, &key);
    // Refresh tokens are single-use → persist the rotated one when returned.
    let enc_refresh = tok.refresh_token.as_deref().map(|r| cctui_crypto::encrypt(r, &key));
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
        .execute(state.pool())
        .await
    } else {
        sqlx::query(
            "UPDATE account_providers SET encrypted_access_token = $2, expires_at = $3 WHERE id = $1",
        )
        .bind(acct.id)
        .bind(&enc_access)
        .bind(expires_at)
        .execute(state.pool())
        .await
    };
    if let Err(e) = result {
        tracing::error!(account = %acct.id, "persist refreshed token failed: {e}");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    if let Some(org) = tok.organization.as_ref().and_then(|o| o.uuid.as_deref()) {
        remember_organization_uuid(state, acct.id, org).await;
    }
    tracing::info!(account = %acct.id, "refreshed oauth access token");
    Ok(tok.access_token)
}

/// How long a refresh token the provider refused is not sent again. An idle
/// account is still polled for usage every few seconds; without this every poll
/// replays the same dead token against the provider's token endpoint.
const REFRESH_REJECTION_BACKOFF: Duration = Duration::from_mins(10);

static REFRESH_REJECTIONS: LazyLock<RefreshRejections> = LazyLock::new(RefreshRejections::default);

/// A 400 (`invalid_grant`: revoked, rotated elsewhere or expired) or 401 from
/// the token endpoint will not heal on retry; a 429 or 5xx might.
#[must_use]
pub const fn refresh_rejection_is_terminal(status: u16) -> bool {
    matches!(status, 400 | 401)
}

/// Refresh tokens the provider refused, by account, keyed on the token's
/// fingerprint so a reconnect (a new token) is tried at once.
#[derive(Default)]
pub struct RefreshRejections(DashMap<Uuid, (String, Instant)>);

impl RefreshRejections {
    pub fn record(&self, account: Uuid, fingerprint: String, at: Instant) {
        self.0.insert(account, (fingerprint, at));
    }

    #[must_use]
    pub fn recently_rejected(&self, account: Uuid, fingerprint: &str, now: Instant) -> bool {
        self.0.get(&account).is_some_and(|r| {
            r.0 == fingerprint && now.saturating_duration_since(r.1) < REFRESH_REJECTION_BACKOFF
        })
    }
}

/// Persist the organization uuid a token or profile payload named. Best-effort.
pub async fn remember_organization_uuid(state: &impl CredentialContext, id: Uuid, org: &str) {
    if let Err(e) = sqlx::query(
        "UPDATE account_providers SET organization_uuid = $2 \
         WHERE id = $1 AND organization_uuid IS DISTINCT FROM $2",
    )
    .bind(id)
    .bind(org)
    .execute(state.pool())
    .await
    {
        tracing::warn!(account = %id, "persist organization uuid failed: {e}");
    }
}

/// A NULL expiry counts as stale (not fresh-forever) so an unbounded OAuth
/// token is refreshed rather than forwarded until it dies upstream and 401s the
/// whole account.
#[must_use]
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
pub async fn current_access_token(
    state: &impl CredentialContext,
    acct: &Account,
) -> Result<String, StatusCode> {
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
        .account_locks()
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

pub async fn reload_account(state: &impl CredentialContext, id: Uuid) -> Option<Account> {
    let row: Option<ReloadRow> = sqlx::query_as(
        "SELECT provider, encrypted_access_token, encrypted_refresh_token, expires_at, \
                    provider_account_id, base_url, auth_scheme \
             FROM account_providers WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(state.pool())
    .await
    .ok()
    .flatten();
    let (provider, enc_access, enc_refresh, expires_at, provider_account_id, base_url, auth_scheme) =
        row?;
    let key = state.vault_key();
    Some(Account {
        id,
        provider,
        access_token: enc_access.and_then(|e| cctui_crypto::decrypt(&e, &key)),
        refresh_token: enc_refresh.and_then(|e| cctui_crypto::decrypt(&e, &key)),
        expires_at,
        provider_account_id,
        base_url,
        auth_scheme,
        // `reload_account` only services the token-refresh / usage-fetch paths,
        // never the soft-limit gate (which runs off `resolve_account`), so the
        // caps are irrelevant here — default them, as are the gateway settings
        // (request shaping runs off `resolve_account` too).
        soft_limits: cctui_proto::api::soft_limit::SoftLimits::default(),
        usage_notices: cctui_proto::api::gateway::UsageNotices::default(),
        provider_settings: None,
        rate_limits: cctui_proto::api::gateway::RateLimits::default(),
    })
}

#[cfg(test)]
mod refresh_rejection_tests {
    use std::time::{Duration, Instant};

    use uuid::Uuid;

    use super::{REFRESH_REJECTION_BACKOFF, RefreshRejections, refresh_rejection_is_terminal};

    #[test]
    fn only_bad_request_and_unauthorized_are_terminal() {
        assert!(refresh_rejection_is_terminal(400));
        assert!(refresh_rejection_is_terminal(401));
        for transient in [403, 408, 429, 500, 502, 503] {
            assert!(!refresh_rejection_is_terminal(transient), "{transient}");
        }
    }

    #[test]
    fn a_rejected_token_is_held_back_until_the_backoff_lapses() {
        let rejections = RefreshRejections::default();
        let account = Uuid::new_v4();
        let at = Instant::now();
        rejections.record(account, "dead".into(), at);
        assert!(rejections.recently_rejected(account, "dead", at + Duration::from_secs(5)));
        assert!(!rejections.recently_rejected(account, "dead", at + REFRESH_REJECTION_BACKOFF));
    }

    #[test]
    fn a_new_token_or_another_account_is_tried_at_once() {
        let rejections = RefreshRejections::default();
        let account = Uuid::new_v4();
        let at = Instant::now();
        rejections.record(account, "dead".into(), at);
        assert!(!rejections.recently_rejected(account, "reconnected", at));
        assert!(!rejections.recently_rejected(Uuid::new_v4(), "dead", at));
    }
}

#[cfg(test)]
mod owner_revocation_tests {
    use uuid::Uuid;

    use super::account_row;

    #[tokio::test]
    async fn revoked_owner_session_token_is_not_resolved() {
        let Some(url) = crate::test_support::db_url("revoked_owner_session_token_is_not_resolved")
        else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let uid = Uuid::new_v4();
        let acct = Uuid::new_v4();
        let prov = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("revoke-gw-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, 'revoke-gw')")
            .bind(acct)
            .bind(uid)
            .execute(&pool)
            .await
            .expect("seed account");
        sqlx::query(
            "INSERT INTO account_providers \
                 (id, user_id, provider, encrypted_refresh_token, account_id) \
             VALUES ($1, $2, 'anthropic', 'x', $3)",
        )
        .bind(prov)
        .bind(uid)
        .bind(acct)
        .execute(&pool)
        .await
        .expect("seed provider");
        let session = format!("revoke-gw-{uid}");
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, working_dir, user_id, adapter_id) \
             VALUES ($1, 'm1', '/w', $2, 'opencode')",
        )
        .bind(&session)
        .bind(uid)
        .execute(&pool)
        .await
        .expect("seed session");
        let hash = format!("revoke-gw-tok-{uid}");
        sqlx::query(
            "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
        )
        .bind(&hash)
        .bind(&session)
        .bind(prov)
        .execute(&pool)
        .await
        .expect("seed token");

        assert!(account_row(&pool, &hash).await.unwrap().is_some(), "live owner resolves");

        sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
            .bind(uid)
            .execute(&pool)
            .await
            .unwrap();
        assert!(account_row(&pool, &hash).await.unwrap().is_none(), "disabled owner is refused");

        sqlx::query("UPDATE users SET disabled_at = NULL, revoked_at = now() WHERE id = $1")
            .bind(uid)
            .execute(&pool)
            .await
            .unwrap();
        assert!(account_row(&pool, &hash).await.unwrap().is_none(), "revoked owner is refused");

        sqlx::query("UPDATE users SET revoked_at = NULL WHERE id = $1")
            .bind(uid)
            .execute(&pool)
            .await
            .unwrap();
        crate::store::tokens::revoke_by_user(&pool, uid).await.unwrap();
        assert!(account_row(&pool, &hash).await.unwrap().is_none(), "revoked token is refused");

        sqlx::query("DELETE FROM session_tokens WHERE token_hash = $1")
            .bind(&hash)
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&session).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// A service token has no session row: the disabled-user check must follow
    /// the user the token was minted for.
    #[tokio::test]
    async fn disabled_user_service_token_is_not_resolved() {
        let Some(url) = crate::test_support::db_url("disabled_user_service_token_is_not_resolved")
        else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let uid = Uuid::new_v4();
        let acct = Uuid::new_v4();
        let prov = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("svc-gw-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, 'svc-gw')")
            .bind(acct)
            .bind(uid)
            .execute(&pool)
            .await
            .expect("seed account");
        sqlx::query(
            "INSERT INTO account_providers \
                 (id, user_id, provider, encrypted_refresh_token, account_id) \
             VALUES ($1, $2, 'anthropic', 'x', $3)",
        )
        .bind(prov)
        .bind(uid)
        .bind(acct)
        .execute(&pool)
        .await
        .expect("seed provider");
        let (token, row) =
            crate::store::tokens::mint_service(&pool, uid, prov, "svc").await.unwrap();
        let hash = crate::auth::sha256_hex(&token);

        assert!(account_row(&pool, &hash).await.unwrap().is_some(), "live service token resolves");

        sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
            .bind(uid)
            .execute(&pool)
            .await
            .unwrap();
        assert!(account_row(&pool, &hash).await.unwrap().is_none(), "disabled user is refused");

        sqlx::query("UPDATE users SET disabled_at = NULL WHERE id = $1")
            .bind(uid)
            .execute(&pool)
            .await
            .unwrap();
        assert!(account_row(&pool, &hash).await.unwrap().is_some(), "re-enabled user resolves");

        assert!(crate::store::tokens::revoke_service(&pool, acct, row.id).await.unwrap());
        assert!(account_row(&pool, &hash).await.unwrap().is_none(), "revoked token is refused");

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}

#[cfg(test)]
mod host_boundary_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Host {
        pool: sqlx::PgPool,
        client: reqwest::Client,
        locks: DashMap<Uuid, Arc<tokio::sync::Mutex<()>>>,
        reauth: DashMap<Uuid, ()>,
        key_reads: AtomicUsize,
        token_url: String,
    }

    impl CredentialContext for Host {
        fn pool(&self) -> &sqlx::PgPool {
            &self.pool
        }
        fn http_client(&self) -> &reqwest::Client {
            &self.client
        }
        fn account_locks(&self) -> &DashMap<Uuid, Arc<tokio::sync::Mutex<()>>> {
            &self.locks
        }
        fn account_reauth(&self) -> &DashMap<Uuid, ()> {
            &self.reauth
        }
        fn vault_key(&self) -> Vec<u8> {
            self.key_reads.fetch_add(1, Ordering::SeqCst);
            vec![42; 32]
        }
        fn openai_token_url(&self) -> String {
            self.token_url.clone()
        }
    }

    impl Host {
        fn new(pool: sqlx::PgPool, token_url: String) -> Self {
            let _ = rustls::crypto::ring::default_provider().install_default();
            Self {
                pool,
                token_url,
                client: reqwest::Client::builder().no_proxy().build().unwrap(),
                locks: DashMap::new(),
                reauth: DashMap::new(),
                key_reads: AtomicUsize::new(0),
            }
        }
    }

    fn account(auth_scheme: &str) -> Account {
        Account {
            id: Uuid::new_v4(),
            provider: "openai".into(),
            access_token: Some("static-credential".into()),
            refresh_token: None,
            expires_at: None,
            provider_account_id: None,
            base_url: None,
            auth_scheme: auth_scheme.into(),
            soft_limits: cctui_proto::api::soft_limit::SoftLimits::default(),
            usage_notices: cctui_proto::api::gateway::UsageNotices::default(),
            provider_settings: None,
            rate_limits: cctui_proto::api::gateway::RateLimits::default(),
        }
    }

    #[tokio::test]
    async fn static_credentials_need_neither_database_nor_vault_nor_refresh() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/unused")
            .unwrap();
        pool.close().await;
        let host = Host::new(pool, "http://127.0.0.1:1/unused".into());
        for scheme in ["api_key", "bearer"] {
            let mut acct = account(scheme);
            assert_eq!(current_access_token(&host, &acct).await.unwrap(), "static-credential");
            assert_eq!(refresh_account(&host, &acct).await.unwrap(), "static-credential");
            acct.access_token = None;
            assert_eq!(current_access_token(&host, &acct).await, Err(StatusCode::UNAUTHORIZED));
        }
        assert_eq!(host.key_reads.load(Ordering::SeqCst), 0);
        assert!(host.locks.is_empty());
    }

    async fn provider_pool(test_name: &str) -> Option<sqlx::PgPool> {
        let url = crate::test_support::db_url(test_name)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET search_path = pg_temp").execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::query("CREATE TEMP TABLE account_providers (LIKE public.account_providers INCLUDING DEFAULTS INCLUDING GENERATED)")
            .execute(&pool).await.unwrap();
        Some(pool)
    }

    async fn token(
        axum::extract::State(calls): axum::extract::State<Arc<AtomicUsize>>,
        axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
    ) -> axum::Json<serde_json::Value> {
        assert_eq!(form.get("grant_type").map(String::as_str), Some("refresh_token"));
        assert_eq!(form.get("refresh_token").map(String::as_str), Some("old-refresh"));
        calls.fetch_add(1, Ordering::SeqCst);
        axum::Json(serde_json::json!({
            "access_token": "fresh-access", "refresh_token": "rotated-refresh", "expires_in": 3600,
        }))
    }

    #[tokio::test]
    async fn concurrent_refreshes_rotate_once_without_workspace_storage() {
        let Some(pool) = provider_pool("core_concurrent_refresh").await else { return };
        let calls = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let token_url = format!("http://{}/token", listener.local_addr().unwrap());
        let app = axum::Router::new()
            .route("/token", axum::routing::post(token))
            .with_state(calls.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let host = Host::new(pool, token_url);
        let mut acct = account("oauth");
        acct.access_token = None;
        acct.refresh_token = Some("old-refresh".into());
        sqlx::query("INSERT INTO account_providers (id, user_id, account_id, provider, auth_scheme, encrypted_refresh_token)
            VALUES ($1, $2, $3, 'openai', 'oauth', $4)")
            .bind(acct.id).bind(Uuid::new_v4()).bind(Uuid::new_v4())
            .bind(cctui_crypto::encrypt("old-refresh", &[42; 32]))
            .execute(&host.pool).await.unwrap();
        let (first, second) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(current_access_token(&host, &acct), current_access_token(&host, &acct))
        })
        .await
        .expect("refresh must not deadlock");
        assert_eq!(first.unwrap(), "fresh-access");
        assert_eq!(second.unwrap(), "fresh-access");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let persisted = reload_account(&host, acct.id).await.unwrap();
        assert_eq!(persisted.access_token.as_deref(), Some("fresh-access"));
        assert_eq!(persisted.refresh_token.as_deref(), Some("rotated-refresh"));
        assert!(access_token_is_fresh(
            persisted.access_token.as_deref(),
            persisted.expires_at,
            Utc::now()
        ));
        server.abort();
        host.pool.close().await;
    }
    #[tokio::test]
    async fn rejected_refresh_is_flagged_and_backed_off_without_workspace_storage() {
        let Some(pool) = provider_pool("core_refresh_reauth").await else { return };
        let calls = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let token_url = format!("http://{}/token", listener.local_addr().unwrap());
        let app = axum::Router::new().route(
            "/token",
            axum::routing::post({
                let calls = calls.clone();
                move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async {
                        (
                            StatusCode::BAD_REQUEST,
                            axum::Json(serde_json::json!({"error": "invalid_grant"})),
                        )
                    }
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let host = Host::new(pool, token_url);
        let mut acct = account("oauth");
        acct.access_token = None;
        acct.refresh_token = Some("rejected-refresh".into());
        sqlx::query(
            "INSERT INTO account_providers (id, user_id, account_id, provider)
            VALUES ($1, $2, $3, 'openai')",
        )
        .bind(acct.id)
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .execute(&host.pool)
        .await
        .unwrap();
        for _ in 0..2 {
            assert_eq!(refresh_account(&host, &acct).await, Err(StatusCode::BAD_GATEWAY));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "a rejected token is not replayed");
        assert!(host.reauth.contains_key(&acct.id));
        let error = wait_for_reauth(&host, acct.id, true).await.unwrap();
        assert!(error.contains("invalid_grant"));
        super::super::health::flag_account_reauth(&host, acct.id, "redundant");
        let stored: Option<String> =
            sqlx::query_scalar("SELECT last_auth_error FROM account_providers WHERE id = $1")
                .bind(acct.id)
                .fetch_one(&host.pool)
                .await
                .unwrap();
        assert_eq!(stored.as_deref(), Some(error.as_str()));
        super::super::health::clear_account_reauth(&host, acct.id);
        assert!(!host.reauth.contains_key(&acct.id));
        assert!(wait_for_reauth(&host, acct.id, false).await.is_none());
        server.abort();
        host.pool.close().await;
    }

    async fn wait_for_reauth(host: &Host, id: Uuid, expected: bool) -> Option<String> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let (flagged, error): (bool, Option<String>) = sqlx::query_as(
                    "SELECT needs_reauth, last_auth_error FROM account_providers WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&host.pool)
                .await
                .unwrap();
                if flagged == expected {
                    return error;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("credential flag write must finish")
    }
}
