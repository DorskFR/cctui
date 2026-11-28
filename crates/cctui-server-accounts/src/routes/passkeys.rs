//! Passkey enrolment, management and login over kagi.
//!
//! A login mints an ordinary `auth_keys` row (kind `passkey`, 30-day expiry,
//! the owner's full ceiling) in the same `HttpOnly` cookie `POST /auth/login`
//! sets; logout revokes it. Revoking the last passkey is allowed: token login
//! is the recovery path.

use std::net::IpAddr;
use std::time::SystemTime;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, Duration, Utc};
use kagi::{
    Account, PasskeyRole, Proof, PublicKeyCredential, RegisterPublicKeyCredential, SignedIn,
};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use cctui_server_kernel::auth::{self, AuthContext, Scope};
use cctui_server_kernel::client_addr::{PeerAddr, caller_key};
use cctui_server_kernel::context::KernelContext;
use cctui_server_kernel::error::AppError;

use crate::context::AccountsHost;
use crate::relying_party::{Passkeys, PgCredentials, SESSION_KIND};

pub use cctui_proto::api::passkeys::{
    PasskeyAssertion, PasskeyAutoPromptRequest, PasskeyChallenge, PasskeyConfig,
    PasskeyListResponse, PasskeyRegisterFinish, PasskeyRow, PasskeyTestResult,
    RelabelPasskeyRequest,
};

/// Lifetime of the `auth_keys` row a passkey login mints. The cookie outlives
/// it; when the key expires the next request 401s and the UI returns to login.
const SESSION_DAYS: i64 = 30;

const LABEL_MAX_CHARS: usize = 64;

const AUTO_PROMPT_KEY: &str = "passkey_auto_prompt";

pub const STEP_UP: &str =
    "sign in again to add a passkey: it needs a sign-in from the last 10 minutes";

type ApiResult<T> = Result<T, AppError>;

fn bad_request(msg: impl Into<String>) -> AppError {
    AppError::new(StatusCode::BAD_REQUEST, msg)
}

fn relying_party(state: &impl AccountsHost) -> ApiResult<&Passkeys> {
    state.passkeys().ok_or_else(|| {
        AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "passkeys are not configured on this server (set CCTUI_EXTERNAL_URL to the \
             public https URL)",
        )
    })
}

fn passkey_error(error: kagi::Error) -> AppError {
    use kagi::Error as E;
    let (code, msg) = match error {
        E::TooMany { retry_after } => (
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "too many passkey attempts from your address: retry in {}s",
                retry_after.as_secs()
            ),
        ),
        E::Busy => (
            StatusCode::SERVICE_UNAVAILABLE,
            "too many passkey ceremonies in flight: retry shortly".to_owned(),
        ),
        E::Ceremony => (StatusCode::BAD_REQUEST, "challenge expired or already used".to_owned()),
        E::Purpose => {
            (StatusCode::BAD_REQUEST, "this challenge was issued for something else".to_owned())
        }
        E::Binding => {
            (StatusCode::FORBIDDEN, "this challenge belongs to another session".to_owned())
        }
        E::Rejected { .. } => (StatusCode::BAD_REQUEST, "the passkey was not accepted".to_owned()),
        E::Cloned { .. } => {
            tracing::warn!("passkey signature counter went backwards: the key may be cloned");
            (StatusCode::BAD_REQUEST, "the passkey was not accepted".to_owned())
        }
        E::StepUp => (StatusCode::FORBIDDEN, STEP_UP.to_owned()),
        E::Duplicate => (StatusCode::CONFLICT, "this passkey is already registered".to_owned()),
        E::LastCredential => {
            (StatusCode::CONFLICT, "the last way to sign in cannot be removed".to_owned())
        }
        E::NotFound => (StatusCode::NOT_FOUND, "no such passkey".to_owned()),
        E::Config(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg),
        other @ (E::Store(_) | E::Internal(_) | E::Webauthn(_)) => {
            tracing::error!("passkeys failed: {other}");
            (StatusCode::INTERNAL_SERVER_ERROR, "passkeys failed".to_owned())
        }
    };
    AppError::new(code, msg)
}

fn client(state: &impl KernelContext, headers: &HeaderMap, peer: &PeerAddr) -> Option<IpAddr> {
    caller_key(headers, peer.0, state.config().trusted_proxy_hops).parse().ok()
}

/// Trim a user-supplied label, falling back to a neutral one so a key is never
/// nameless in the list.
fn normalize_label(raw: Option<&str>) -> ApiResult<String> {
    let trimmed = raw.unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        return Ok("Passkey".to_owned());
    }
    if trimmed.chars().count() > LABEL_MAX_CHARS {
        return Err(bad_request(format!("label must be at most {LABEL_MAX_CHARS} characters")));
    }
    Ok(trimmed)
}

fn parse<T: serde::de::DeserializeOwned>(credential: Value) -> ApiResult<T> {
    serde_json::from_value(credential)
        .map_err(|e| bad_request(format!("malformed credential: {e}")))
}

/// A passkey signs in with the account's full ceiling, so only a credential that
/// already holds it may add or change one: no machine, dispatcher, worker or
/// plugin key, and no key narrower than its owner.
async fn require_full_human_credential(pool: &PgPool, ctx: &AuthContext) -> ApiResult<()> {
    let plugin: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM auth_keys WHERE id = $1 AND kind = 'plugin')",
    )
    .bind(ctx.key_id)
    .fetch_one(pool)
    .await?;
    if plugin || !auth::is_human_credential(pool, ctx).await? {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "passkeys can only be managed from a user sign-in",
        ));
    }
    let ceiling = cctui_server_kernel::store::acls::user_ceiling(pool, ctx.user_id).await?;
    if !ceiling.is_subset(&ctx.scopes) {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "this key holds fewer scopes than your account: sign in again to manage passkeys",
        ));
    }
    Ok(())
}

/// Marks the interactive sign-in a key was presented with.
pub async fn record_sign_in(
    pool: &PgPool,
    key_id: Uuid,
    at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE auth_keys SET signed_in_at = $2 WHERE id = $1")
        .bind(key_id)
        .bind(at)
        .execute(pool)
        .await
        .map(drop)
}

/// Only a browser session counts: a key sent as a bearer token is an API or CLI
/// credential, whenever it last signed in to the web UI.
async fn registration_proof(
    pool: &PgPool,
    ctx: &AuthContext,
    headers: &HeaderMap,
) -> ApiResult<Proof> {
    let by_cookie =
        !headers.contains_key(header::AUTHORIZATION) && auth::token_from_cookies(headers).is_some();
    let signed_in_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT signed_in_at FROM auth_keys WHERE id = $1")
            .bind(ctx.key_id)
            .fetch_optional(pool)
            .await?
            .flatten();
    Ok(match signed_in_at {
        Some(at) if by_cookie => Proof::RecentSignIn { at: at.into(), human: true },
        _ => Proof::RecentSignIn { at: SystemTime::UNIX_EPOCH, human: false },
    })
}

/// `POST /passkeys/register/start` — options for creating a passkey.
pub async fn register_start<S: AccountsHost>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    headers: HeaderMap,
    peer: PeerAddr,
) -> ApiResult<Json<PasskeyChallenge>> {
    let pool = state.kernel_pool();
    require_full_human_credential(pool, &ctx).await?;
    let passkeys = relying_party(&state)?;
    let proof = registration_proof(pool, &ctx, &headers).await?;
    let name = sqlx::query_scalar::<_, String>("SELECT name FROM users WHERE id = $1")
        .bind(ctx.user_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or_else(|| "cctui".to_owned());
    let account = Account { user: ctx.user_id, name: &name, display_name: &name };
    let started = passkeys
        .start_registration(
            &PgCredentials::new(pool),
            account,
            proof,
            &ctx.key_id.to_string(),
            client(&state, &headers, &peer),
        )
        .await
        .map_err(passkey_error)?;
    Ok(Json(PasskeyChallenge {
        ceremony: started.ceremony,
        options: serde_json::to_value(&started.options)?,
    }))
}

/// `POST /passkeys/register/finish` — verify and store the new credential.
pub async fn register_finish<S: AccountsHost>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<PasskeyRegisterFinish>,
) -> ApiResult<Json<PasskeyRow>> {
    let pool = state.kernel_pool();
    require_full_human_credential(pool, &ctx).await?;
    let passkeys = relying_party(&state)?;
    let label = normalize_label(req.label.as_deref())?;
    let credential: RegisterPublicKeyCredential = parse(req.credential)?;
    let registered = passkeys
        .finish_registration(
            &PgCredentials::new(pool),
            &req.ceremony,
            &ctx.key_id.to_string(),
            &credential,
            &label,
        )
        .await
        .map_err(passkey_error)?;
    // Absent `credProps` means the browser did not say; the key probably works.
    let discoverable = registered.discoverable.unwrap_or(true);
    let created_at: DateTime<Utc> = sqlx::query_scalar(
        "UPDATE webauthn_credentials SET discoverable = $2 WHERE id = $1 RETURNING created_at",
    )
    .bind(registered.passkey)
    .bind(discoverable)
    .fetch_one(pool)
    .await?;
    tracing::info!(user_id = %ctx.user_id, passkey_id = %registered.passkey, "passkey enrolled");
    Ok(Json(PasskeyRow {
        id: registered.passkey,
        label,
        discoverable,
        created_at,
        last_used_at: None,
    }))
}

async fn passkeys_of(pool: &PgPool, user_id: Uuid) -> ApiResult<PasskeyListResponse> {
    let rows = sqlx::query_as::<_, (Uuid, String, bool, DateTime<Utc>, Option<DateTime<Utc>>)>(
        "SELECT id, label, discoverable, created_at, last_used_at FROM webauthn_credentials \
         WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(PasskeyListResponse {
        passkeys: rows
            .into_iter()
            .map(|(id, label, discoverable, created_at, last_used_at)| PasskeyRow {
                id,
                label,
                discoverable,
                created_at,
                last_used_at,
            })
            .collect(),
    })
}

/// `GET /passkeys` — the caller's own keys, newest first.
pub async fn list<S: KernelContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<Json<PasskeyListResponse>> {
    passkeys_of(state.pool(), ctx.user_id).await.map(Json)
}

/// `GET /admin/users/{id}/passkeys` — another user's keys, newest first.
pub async fn admin_list<S: KernelContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
) -> ApiResult<Json<PasskeyListResponse>> {
    ctx.requires(Scope::Admin).map_err(|s| AppError::new(s, "admin token required"))?;
    passkeys_of(state.pool(), user_id).await.map(Json)
}

/// `PATCH /passkeys/{id}` — rename one of the caller's keys.
pub async fn relabel<S: KernelContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<RelabelPasskeyRequest>,
) -> ApiResult<StatusCode> {
    require_full_human_credential(state.pool(), &ctx).await?;
    let label = normalize_label(Some(&req.label))?;
    let done =
        sqlx::query("UPDATE webauthn_credentials SET label = $1 WHERE id = $2 AND user_id = $3")
            .bind(&label)
            .bind(id)
            .bind(ctx.user_id)
            .execute(state.pool())
            .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such passkey"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Drops one of `user_id`'s keys and ends the sessions it opened, returning how
/// many it ended.
async fn revoke_passkey(
    passkeys: &Passkeys,
    pool: &PgPool,
    auth_config: &auth::AuthConfig,
    user_id: Uuid,
    id: Uuid,
) -> ApiResult<usize> {
    let credentials = PgCredentials::new(pool);
    passkeys
        .remove(&credentials, &user_id, &id, PasskeyRole::Alternative, true)
        .await
        .map_err(passkey_error)?;
    let hashes = credentials.revoked();
    for hash in &hashes {
        auth_config.purge(hash);
    }
    Ok(hashes.len())
}

/// `DELETE /passkeys/{id}` — drop one of the caller's keys and end the sessions
/// it opened.
pub async fn revoke<S: AccountsHost>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let pool = state.kernel_pool();
    require_full_human_credential(pool, &ctx).await?;
    let sessions =
        revoke_passkey(relying_party(&state)?, pool, state.auth_config(), ctx.user_id, id).await?;
    tracing::info!(user_id = %ctx.user_id, passkey_id = %id, sessions, "passkey revoked");
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /admin/users/{id}/passkeys/{passkey}` — drop another user's key and
/// end the sessions it opened.
pub async fn admin_revoke<S: AccountsHost>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path((user_id, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    ctx.requires(Scope::Admin).map_err(|s| AppError::new(s, "admin token required"))?;
    let pool = state.kernel_pool();
    let sessions =
        revoke_passkey(relying_party(&state)?, pool, state.auth_config(), user_id, id).await?;
    tracing::info!(
        admin_id = %ctx.user_id, %user_id, passkey_id = %id, sessions,
        "passkey revoked by an admin"
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn start_assertion(
    state: &impl AccountsHost,
    headers: &HeaderMap,
    peer: PeerAddr,
) -> ApiResult<Json<PasskeyChallenge>> {
    let started = relying_party(state)?
        .start_discoverable(client(state, headers, &peer), false)
        .await
        .map_err(passkey_error)?;
    Ok(Json(PasskeyChallenge {
        ceremony: started.ceremony,
        options: serde_json::to_value(&started.options)?,
    }))
}

struct Asserted {
    signed_in: SignedIn<Uuid, Uuid>,
    label: String,
}

/// A verified assertion from an active user. `refused` is the status a rejected
/// passkey answers with.
async fn finish_assertion(
    state: &impl AccountsHost,
    req: PasskeyAssertion,
    refused: StatusCode,
) -> ApiResult<Asserted> {
    let pool = state.kernel_pool();
    let credential: PublicKeyCredential = parse(req.credential)?;
    let signed_in = relying_party(state)?
        .finish_sign_in(&PgCredentials::new(pool), &req.ceremony, &credential)
        .await
        .map_err(|error| match passkey_error(error) {
            AppError::Status(StatusCode::BAD_REQUEST, msg) => AppError::new(refused, msg),
            other => other,
        })?;
    let label: Option<String> = sqlx::query_scalar(
        "UPDATE webauthn_credentials c SET last_used_at = now() FROM users u \
         WHERE c.id = $1 AND u.id = c.user_id AND u.revoked_at IS NULL AND u.disabled_at IS NULL \
         RETURNING c.label",
    )
    .bind(signed_in.passkey)
    .fetch_optional(pool)
    .await?;
    let label = label.ok_or_else(|| AppError::new(refused, "unknown passkey"))?;
    Ok(Asserted { signed_in, label })
}

/// `POST /passkeys/test/start` — same ceremony as login, run while signed in.
pub async fn test_start<S: AccountsHost>(
    State(state): State<S>,
    headers: HeaderMap,
    peer: PeerAddr,
) -> ApiResult<Json<PasskeyChallenge>> {
    start_assertion(&state, &headers, peer).await
}

/// `POST /passkeys/test/finish` — verify, and insist the key is the caller's
/// own. Mints nothing: this answers "does it work", not "let me in".
pub async fn test_finish<S: AccountsHost>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<PasskeyAssertion>,
) -> ApiResult<Json<PasskeyTestResult>> {
    let asserted = finish_assertion(&state, req, StatusCode::BAD_REQUEST).await?;
    if asserted.signed_in.user != ctx.user_id {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "that passkey belongs to another account",
        ));
    }
    tracing::info!(user_id = %ctx.user_id, passkey_id = %asserted.signed_in.passkey, "passkey tested");
    Ok(Json(PasskeyTestResult { label: asserted.label }))
}

/// `GET /auth/passkey/config` — what the login screen needs to know before
/// anyone has authenticated.
pub async fn config<S: AccountsHost>(State(state): State<S>) -> Json<PasskeyConfig> {
    let available = state.passkeys().is_some();
    let enrolled = available
        && sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM webauthn_credentials)")
            .fetch_one(state.kernel_pool())
            .await
            .unwrap_or(false);
    Json(PasskeyConfig {
        available,
        enrolled,
        auto_prompt: read_auto_prompt(state.kernel_pool()).await,
    })
}

/// `POST /auth/passkey/login/start` — options for a usernameless sign-in.
pub async fn login_start<S: AccountsHost>(
    State(state): State<S>,
    headers: HeaderMap,
    peer: PeerAddr,
) -> ApiResult<Json<PasskeyChallenge>> {
    start_assertion(&state, &headers, peer).await
}

/// `POST /auth/passkey/login/finish` — verify the assertion, mint a session key
/// for the user it resolved to, and set the auth cookie.
pub async fn login_finish<S: AccountsHost>(
    State(state): State<S>,
    headers: HeaderMap,
    Json(req): Json<PasskeyAssertion>,
) -> Result<Response, AppError> {
    let Asserted { signed_in, label } =
        finish_assertion(&state, req, StatusCode::UNAUTHORIZED).await?;
    let pool = state.kernel_pool();
    let token = auth::user_token(&auth::mint_secret());
    let hash = auth::sha256_hex(&token);
    let preview = auth::token_preview(&token);
    let scopes = cctui_server_kernel::store::acls::user_ceiling(pool, signed_in.user).await?;
    let label = format!("passkey: {label}");
    let key_id = auth::register_key(
        pool,
        auth::NewKey {
            user_id: signed_in.user,
            key_hash: &hash,
            key_preview: Some(&preview),
            label: Some(&label),
            kind: SESSION_KIND,
            machine_id: None,
            dispatcher_id: None,
            expires_at: Some(Utc::now() + Duration::days(SESSION_DAYS)),
            passkey_id: Some(signed_in.passkey),
        },
        scopes,
    )
    .await?;
    record_sign_in(pool, key_id, signed_in.at.into()).await?;

    let _ = sqlx::query(
        "DELETE FROM auth_keys WHERE kind = $1 AND expires_at < now() - interval '7 days'",
    )
    .bind(SESSION_KIND)
    .execute(pool)
    .await;

    tracing::info!(user_id = %signed_in.user, %key_id, "passkey login");
    let cookie = auth::set_auth_cookie(&token, auth::request_is_https(&headers));
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response())
}

/// The stored auto-prompt flag, defaulting to off — a modal that opens itself
/// is a choice an operator makes, not one they inherit.
pub async fn read_auto_prompt(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, Value>("SELECT value FROM instance_settings WHERE key = $1")
        .bind(AUTO_PROMPT_KEY)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// `PUT /admin/passkeys/auto-prompt` — server-wide: start the passkey read as
/// soon as the login screen opens, instead of waiting for a click.
pub async fn set_auto_prompt<S: KernelContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<PasskeyAutoPromptRequest>,
) -> ApiResult<StatusCode> {
    ctx.requires(Scope::Admin).map_err(|s| AppError::new(s, "admin token required"))?;
    sqlx::query(
        "INSERT INTO instance_settings (key, value, updated_at) \
         VALUES ($1, to_jsonb($2::bool), now()) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(AUTO_PROMPT_KEY)
    .bind(req.auto_prompt)
    .execute(state.pool())
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Revoke the `auth_keys` row a passkey login minted, if that is what the
/// presented token is.
///
/// Called from logout so a passkey session leaves nothing
/// behind; a token session is untouched (its key is the user's, not ours).
pub async fn revoke_session_key(pool: &PgPool, token: &str) {
    let hash = auth::sha256_hex(token);
    let _ = sqlx::query(
        "UPDATE auth_keys SET revoked_at = now() \
         WHERE key_hash = $1 AND kind = $2 AND revoked_at IS NULL",
    )
    .bind(&hash)
    .bind(SESSION_KIND)
    .execute(pool)
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use kagi::{Passkey, Removed};

    #[test]
    fn a_passkey_stored_before_kagi_still_loads() {
        let stored = r#"{"cred":{"attestation":{"data":"None","metadata":"None"},"attestation_format":"none","backup_eligible":true,"backup_state":true,"counter":7,"cred":{"key":{"EC_EC2":{"curve":"SECP256R1","x":"AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA","y":"ISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0-P0A"}},"type_":"ES256"},"cred_id":"AAECAwQFBgcICQoLDA0ODw","extensions":{"appid":"NotRequested","cred_props":"NotRequested","cred_protect":"NotRequested","hmac_create_secret":"NotRequested"},"registration_policy":"required","transports":["internal","hybrid"],"user_verified":true}}"#;
        let passkey: Passkey = serde_json::from_str(stored).unwrap();
        assert_eq!(passkey.cred_id().as_slice(), (0u8..16).collect::<Vec<_>>());
        let reloaded: Passkey =
            serde_json::from_value(serde_json::to_value(&passkey).unwrap()).unwrap();
        assert_eq!(reloaded.cred_id(), passkey.cred_id());
        assert_eq!(
            serde_json::to_value(&reloaded).unwrap(),
            serde_json::from_str::<Value>(stored).unwrap()
        );
    }

    #[test]
    fn label_defaults_and_trims() {
        assert_eq!(normalize_label(None).unwrap(), "Passkey");
        assert_eq!(normalize_label(Some("   ")).unwrap(), "Passkey");
        assert_eq!(normalize_label(Some("  iPhone  de  David ")).unwrap(), "iPhone de David");
        assert_eq!(normalize_label(Some("Clé Yubico")).unwrap(), "Clé Yubico");
    }

    #[test]
    fn label_rejects_an_essay() {
        let long = "x".repeat(LABEL_MAX_CHARS + 1);
        assert!(matches!(
            normalize_label(Some(&long)).unwrap_err(),
            AppError::Status(code, _) if code == StatusCode::BAD_REQUEST
        ));
        assert!(normalize_label(Some(&"x".repeat(LABEL_MAX_CHARS))).is_ok());
    }

    #[test]
    fn a_stale_sign_in_says_to_sign_in_again_without_logging_out() {
        let AppError::Status(code, msg) = passkey_error(kagi::Error::StepUp) else {
            panic!("a status");
        };
        assert_eq!(code, StatusCode::FORBIDDEN);
        assert!(msg.starts_with("sign in again"), "{msg}");
    }

    fn relying_party_on(pool: &PgPool) -> Passkeys {
        crate::relying_party::build("https://cctui.example.com", None, pool.clone(), b"test")
            .expect("relying party")
    }

    async fn user(pool: &PgPool, scopes: &str) -> Uuid {
        let uid: Uuid = sqlx::query_scalar(
            "INSERT INTO users (id, name, key_hash) \
             VALUES (gen_random_uuid(), $1, gen_random_uuid()::text) RETURNING id",
        )
        .bind(format!("passkey-test-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .expect("insert user");
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO user_acls (user_id, scope) SELECT $1, unnest(ARRAY[{scopes}])"
        )))
        .bind(uid)
        .execute(pool)
        .await
        .expect("insert ceiling");
        uid
    }

    async fn key(pool: &PgPool, uid: Uuid, kind: &str, scopes: &[Scope]) -> (String, AuthContext) {
        let token = auth::user_token(&auth::mint_secret());
        let key_id = auth::register_key(
            pool,
            auth::NewKey {
                user_id: uid,
                key_hash: &auth::sha256_hex(&token),
                key_preview: None,
                label: Some("test"),
                kind,
                machine_id: None,
                dispatcher_id: None,
                expires_at: None,
                passkey_id: None,
            },
            scopes.iter().copied(),
        )
        .await
        .expect("register key");
        let ctx = AuthContext {
            user_id: uid,
            key_id,
            machine_id: None,
            scopes: scopes.iter().copied().collect(),
        };
        (token, ctx)
    }

    fn cookie(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, format!("{}={token}", auth::AUTH_COOKIE).parse().unwrap());
        headers
    }

    #[tokio::test]
    async fn passkey_revoke_kills_its_login_keys() {
        let Some(url) =
            cctui_server_kernel::test_support::db_url("passkey_revoke_kills_its_login_keys")
        else {
            return;
        };
        let pool = PgPool::connect(&url).await.expect("connect test db");
        let uid = user(&pool, "'read'").await;
        let passkey_id: Uuid = sqlx::query_scalar(
            "INSERT INTO webauthn_credentials (user_id, credential_id, passkey, label) \
             VALUES ($1, gen_random_uuid()::text::bytea, '{}', 'test') RETURNING id",
        )
        .bind(uid)
        .fetch_one(&pool)
        .await
        .expect("insert passkey");

        let token = auth::user_token(&auth::mint_secret());
        let hash = auth::sha256_hex(&token);
        auth::register_key(
            &pool,
            auth::NewKey {
                user_id: uid,
                key_hash: &hash,
                key_preview: None,
                label: Some("passkey: test"),
                kind: SESSION_KIND,
                machine_id: None,
                dispatcher_id: None,
                expires_at: Some(Utc::now() + Duration::days(SESSION_DAYS)),
                passkey_id: Some(passkey_id),
            },
            [Scope::Read],
        )
        .await
        .expect("register key");
        let cfg = auth::AuthConfig::new(vec![], pool.clone());
        assert!(cfg.validate(&token).await.is_some(), "cookie works before revoke");

        let passkeys = relying_party_on(&pool);
        let credentials = PgCredentials::new(&pool);
        let removed = passkeys
            .remove(&credentials, &uid, &passkey_id, PasskeyRole::Alternative, true)
            .await
            .expect("the last passkey may go: token login remains");
        assert_eq!(removed, Removed { passkey: passkey_id, remaining: 0 });
        let hashes = credentials.revoked();
        assert_eq!(hashes, vec![hash.clone()]);
        for h in &hashes {
            cfg.purge(h);
        }
        assert!(cfg.validate(&token).await.is_none(), "cookie is dead after revoke");
        let again =
            passkeys.remove(&credentials, &uid, &passkey_id, PasskeyRole::Alternative, true).await;
        assert!(matches!(again, Err(kagi::Error::NotFound)));

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    async fn passkey_with_session(pool: &PgPool, uid: Uuid) -> (Uuid, String) {
        let passkey_id: Uuid = sqlx::query_scalar(
            "INSERT INTO webauthn_credentials (user_id, credential_id, passkey, label) \
             VALUES ($1, gen_random_uuid()::text::bytea, '{}', 'test') RETURNING id",
        )
        .bind(uid)
        .fetch_one(pool)
        .await
        .expect("insert passkey");
        let token = auth::user_token(&auth::mint_secret());
        auth::register_key(
            pool,
            auth::NewKey {
                user_id: uid,
                key_hash: &auth::sha256_hex(&token),
                key_preview: None,
                label: Some("passkey: test"),
                kind: SESSION_KIND,
                machine_id: None,
                dispatcher_id: None,
                expires_at: Some(Utc::now() + Duration::days(SESSION_DAYS)),
                passkey_id: Some(passkey_id),
            },
            [Scope::Read],
        )
        .await
        .expect("register key");
        (passkey_id, token)
    }

    #[tokio::test]
    async fn an_admin_lists_and_revokes_another_users_passkey() {
        let Some(url) = cctui_server_kernel::test_support::db_url(
            "an_admin_lists_and_revokes_another_users_passkey",
        ) else {
            return;
        };
        let pool = PgPool::connect(&url).await.expect("connect test db");
        let host = cctui_server_kernel::test_support::TestHost::new(pool.clone());
        let passkeys = relying_party_on(&pool);
        let admin = user(&pool, "'admin', 'read'").await;
        let (_, admin_ctx) = key(&pool, admin, "user", &[Scope::Admin, Scope::Read]).await;
        let (_, reader_ctx) = key(&pool, admin, "user", &[Scope::Read]).await;
        let target = user(&pool, "'read'").await;
        let bystander = user(&pool, "'read'").await;
        let (passkey_id, token) = passkey_with_session(&pool, target).await;
        let (kept_id, kept_token) = passkey_with_session(&pool, bystander).await;
        let cfg = &host.auth_config;
        let listed =
            |ctx: AuthContext| admin_list(State(host.clone()), Extension(ctx), Path(target));

        let rows = listed(admin_ctx.clone()).await.expect("admin lists").0.passkeys;
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![passkey_id]);
        assert!(matches!(
            listed(reader_ctx).await,
            Err(AppError::Status(code, _)) if code == StatusCode::FORBIDDEN
        ));

        let wrong_owner = revoke_passkey(&passkeys, &pool, cfg, bystander, passkey_id).await;
        assert!(matches!(
            wrong_owner,
            Err(AppError::Status(code, _)) if code == StatusCode::NOT_FOUND
        ));
        assert!(cfg.validate(&token).await.is_some(), "a refused revoke ends nothing");

        let ended = revoke_passkey(&passkeys, &pool, cfg, target, passkey_id).await;
        assert_eq!(ended.expect("admin revokes"), 1);
        assert!(cfg.validate(&token).await.is_none(), "the passkey's session is dead");
        assert!(listed(admin_ctx).await.expect("admin lists").0.passkeys.is_empty());
        assert_eq!(passkeys_of(&pool, bystander).await.unwrap().passkeys[0].id, kept_id);
        assert!(cfg.validate(&kept_token).await.is_some(), "another user's session survives");

        for uid in [admin, target, bystander] {
            sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
        }
    }

    #[tokio::test]
    async fn only_a_full_human_credential_manages_passkeys() {
        let Some(url) = cctui_server_kernel::test_support::db_url(
            "only_a_full_human_credential_manages_passkeys",
        ) else {
            return;
        };
        let pool = PgPool::connect(&url).await.expect("connect test db");
        let uid = user(&pool, "'read', 'dispatch'").await;
        let full = [Scope::Read, Scope::Dispatch];
        let refused = |r: ApiResult<()>| matches!(r, Err(AppError::Status(code, _)) if code == StatusCode::FORBIDDEN);

        let (_, user_key) = key(&pool, uid, "user", &full).await;
        assert!(require_full_human_credential(&pool, &user_key).await.is_ok());
        let (_, passkey_login) = key(&pool, uid, SESSION_KIND, &full).await;
        assert!(require_full_human_credential(&pool, &passkey_login).await.is_ok());
        let (_, narrow) = key(&pool, uid, "user", &[Scope::Read]).await;
        assert!(refused(require_full_human_credential(&pool, &narrow).await));
        for kind in ["plugin", "dispatcher", "ephemeral", "machine"] {
            let (_, ctx) = key(&pool, uid, kind, &full).await;
            assert!(refused(require_full_human_credential(&pool, &ctx).await), "{kind} key");
        }
        let machine = AuthContext { machine_id: Some(Uuid::new_v4()), ..user_key };
        assert!(refused(require_full_human_credential(&pool, &machine).await));

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    #[tokio::test]
    async fn adding_a_passkey_needs_a_recent_sign_in_from_the_browser() {
        let Some(url) = cctui_server_kernel::test_support::db_url(
            "adding_a_passkey_needs_a_recent_sign_in_from_the_browser",
        ) else {
            return;
        };
        let pool = PgPool::connect(&url).await.expect("connect test db");
        let passkeys = relying_party_on(&pool);
        let credentials = PgCredentials::new(&pool);
        let uid = user(&pool, "'read'").await;
        let (token, ctx) = key(&pool, uid, "user", &[Scope::Read]).await;
        let start = |headers: HeaderMap| {
            let (pool, ctx, passkeys, credentials) = (&pool, &ctx, &passkeys, &credentials);
            async move {
                let proof = registration_proof(pool, ctx, &headers).await.unwrap();
                let account = Account { user: uid, name: "test", display_name: "test" };
                passkeys
                    .start_registration(credentials, account, proof, &ctx.key_id.to_string(), None)
                    .await
            }
        };
        let step_up = |r: Result<kagi::RegistrationChallenge, kagi::Error>| {
            matches!(r, Err(kagi::Error::StepUp))
        };

        assert!(step_up(start(cookie(&token)).await), "a key that never signed in");

        record_sign_in(&pool, ctx.key_id, Utc::now()).await.unwrap();
        assert!(start(cookie(&token)).await.is_ok(), "a fresh browser sign-in");

        let mut bearer = cookie(&token);
        bearer.insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        assert!(step_up(start(bearer).await), "the same key used as an API token");
        assert!(step_up(start(HeaderMap::new()).await), "no cookie at all");

        record_sign_in(&pool, ctx.key_id, Utc::now() - Duration::minutes(11)).await.unwrap();
        assert!(step_up(start(cookie(&token)).await), "a sign-in older than the window");

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}
