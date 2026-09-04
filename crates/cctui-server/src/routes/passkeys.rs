//! Passkey (WebAuthn) enrolment, management and login.
//!
//! ## The shape of the thing
//!
//! Three ceremonies, each a `start` that hands the browser a challenge and a
//! `finish` that verifies what came back. The in-flight state lives in
//! `webauthn_challenges` (single-use, TTL-bounded) rather than in memory, so a
//! ceremony survives a restart and works with more than one replica.
//!
//!   * **register** (authenticated) — enrol a key on the caller's account. As
//!     many as they like: they are rows, not a column.
//!   * **test** (authenticated) — prove the key that was just enrolled actually
//!     answers, without logging anyone out to find out.
//!   * **login** (unauthenticated) — usernameless. The browser discovers the
//!     credential and returns the user handle we stored at registration, so the
//!     login screen never asks who you are.
//!
//! ## What a successful login *is*
//!
//! Not a new session concept: a passkey assertion mints an ordinary `auth_keys`
//! row (kind `passkey`, 30-day expiry, the owner's full ceiling) and puts that
//! token in the same `HttpOnly` cookie `POST /auth/login` sets. Every existing
//! authz path is untouched. Logging out revokes the minted key, so a passkey
//! session leaves nothing behind.
//!
//! The token login is never removed, and revoking your last passkey is allowed:
//! the token is the recovery path, and it must stay one.

// "WebAuthn" and the authenticator brand names below are proper nouns that trip
// clippy's camel-case doc heuristic throughout this module; none is a code item.
#![allow(clippy::doc_markdown)]

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use ts_rs::TS;
use uuid::Uuid;
use webauthn_rs::prelude::{
    DiscoverableAuthentication, DiscoverableKey, Passkey, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential,
};

use crate::auth::{self, AuthContext, Scope};
use crate::state::AppState;
use cctui_proto::api::ApiError;

/// How long a browser has to answer a challenge before it is swept. Comfortably
/// past the authenticator timeout webauthn-rs asks for, so the user meets the
/// authenticator's own deadline first and gets its error, not ours.
const CHALLENGE_TTL_MINUTES: i64 = 10;

/// Lifetime of the `auth_keys` row a passkey login mints. The cookie outlives
/// it; when the key expires the next request 401s and the UI returns to login,
/// where the passkey signs in again in one gesture.
const SESSION_DAYS: i64 = 30;

/// `auth_keys.kind` for a token minted by a passkey assertion. Lets logout
/// revoke exactly these, and keeps them legible in the admin key list.
const SESSION_KIND: &str = "passkey";

/// Longest label we store for a key. It is a human's note ("iPhone",
/// "Bitwarden"), not a document.
const LABEL_MAX_CHARS: usize = 64;

/// The `instance_settings` key behind the admin-wide "try the passkey as soon
/// as the login screen opens" toggle.
const AUTO_PROMPT_KEY: &str = "passkey_auto_prompt";

type ApiResult<T> = Result<T, (StatusCode, Json<ApiError>)>;

fn bad_request(msg: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (StatusCode::BAD_REQUEST, Json(ApiError { error: msg.into() }))
}

fn db_error(e: &sqlx::Error) -> (StatusCode, Json<ApiError>) {
    tracing::error!("passkey store error: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
}

/// The relying party, or a 503 explaining that this deployment has none. Every
/// ceremony starts here so an unconfigured server fails loudly on the route and
/// silently in the UI (which asked `/auth/passkey/config` first).
fn relying_party(state: &AppState) -> ApiResult<&webauthn_rs::Webauthn> {
    state.webauthn.as_deref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                error: "passkeys are not configured on this server (set CCTUI_EXTERNAL_URL to the \
                        public https URL)"
                    .into(),
            }),
        )
    })
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

// ---------------------------------------------------------------------------
// Challenge store
// ---------------------------------------------------------------------------

/// Park a ceremony's state and return its id. The id is the only handle the
/// browser gets: it names a row, it is not the challenge.
async fn stash_challenge(
    pool: &PgPool,
    user_id: Option<Uuid>,
    kind: &str,
    state: &Value,
) -> Result<Uuid, sqlx::Error> {
    // Opportunistic sweep. Expired rows are dead weight, and this is the only
    // moment we are guaranteed to be writing to the table anyway.
    let _ =
        sqlx::query("DELETE FROM webauthn_challenges WHERE expires_at < now()").execute(pool).await;
    let id: (Uuid,) = sqlx::query_as(
        "INSERT INTO webauthn_challenges (user_id, kind, state, expires_at) \
         VALUES ($1, $2, $3, now() + ($4 || ' minutes')::interval) RETURNING id",
    )
    .bind(user_id)
    .bind(kind)
    .bind(state)
    .bind(CHALLENGE_TTL_MINUTES.to_string())
    .fetch_one(pool)
    .await?;
    Ok(id.0)
}

/// Consume a ceremony's state: single-use by construction, since the DELETE
/// returns it. An expired or already-used challenge simply isn't there.
async fn take_challenge(
    pool: &PgPool,
    id: Uuid,
    kind: &str,
    user_id: Option<Uuid>,
) -> ApiResult<Value> {
    let row: Option<(Value,)> = sqlx::query_as(
        "DELETE FROM webauthn_challenges \
         WHERE id = $1 AND kind = $2 AND expires_at > now() \
         AND ($3::uuid IS NULL OR user_id = $3) \
         RETURNING state",
    )
    .bind(id)
    .bind(kind)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| db_error(&e))?;
    row.map(|(v,)| v).ok_or_else(|| bad_request("challenge expired or already used"))
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Serialize, TS)]
#[ts(export)]
pub struct PasskeyChallenge {
    /// Handle for the parked ceremony state; echoed back on finish.
    pub challenge_id: Uuid,
    /// The raw WebAuthn options, passed to `navigator.credentials.*` verbatim
    /// after the browser-side base64url decoding. Deliberately untyped here:
    /// the shape is the W3C one and webauthn-rs owns it.
    pub options: Value,
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct PasskeyRegisterFinish {
    pub challenge_id: Uuid,
    /// Human label for the key ("iPhone", "YubiKey", "Bitwarden").
    pub label: Option<String>,
    /// The `PublicKeyCredential` from `navigator.credentials.create()`.
    pub credential: Value,
    /// `credProps.rk` as the browser reported it, when it did. `false` means
    /// the key is not discoverable and so cannot drive the usernameless login.
    pub discoverable: Option<bool>,
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct PasskeyAssertion {
    pub challenge_id: Uuid,
    /// The `PublicKeyCredential` from `navigator.credentials.get()`.
    pub credential: Value,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct PasskeyRow {
    pub id: Uuid,
    pub label: String,
    /// False when the authenticator declined to store a discoverable
    /// credential: the key still works as a second factor but will not appear
    /// at the login screen, and the UI says so.
    pub discoverable: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct PasskeyListResponse {
    pub passkeys: Vec<PasskeyRow>,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct PasskeyConfig {
    /// Whether this server can run a passkey ceremony at all (relying party
    /// configured). False means the login screen shows only the token box.
    pub available: bool,
    /// Whether anyone has enrolled a key. The login screen offers the passkey
    /// button only when a ceremony could actually succeed.
    pub enrolled: bool,
    /// Server-wide admin setting: attempt the passkey read as soon as the login
    /// screen opens instead of waiting for a click. The user can always dismiss
    /// it and type a token.
    pub auto_prompt: bool,
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct PasskeyAutoPromptRequest {
    pub auto_prompt: bool,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct PasskeyTestResult {
    /// The label of the key that answered, so the UI can say which one.
    pub label: String,
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct RelabelPasskeyRequest {
    pub label: String,
}

// ---------------------------------------------------------------------------
// Registration (authenticated)
// ---------------------------------------------------------------------------

/// `POST /passkeys/register/start` — options for `navigator.credentials.create()`.
///
/// Already-enrolled credentials are excluded, so a second attempt on the same
/// authenticator is refused by the browser rather than producing a duplicate.
pub async fn register_start(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<Json<PasskeyChallenge>> {
    let webauthn = relying_party(&state)?;
    let name = sqlx::query_scalar::<_, String>("SELECT name FROM users WHERE id = $1")
        .bind(ctx.user_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| db_error(&e))?
        .unwrap_or_else(|| "cctui".to_owned());

    let existing: Vec<(Vec<u8>,)> =
        sqlx::query_as("SELECT credential_id FROM webauthn_credentials WHERE user_id = $1")
            .bind(ctx.user_id)
            .fetch_all(&state.pool)
            .await
            .map_err(|e| db_error(&e))?;
    let exclude = (!existing.is_empty())
        .then(|| existing.into_iter().map(|(id,)| id.into()).collect::<Vec<_>>());

    let (mut options, reg_state) = webauthn
        .start_passkey_registration(ctx.user_id, &name, &name, exclude)
        .map_err(|e| bad_request(format!("could not start registration: {e}")))?;
    // The login flow is usernameless, so the credential has to be discoverable.
    crate::webauthn::require_resident_key(&mut options);

    let stashed = serde_json::to_value(&reg_state)
        .map_err(|e| bad_request(format!("could not park challenge: {e}")))?;
    let challenge_id = stash_challenge(&state.pool, Some(ctx.user_id), "register", &stashed)
        .await
        .map_err(|e| db_error(&e))?;
    let options = serde_json::to_value(&options)
        .map_err(|e| bad_request(format!("could not serialize options: {e}")))?;
    Ok(Json(PasskeyChallenge { challenge_id, options }))
}

/// `POST /passkeys/register/finish` — verify and store the new credential.
pub async fn register_finish(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<PasskeyRegisterFinish>,
) -> ApiResult<Json<PasskeyRow>> {
    let webauthn = relying_party(&state)?;
    let label = normalize_label(req.label.as_deref())?;
    let parked =
        take_challenge(&state.pool, req.challenge_id, "register", Some(ctx.user_id)).await?;
    let reg_state: PasskeyRegistration =
        serde_json::from_value(parked).map_err(|e| bad_request(format!("stale challenge: {e}")))?;
    let credential: RegisterPublicKeyCredential = serde_json::from_value(req.credential)
        .map_err(|e| bad_request(format!("malformed credential: {e}")))?;

    let passkey = webauthn
        .finish_passkey_registration(&credential, &reg_state)
        .map_err(|e| bad_request(format!("registration rejected: {e}")))?;
    let stored = serde_json::to_value(&passkey)
        .map_err(|e| bad_request(format!("could not store credential: {e}")))?;

    let row = sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
        "INSERT INTO webauthn_credentials (user_id, credential_id, passkey, label, discoverable) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id, created_at",
    )
    .bind(ctx.user_id)
    .bind(passkey.cred_id().as_ref())
    .bind(&stored)
    .bind(&label)
    // Absent `credProps` means the browser didn't say; assume the key is
    // discoverable rather than warning about something that probably works.
    .bind(req.discoverable.unwrap_or(true))
    .fetch_one(&state.pool)
    .await
    .map_err(|e| db_error(&e))?;

    tracing::info!(user_id = %ctx.user_id, passkey_id = %row.0, "passkey enrolled");
    Ok(Json(PasskeyRow {
        id: row.0,
        label,
        discoverable: req.discoverable.unwrap_or(true),
        created_at: row.1,
        last_used_at: None,
    }))
}

// ---------------------------------------------------------------------------
// Management (authenticated)
// ---------------------------------------------------------------------------

/// `GET /passkeys` — the caller's own keys, newest first.
pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<Json<PasskeyListResponse>> {
    let rows = sqlx::query_as::<_, (Uuid, String, bool, DateTime<Utc>, Option<DateTime<Utc>>)>(
        "SELECT id, label, discoverable, created_at, last_used_at FROM webauthn_credentials \
         WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(ctx.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_error(&e))?;
    Ok(Json(PasskeyListResponse {
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
    }))
}

/// `PATCH /passkeys/{id}` — rename one of the caller's keys.
pub async fn relabel(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<RelabelPasskeyRequest>,
) -> ApiResult<StatusCode> {
    let label = normalize_label(Some(&req.label))?;
    let done =
        sqlx::query("UPDATE webauthn_credentials SET label = $1 WHERE id = $2 AND user_id = $3")
            .bind(&label)
            .bind(id)
            .bind(ctx.user_id)
            .execute(&state.pool)
            .await
            .map_err(|e| db_error(&e))?;
    if done.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "no such passkey".into() })));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /passkeys/{id}` — drop one of the caller's keys.
///
/// Removing the last one is allowed on purpose: the token login is the recovery
/// path and always works, so there is nothing to protect the user from here.
pub async fn revoke(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let done = sqlx::query("DELETE FROM webauthn_credentials WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(ctx.user_id)
        .execute(&state.pool)
        .await
        .map_err(|e| db_error(&e))?;
    if done.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "no such passkey".into() })));
    }
    tracing::info!(user_id = %ctx.user_id, passkey_id = %id, "passkey revoked");
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Assertion — shared by "test mine" and "log me in"
// ---------------------------------------------------------------------------

/// Start a discoverable (usernameless) assertion and park its state.
async fn start_assertion(state: &AppState) -> ApiResult<Json<PasskeyChallenge>> {
    let webauthn = relying_party(state)?;
    let (options, auth_state) = webauthn
        .start_discoverable_authentication()
        .map_err(|e| bad_request(format!("could not start authentication: {e}")))?;
    let stashed = serde_json::to_value(&auth_state)
        .map_err(|e| bad_request(format!("could not park challenge: {e}")))?;
    let challenge_id = stash_challenge(&state.pool, None, "authenticate", &stashed)
        .await
        .map_err(|e| db_error(&e))?;
    let options = serde_json::to_value(&options)
        .map_err(|e| bad_request(format!("could not serialize options: {e}")))?;
    Ok(Json(PasskeyChallenge { challenge_id, options }))
}

/// A verified assertion: who signed, and with which key.
struct Asserted {
    user_id: Uuid,
    passkey_id: Uuid,
    label: String,
}

/// Verify an assertion end to end: resolve the user handle the browser
/// returned, load that user's stored credential, check the signature, and roll
/// the signature counter forward.
async fn finish_assertion(state: &AppState, req: PasskeyAssertion) -> ApiResult<Asserted> {
    let webauthn = relying_party(state)?;
    let parked = take_challenge(&state.pool, req.challenge_id, "authenticate", None).await?;
    let auth_state: DiscoverableAuthentication =
        serde_json::from_value(parked).map_err(|e| bad_request(format!("stale challenge: {e}")))?;
    let credential: PublicKeyCredential = serde_json::from_value(req.credential)
        .map_err(|e| bad_request(format!("malformed assertion: {e}")))?;

    // The browser tells us which user handle it signed for; we then prove it by
    // verifying against the credential we stored for that exact user.
    let (user_id, cred_id) = webauthn
        .identify_discoverable_authentication(&credential)
        .map_err(|e| bad_request(format!("unusable assertion: {e}")))?;

    let row = sqlx::query_as::<_, (Uuid, String, Value)>(
        "SELECT c.id, c.label, c.passkey FROM webauthn_credentials c \
         JOIN users u ON u.id = c.user_id \
         WHERE c.user_id = $1 AND c.credential_id = $2 \
         AND u.revoked_at IS NULL AND u.disabled_at IS NULL",
    )
    .bind(user_id)
    .bind(cred_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| db_error(&e))?
    .ok_or_else(|| {
        (StatusCode::UNAUTHORIZED, Json(ApiError { error: "unknown passkey".into() }))
    })?;

    let (passkey_id, label, stored) = row;
    let mut passkey: Passkey = serde_json::from_value(stored)
        .map_err(|e| bad_request(format!("stored credential unreadable: {e}")))?;
    let result = webauthn
        .finish_discoverable_authentication(
            &credential,
            auth_state,
            &[DiscoverableKey::from(&passkey)],
        )
        .map_err(|e| {
            tracing::warn!(%user_id, "passkey assertion rejected: {e}");
            (StatusCode::UNAUTHORIZED, Json(ApiError { error: "assertion rejected".into() }))
        })?;

    // Persist the rolled counter / backup flags so a cloned authenticator is
    // caught on its next use rather than never.
    if passkey.update_credential(&result).is_some_and(|updated| updated)
        && let Ok(refreshed) = serde_json::to_value(&passkey)
    {
        let _ = sqlx::query("UPDATE webauthn_credentials SET passkey = $1 WHERE id = $2")
            .bind(refreshed)
            .bind(passkey_id)
            .execute(&state.pool)
            .await;
    }
    let _ = sqlx::query("UPDATE webauthn_credentials SET last_used_at = now() WHERE id = $1")
        .bind(passkey_id)
        .execute(&state.pool)
        .await;

    Ok(Asserted { user_id, passkey_id, label })
}

// ---------------------------------------------------------------------------
// Test the key you just enrolled (authenticated)
// ---------------------------------------------------------------------------

/// `POST /passkeys/test/start` — same ceremony as login, run while signed in.
pub async fn test_start(State(state): State<AppState>) -> ApiResult<Json<PasskeyChallenge>> {
    start_assertion(&state).await
}

/// `POST /passkeys/test/finish` — verify, and insist the key is the caller's
/// own. Mints nothing: this answers "does it work", not "let me in".
pub async fn test_finish(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<PasskeyAssertion>,
) -> ApiResult<Json<PasskeyTestResult>> {
    let asserted = finish_assertion(&state, req).await?;
    if asserted.user_id != ctx.user_id {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError { error: "that passkey belongs to another account".into() }),
        ));
    }
    tracing::info!(user_id = %ctx.user_id, passkey_id = %asserted.passkey_id, "passkey tested");
    Ok(Json(PasskeyTestResult { label: asserted.label }))
}

// ---------------------------------------------------------------------------
// Login (unauthenticated)
// ---------------------------------------------------------------------------

/// `GET /auth/passkey/config` — what the login screen needs to know before
/// anyone has authenticated: can this server do passkeys, has anyone enrolled
/// one, and should the read start on its own.
pub async fn config(State(state): State<AppState>) -> Json<PasskeyConfig> {
    let available = state.webauthn.is_some();
    let enrolled = available
        && sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM webauthn_credentials)")
            .fetch_one(&state.pool)
            .await
            .unwrap_or(false);
    Json(PasskeyConfig { available, enrolled, auto_prompt: read_auto_prompt(&state.pool).await })
}

/// `POST /auth/passkey/login/start` — options for `navigator.credentials.get()`.
pub async fn login_start(State(state): State<AppState>) -> ApiResult<Json<PasskeyChallenge>> {
    start_assertion(&state).await
}

/// `POST /auth/passkey/login/finish` — verify the assertion, mint a session key
/// for the user it resolved to, and set the auth cookie.
pub async fn login_finish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PasskeyAssertion>,
) -> Result<Response, (StatusCode, Json<ApiError>)> {
    let asserted = finish_assertion(&state, req).await?;

    // Same credential shape as any other user token, so nothing downstream has
    // to learn a new kind of session.
    let token = auth::user_token(&auth::mint_secret());
    let hash = auth::sha256_hex(&token);
    let preview = auth::token_preview(&token);
    let scopes = auth::ceiling_of(&state.pool, asserted.user_id).await;
    let label = format!("passkey: {}", asserted.label);
    let key_id = auth::register_key(
        &state.pool,
        auth::NewKey {
            user_id: asserted.user_id,
            key_hash: &hash,
            key_preview: Some(&preview),
            label: Some(&label),
            kind: SESSION_KIND,
            machine_id: None,
            dispatcher_id: None,
        },
        scopes,
    )
    .await
    .map_err(|e| db_error(&e))?;

    let expires = Utc::now() + Duration::days(SESSION_DAYS);
    sqlx::query("UPDATE auth_keys SET expires_at = $1 WHERE id = $2")
        .bind(expires)
        .bind(key_id)
        .execute(&state.pool)
        .await
        .map_err(|e| db_error(&e))?;

    // Expired session keys are dead rows; drop the ones long past their date so
    // the admin key list stays about keys a human made.
    let _ = sqlx::query(
        "DELETE FROM auth_keys WHERE kind = $1 AND expires_at < now() - interval '7 days'",
    )
    .bind(SESSION_KIND)
    .execute(&state.pool)
    .await;

    tracing::info!(user_id = %asserted.user_id, %key_id, "passkey login");
    let cookie = auth::set_auth_cookie(&token, auth::request_is_https(&headers));
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response())
}

// ---------------------------------------------------------------------------
// Server-wide policy (admin)
// ---------------------------------------------------------------------------

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
pub async fn set_auto_prompt(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<PasskeyAutoPromptRequest>,
) -> ApiResult<StatusCode> {
    ctx.requires(Scope::Admin)
        .map_err(|s| (s, Json(ApiError { error: "admin token required".into() })))?;
    sqlx::query(
        "INSERT INTO instance_settings (key, value, updated_at) \
         VALUES ($1, to_jsonb($2::bool), now()) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(AUTO_PROMPT_KEY)
    .bind(req.auto_prompt)
    .execute(&state.pool)
    .await
    .map_err(|e| db_error(&e))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Revoke the `auth_keys` row a passkey login minted, if that is what the
/// presented token is. Called from logout so a passkey session leaves nothing
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
        assert_eq!(normalize_label(Some(&long)).unwrap_err().0, StatusCode::BAD_REQUEST);
        assert!(normalize_label(Some(&"x".repeat(LABEL_MAX_CHARS))).is_ok());
    }
}
