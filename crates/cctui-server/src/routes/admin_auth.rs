//! Admin-only CRUD for users and machines.
//!
//! All handlers require `TokenRole::Admin` (bootstrap env token).
//! Keys are returned in plaintext exactly once — on create or rotate —
//! and stored only as `sha256(token)`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::auth::{AuthContext, Scope, machine_token, mint_secret, sha256_hex, user_token};
use crate::error::AppError;
use crate::state::AppState;

pub use cctui_proto::api::admin::{
    ApiKeyRow, CreateUserRequest, CreateUserResponse, MachineRow, MintKeyRequest, MintKeyResponse,
    RelabelTokenRequest, RenameMachineRequest, RotateResponse, SetAclsRequest, UpdateUserRequest,
    UserAclsResponse, UserRow, UserTokenRow,
};

fn forbid_or(ctx: &AuthContext) -> Result<(), AppError> {
    ctx.requires(Scope::Admin).map_err(|s| AppError::new(s, "admin token required"))
}

pub async fn create_user(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateUserRequest>,
) -> Result<Json<CreateUserResponse>, AppError> {
    forbid_or(&ctx)?;
    if req.name.trim().is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "name required"));
    }
    let id = Uuid::new_v4();
    let secret = mint_secret();
    let token = user_token(&secret);
    let hash = sha256_hex(&token);
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(&req.name)
        .bind(&hash)
        .execute(&state.pool)
        .await?;
    // Seed the new user's ceiling: the default capability set a fresh
    // user gets — read + enroll + dispatch (NOT admin). Matches the legacy
    // default where can_dispatch=TRUE and any user token could enroll/dispatch.
    let default_ceiling = [Scope::Read, Scope::Enroll, Scope::Dispatch];
    for scope in default_ceiling {
        let _ = sqlx::query(
            "INSERT INTO user_acls (user_id, scope) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(scope.as_str())
        .execute(&state.pool)
        .await;
    }
    // Register the primary key in the unified table with grant = ceiling.
    let preview = crate::auth::token_preview(&token);
    if let Err(e) = crate::auth::register_key(
        &state.pool,
        crate::auth::NewKey {
            user_id: id,
            key_hash: &hash,
            key_preview: Some(&preview),
            label: Some("primary"),
            kind: "user",
            machine_id: None,
            dispatcher_id: None,
            expires_at: None,
            passkey_id: None,
        },
        default_ceiling,
    )
    .await
    {
        tracing::warn!("failed to register primary key in auth_keys: {e}");
    }
    tracing::info!(user_id = %id, name = %req.name, "user created");
    Ok(Json(CreateUserResponse { id, name: req.name, key: token }))
}

pub async fn list_users(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<UserRow>>, AppError> {
    forbid_or(&ctx)?;
    let rows: Vec<UserRow> = sqlx::query_as(
        "SELECT u.id, u.name, u.created_at, u.revoked_at, u.disabled_at, u.can_dispatch, \
         (SELECT max(k.last_used_at) FROM auth_keys k WHERE k.user_id = u.id) AS last_seen_at \
         FROM users u ORDER BY u.created_at",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

pub async fn revoke_user(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    forbid_or(&ctx)?;
    let res =
        sqlx::query("UPDATE users SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
            .bind(id)
            .execute(&state.pool)
            .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "user not found"));
    }
    crate::store::tokens::revoke_by_user(&state.pool, id).await?;
    state.auth_config.purge_all();
    tracing::info!(user_id = %id, "user revoked");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rotate_user(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<RotateResponse>, AppError> {
    forbid_or(&ctx)?;
    let token = rotate_primary_key(&state.pool, id).await?;
    state.auth_config.purge_all();
    tracing::info!(user_id = %id, "user key rotated");
    Ok(Json(RotateResponse { id, key: token }))
}

/// Replace a user's primary credential: retire the old secret everywhere auth
/// would accept it, then mint the replacement with the owner's current ceiling
/// as its grant. One transaction, so the user is never left with two live keys
/// or none. Returns the new plaintext token.
async fn rotate_primary_key(pool: &sqlx::PgPool, id: Uuid) -> Result<String, AppError> {
    let old_hash: Option<(String,)> =
        sqlx::query_as("SELECT key_hash FROM users WHERE id = $1 AND revoked_at IS NULL")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let Some((old_hash,)) = old_hash else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "user not found"));
    };
    let secret = mint_secret();
    let token = user_token(&secret);
    let hash = sha256_hex(&token);
    let preview = crate::auth::token_preview(&token);
    let ceiling = crate::store::acls::user_ceiling(pool, id).await?;

    let mut tx = pool.begin().await?;
    // Auth resolves `auth_keys` before `users.key_hash`: without revoking the
    // mirror the old secret keeps working and the new one only resolves through
    // the legacy fallback.
    sqlx::query(
        "UPDATE auth_keys SET revoked_at = now() WHERE key_hash = $1 AND revoked_at IS NULL",
    )
    .bind(&old_hash)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE users SET key_hash = $1 WHERE id = $2")
        .bind(&hash)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    crate::auth::register_key(
        &mut *tx,
        crate::auth::NewKey {
            user_id: id,
            key_hash: &hash,
            key_preview: Some(&preview),
            label: Some("primary"),
            kind: "user",
            machine_id: None,
            dispatcher_id: None,
            expires_at: None,
            passkey_id: None,
        },
        ceiling,
    )
    .await?;
    tx.commit().await?;
    Ok(token)
}

pub async fn list_user_machines(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<Vec<MachineRow>>, AppError> {
    forbid_or(&ctx)?;
    let mut rows: Vec<MachineRow> = sqlx::query_as(
        "SELECT id, user_id, name, display_name, first_seen_at, last_seen_at, revoked_at, kind, \
                hue, key_preview \
         FROM machines WHERE user_id = $1 AND deleted_at IS NULL ORDER BY first_seen_at",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;
    // Derive the online/stale/offline tier from `last_seen_at` age so
    // the UI can render a machine health dot without re-implementing the
    // thresholds client-side.
    for row in &mut rows {
        row.liveness = crate::machine_liveness::derive(row.last_seen_at);
    }
    Ok(Json(rows))
}

/// Soft-delete a machine row. Only allowed once the machine is already
/// revoked — we preserve the row itself so historical FK references
/// (sessions, archive entries) don't break, but hide it from the admin UI.
pub async fn delete_machine(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    forbid_or(&ctx)?;
    let res = sqlx::query(
        "UPDATE machines SET deleted_at = now() \
         WHERE id = $1 AND revoked_at IS NOT NULL AND deleted_at IS NULL",
    )
    .bind(id)
    .execute(&state.pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::CONFLICT, "machine must be revoked before delete"));
    }
    tracing::info!(machine_id = %id, "machine deleted (soft)");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn revoke_machine(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    forbid_or(&ctx)?;
    let old_hash: Option<(String,)> =
        sqlx::query_as("SELECT key_hash FROM machines WHERE id = $1 AND revoked_at IS NULL")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let Some((old_hash,)) = old_hash else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "machine not found"));
    };
    sqlx::query("UPDATE machines SET revoked_at = now() WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;
    // Auth resolves against auth_keys first; revoke the mirror row too or the
    // key keeps authenticating.
    sqlx::query(
        "UPDATE auth_keys SET revoked_at = now() WHERE machine_id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(&state.pool)
    .await?;
    state.auth_config.purge(&old_hash);
    tracing::info!(machine_id = %id, "machine revoked");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rename_machine(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<RenameMachineRequest>,
) -> Result<StatusCode, AppError> {
    forbid_or(&ctx)?;
    let trimmed = req.display_name.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(h) = req.hue
        && !(0..360).contains(&h)
    {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "hue must be in 0..360"));
    }
    let outcome = sqlx::query("UPDATE machines SET display_name = $1, hue = $2 WHERE id = $3")
        .bind(&trimmed)
        .bind(req.hue)
        .bind(id)
        .execute(&state.pool)
        .await?;
    if outcome.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "machine not found"));
    }
    tracing::info!(machine_id = %id, display_name = ?trimmed, "machine renamed");
    Ok(StatusCode::NO_CONTENT)
}

/// Update a user's mutable fields (rename + dispatch toggle).
/// `name` is `NOT NULL`, so a blank name is rejected rather than cleared;
/// `can_dispatch` flips the per-user dispatch permission. Fields left `None`
/// are untouched, so the UI can PATCH just the field it changed.
pub async fn update_user(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<StatusCode, AppError> {
    forbid_or(&ctx)?;
    let name = match req.name.as_deref().map(str::trim) {
        Some("") => {
            return Err(AppError::new(StatusCode::BAD_REQUEST, "name required"));
        }
        other => other,
    };
    if name.is_none() && req.can_dispatch.is_none() && req.disabled.is_none() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "nothing to update"));
    }
    // COALESCE keeps the existing value when a field is NULL (not supplied).
    // `disabled` maps to the timestamp: true → now() (kept if already set so
    // the original disable time survives repeats), false → NULL.
    let outcome = sqlx::query(
        "UPDATE users SET \
            name = COALESCE($1, name), \
            can_dispatch = COALESCE($2, can_dispatch), \
            disabled_at = CASE \
                WHEN $3::bool IS NULL THEN disabled_at \
                WHEN $3 THEN COALESCE(disabled_at, now()) \
                ELSE NULL END \
         WHERE id = $4",
    )
    .bind(name)
    .bind(req.can_dispatch)
    .bind(req.disabled)
    .bind(id)
    .execute(&state.pool)
    .await?;
    if outcome.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "user not found"));
    }
    // Disabling must take effect immediately, not after the auth-cache TTL.
    if req.disabled == Some(true) {
        state.auth_config.purge_all();
    }
    tracing::info!(user_id = %id, name, can_dispatch = ?req.can_dispatch, disabled = ?req.disabled, "user updated");
    Ok(StatusCode::NO_CONTENT)
}

/// Permanently delete a revoked user and everything owned by it.
/// Mirrors `delete_machine`'s "must be revoked first" guard so a live user is
/// never destroyed by a mis-click. `machines`, `user_tokens`, `triggers` and
/// uploaded skills cascade on the FK; `sessions` reference the user/machine
/// WITHOUT cascade, so we null those references first (history is preserved,
/// just disowned). All in one transaction so a partial failure rolls back.
pub async fn purge_user(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    forbid_or(&ctx)?;
    let mut tx = state.pool.begin().await?;
    let revoked: Option<(Option<DateTime<Utc>>,)> =
        sqlx::query_as("SELECT revoked_at FROM users WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((revoked_at,)) = revoked else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "user not found"));
    };
    if revoked_at.is_none() {
        return Err(AppError::new(StatusCode::CONFLICT, "user must be revoked before delete"));
    }
    // Disown sessions that reference this user or any of its machines (no FK
    // cascade there — preserve the transcript rows, just drop ownership).
    sqlx::query(
        "UPDATE sessions SET machine_uuid = NULL \
         WHERE machine_uuid IN (SELECT id FROM machines WHERE user_id = $1)",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE sessions SET user_id = NULL WHERE user_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let outcome = sqlx::query("DELETE FROM users WHERE id = $1").bind(id).execute(&mut *tx).await?;
    if outcome.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "user not found"));
    }
    tx.commit().await?;
    state.auth_config.purge_all();
    tracing::info!(user_id = %id, "user purged");
    Ok(StatusCode::NO_CONTENT)
}

/// List a user's tokens. Token secrets are never recoverable —
/// this returns only metadata so the UI can relabel/revoke them.
pub async fn list_user_tokens(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<UserTokenRow>>, AppError> {
    forbid_or(&ctx)?;
    let rows: Vec<UserTokenRow> = sqlx::query_as(
        "SELECT id, label, created_at, expires_at, revoked_at, token_preview \
         FROM user_tokens WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

/// Relabel a token. `None`/blank clears the label.
pub async fn relabel_user_token(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((user_id, token_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<RelabelTokenRequest>,
) -> Result<StatusCode, AppError> {
    forbid_or(&ctx)?;
    let trimmed = req.label.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let outcome = sqlx::query("UPDATE user_tokens SET label = $1 WHERE id = $2 AND user_id = $3")
        .bind(&trimmed)
        .bind(token_id)
        .bind(user_id)
        .execute(&state.pool)
        .await?;
    if outcome.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "token not found"));
    }
    tracing::info!(%user_id, %token_id, label = ?trimmed, "token relabeled");
    Ok(StatusCode::NO_CONTENT)
}

/// Look up the secret behind a `user_tokens` row. Revocation keys off the hash,
/// not the row id: the same secret also has an `auth_keys` row under a different
/// id, and auth resolves that one first.
async fn user_token_hash(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    token_id: Uuid,
) -> Result<String, AppError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT token_hash FROM user_tokens WHERE id = $1 AND user_id = $2")
            .bind(token_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    row.map(|(h,)| h).ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "token not found"))
}

/// Revoke a single token. Mirrors `revoke_user`; purges the
/// auth cache so the token stops working immediately. Idempotent: re-revoking an
/// already-revoked token is a no-op 204, since the point is that every row the
/// secret resolves through ends up retired.
pub async fn revoke_user_token(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((user_id, token_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    forbid_or(&ctx)?;
    let hash = user_token_hash(&state.pool, user_id, token_id).await?;
    crate::routes::me::revoke_by_hash(&state.pool, &hash).await?;
    state.auth_config.purge_all();
    tracing::info!(%user_id, %token_id, "token revoked");
    Ok(StatusCode::NO_CONTENT)
}

/// Hard-delete a token row — "revoke + purge" in one go. Unlike
/// `revoke_user_token` (which keeps the row around showing `revoked`), this
/// removes it entirely. A token is pure auth surface with no historical FK, so
/// deleting the row is safe and equivalent to revoking from a security view.
/// The auth cache is purged so the secret stops working immediately.
pub async fn delete_user_token(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((user_id, token_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    forbid_or(&ctx)?;
    let hash = user_token_hash(&state.pool, user_id, token_id).await?;
    delete_token_rows(&state.pool, user_id, token_id, &hash).await?;
    state.auth_config.purge_all();
    tracing::info!(%user_id, %token_id, "token purged");
    Ok(StatusCode::NO_CONTENT)
}

/// Both rows the secret resolves through, in one transaction. Nothing but
/// `key_acls` (ON DELETE CASCADE) references `auth_keys`, so the mirror can go
/// rather than linger as a live credential.
async fn delete_token_rows(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    token_id: Uuid,
    hash: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM user_tokens WHERE id = $1 AND user_id = $2")
        .bind(token_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM auth_keys WHERE key_hash = $1 AND user_id = $2")
        .bind(hash)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

pub async fn rotate_machine(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<RotateResponse>, AppError> {
    forbid_or(&ctx)?;
    let old_hash: Option<(String,)> =
        sqlx::query_as("SELECT key_hash FROM machines WHERE id = $1 AND revoked_at IS NULL")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let Some((old_hash,)) = old_hash else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "machine not found"));
    };
    let secret = mint_secret();
    let token = machine_token(&secret);
    let hash = sha256_hex(&token);
    let preview = crate::auth::token_preview(&token);
    sqlx::query("UPDATE machines SET key_hash = $1, key_preview = $2 WHERE id = $3")
        .bind(&hash)
        .bind(&preview)
        .bind(id)
        .execute(&state.pool)
        .await?;
    // Auth resolves against auth_keys first; without this the replaced key keeps
    // authenticating and the new one only works via the legacy dual-read.
    let mirrored = sqlx::query(
        "UPDATE auth_keys SET key_hash = $1, key_preview = $2 \
         WHERE machine_id = $3 AND revoked_at IS NULL",
    )
    .bind(&hash)
    .bind(&preview)
    .bind(id)
    .execute(&state.pool)
    .await?;
    if mirrored.rows_affected() == 0 {
        let owner: Option<(Uuid, String)> =
            sqlx::query_as("SELECT user_id, name FROM machines WHERE id = $1")
                .bind(id)
                .fetch_optional(&state.pool)
                .await?;
        if let Some((user_id, name)) = owner {
            let grant = crate::store::acls::user_ceiling(&state.pool, user_id).await?;
            if let Err(e) = crate::auth::register_key(
                &state.pool,
                crate::auth::NewKey {
                    user_id,
                    key_hash: &hash,
                    key_preview: Some(&preview),
                    label: Some(&name),
                    kind: "machine",
                    machine_id: Some(id),
                    dispatcher_id: None,
                    expires_at: None,
                    passkey_id: None,
                },
                grant,
            )
            .await
            {
                tracing::warn!(machine_id = %id, "failed to register rotated machine key in auth_keys: {e}");
            }
        }
    }
    state.auth_config.purge(&old_hash);
    tracing::info!(machine_id = %id, "machine key rotated");
    Ok(Json(RotateResponse { id, key: token }))
}

// ===========================================================================
// per-user scope (ceiling) + per-key (grant) management for the
// Users page. Cross-user actions require the `admin` scope; a user may always
// manage its OWN ceiling (read-only) and its OWN keys (mint/revoke/edit scopes).
// Edits are plain INSERT/DELETE on the acl tables, constrained key ⊆ user, and
// the auth cache is purged so a change takes effect immediately for live keys.
// ===========================================================================

/// Allow if the caller is admin, or is acting on its own account.
fn self_or_admin(ctx: &AuthContext, target: Uuid) -> Result<(), AppError> {
    if ctx.is_admin() || ctx.user_id == target {
        Ok(())
    } else {
        Err(AppError::new(StatusCode::FORBIDDEN, "admin scope required"))
    }
}

/// `GET /users/{id}/acls` — the user's ceiling. Self or admin.
pub async fn get_user_acls(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<UserAclsResponse>, AppError> {
    self_or_admin(&ctx, user_id)?;
    let scopes = crate::store::acls::user_ceiling(&state.pool, user_id).await?;
    Ok(Json(UserAclsResponse { user_id, scopes: scopes.iter().map(ToString::to_string).collect() }))
}

fn parse_scopes(raw: &[String]) -> Result<Vec<Scope>, AppError> {
    raw.iter()
        .map(|s| {
            Scope::parse(s).ok_or_else(|| {
                AppError::new(StatusCode::BAD_REQUEST, format!("unknown scope: {s}"))
            })
        })
        .collect()
}

/// `PATCH /users/{id}/acls` — replace the user's ceiling. Admin only (granting a
/// user new capabilities is privileged). Setting the ceiling re-intersects all
/// of the user's keys at the next request (the drift-killer), so demotion is
/// immediate; the cache is purged to skip the TTL.
pub async fn set_user_acls(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<SetAclsRequest>,
) -> Result<StatusCode, AppError> {
    forbid_or(&ctx)?; // admin only
    let scopes = parse_scopes(&req.scopes)?;
    let mut tx = state.pool.begin().await?;
    crate::store::acls::set_user_ceiling(&mut tx, user_id, &scopes).await?;
    // Keep the legacy can_dispatch flag in sync: the dispatch scope
    // supersedes it, but other code paths / older clients may still read it.
    let can_dispatch = scopes.contains(&Scope::Dispatch);
    sqlx::query("UPDATE users SET can_dispatch = $1 WHERE id = $2")
        .bind(can_dispatch)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    // A ceiling change affects every key the user owns; we don't track which
    // hashes are cached, so drop the whole cache (short TTL, repopulates fast).
    state.auth_config.purge_all();
    tracing::info!(%user_id, ?scopes, "user ceiling updated");
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /users/{id}/keys` — the user's `auth_keys` with their granted scopes. Self
/// or admin.
pub async fn list_user_keys(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<Vec<ApiKeyRow>>, AppError> {
    self_or_admin(&ctx, user_id)?;
    let mut rows: Vec<ApiKeyRow> = sqlx::query_as(
        "SELECT id, label, key_preview, kind, created_at, expires_at, revoked_at, last_used_at \
         FROM auth_keys WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;
    for row in &mut rows {
        let scopes: Vec<(String,)> = sqlx::query_as("SELECT scope FROM key_acls WHERE key_id = $1")
            .bind(row.id)
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default();
        row.scopes = scopes.into_iter().map(|(s,)| s).collect();
    }
    Ok(Json(rows))
}

/// `POST /users/{id}/keys` — mint a scoped key for the user. Self or admin. The
/// grant is intersected with the owner's ceiling (`key ⊆ user`, the drift
/// rule): requesting a scope the user doesn't hold is silently dropped.
pub async fn mint_user_key(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<MintKeyRequest>,
) -> Result<Json<MintKeyResponse>, AppError> {
    self_or_admin(&ctx, user_id)?;
    if !crate::auth::is_human_credential(&state.pool, &ctx).await? {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "only a user credential can mint user keys",
        ));
    }
    let requested = parse_scopes(&req.scopes)?;
    let ceiling = crate::store::acls::user_ceiling(&state.pool, user_id).await?;
    let granted: Vec<Scope> =
        requested.into_iter().filter(|s| ceiling.contains(s) && ctx.scopes.contains(s)).collect();

    let token = user_token(&mint_secret());
    let hash = sha256_hex(&token);
    let preview = crate::auth::token_preview(&token);

    // One transaction: a `user_tokens` row without its `auth_keys` mirror is a
    // credential only the legacy fallback can see, which a revoke would miss.
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO user_tokens (user_id, token_hash, label, expires_at, token_preview) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(&hash)
    .bind(req.label.as_deref())
    .bind(req.expires_at)
    .bind(&preview)
    .execute(&mut *tx)
    .await?;

    let key_id = crate::auth::register_key(
        &mut *tx,
        crate::auth::NewKey {
            user_id,
            key_hash: &hash,
            key_preview: Some(&preview),
            label: req.label.as_deref(),
            kind: "user",
            machine_id: None,
            dispatcher_id: None,
            expires_at: req.expires_at,
            passkey_id: None,
        },
        granted.clone(),
    )
    .await?;
    tx.commit().await?;
    tracing::info!(%user_id, %key_id, ?granted, "key minted");
    Ok(Json(MintKeyResponse {
        id: key_id,
        key: token,
        scopes: granted.iter().map(ToString::to_string).collect(),
    }))
}

/// `PATCH /users/{id}/keys/{kid}/acls` — edit a key's granted scopes IN PLACE
/// (the secret/hash is untouched, so the token keeps working). Self or admin.
/// Constrained `key ⊆ user` at edit time; cache purged so it takes effect now.
pub async fn set_key_acls(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((user_id, key_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<SetAclsRequest>,
) -> Result<StatusCode, AppError> {
    self_or_admin(&ctx, user_id)?;
    // The key must belong to the named user.
    let owns: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM auth_keys WHERE id = $1 AND user_id = $2")
            .bind(key_id)
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await?;
    if owns.is_none() {
        return Err(AppError::new(StatusCode::NOT_FOUND, "key not found"));
    }
    let requested = parse_scopes(&req.scopes)?;
    let ceiling = crate::store::acls::user_ceiling(&state.pool, user_id).await?;
    let granted: Vec<Scope> = requested.into_iter().filter(|s| ceiling.contains(s)).collect();

    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM key_acls WHERE key_id = $1").bind(key_id).execute(&mut *tx).await?;
    crate::store::acls::grant_key(&mut *tx, key_id, granted.iter().copied()).await?;
    tx.commit().await?;
    state.auth_config.purge_all();
    tracing::info!(%user_id, %key_id, ?granted, "key scopes edited");
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /users/{id}/keys/{kid}` — revoke a key (sets `revoked_at`; cascades
/// drop its `key_acls` on hard-delete, but revoke preserves the audit row). Self
/// or admin. Also revokes the legacy mirror rows by hash so the dual-read path
/// stops accepting it. Cache purged so it stops working immediately.
#[allow(clippy::cognitive_complexity)]
pub async fn revoke_user_key(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((user_id, key_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    self_or_admin(&ctx, user_id)?;
    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE auth_keys SET revoked_at = now() \
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL RETURNING key_hash",
    )
    .bind(key_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((hash,)) = row else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "key not found"));
    };
    // Revoke the legacy mirrors sharing this hash (transparency cutover).
    if let Err(e) = sqlx::query("UPDATE user_tokens SET revoked_at = now() WHERE token_hash = $1")
        .bind(&hash)
        .execute(&state.pool)
        .await
    {
        tracing::error!(
            %user_id,
            %key_id,
            error = %e,
            "failed to revoke legacy user_tokens mirror — the key may still authenticate via the dual-read path"
        );
    }
    if let Err(e) = sqlx::query("UPDATE machines SET revoked_at = now() WHERE key_hash = $1")
        .bind(&hash)
        .execute(&state.pool)
        .await
    {
        tracing::error!(
            %user_id,
            %key_id,
            error = %e,
            "failed to revoke legacy machines mirror — the key may still authenticate via the dual-read path"
        );
    }
    state.auth_config.purge(&hash);
    tracing::info!(%user_id, %key_id, "key revoked");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn ctx(user_id: Uuid, scopes: &[Scope]) -> AuthContext {
        AuthContext {
            user_id,
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: scopes.iter().copied().collect::<BTreeSet<_>>(),
        }
    }

    #[test]
    fn forbid_or_gates_on_admin_scope() {
        assert!(forbid_or(&ctx(Uuid::new_v4(), &[Scope::Admin])).is_ok());
        let Err(AppError::Status(status, _)) = forbid_or(&ctx(Uuid::new_v4(), &[Scope::Read]))
        else {
            panic!("expected a status error");
        };
        assert_eq!(status, StatusCode::FORBIDDEN);
        let Err(AppError::Status(status, _)) = forbid_or(&ctx(Uuid::new_v4(), &[])) else {
            panic!("expected a status error");
        };
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn self_or_admin_allows_owner_and_admin_but_denies_other() {
        let me = Uuid::new_v4();
        let other = Uuid::new_v4();

        assert!(self_or_admin(&ctx(me, &[Scope::Read]), me).is_ok(), "acting on own account");
        assert!(self_or_admin(&ctx(me, &[Scope::Admin]), other).is_ok(), "admin acts on anyone");

        let Err(AppError::Status(status, _)) = self_or_admin(&ctx(me, &[Scope::Read]), other)
        else {
            panic!("expected a status error");
        };
        assert_eq!(status, StatusCode::FORBIDDEN, "non-admin cannot touch another account");
    }

    #[test]
    fn parse_scopes_accepts_known_and_rejects_unknown() {
        let ok = parse_scopes(&["read".into(), "admin".into(), "dispatch".into()]).unwrap();
        assert_eq!(ok, vec![Scope::Read, Scope::Admin, Scope::Dispatch]);

        assert!(parse_scopes(&[]).unwrap().is_empty());

        let Err(AppError::Status(status, _)) = parse_scopes(&["read".into(), "root".into()]) else {
            panic!("expected a status error");
        };
        assert_eq!(status, StatusCode::BAD_REQUEST, "an unknown scope is rejected");
    }

    // ---- revocation against a real database ----
    //
    // Auth resolves `auth_keys` BEFORE the legacy `user_tokens` /
    // `users.key_hash` rows, so a revoke that touches only the legacy row leaves
    // the credential working. These drive the handlers' DB cores and assert
    // through `AuthConfig::validate`, which is the exact call whose `None` the
    // middleware turns into a 401.

    async fn test_pool(name: &str) -> Option<sqlx::PgPool> {
        let url = crate::routes::gateway::test_db_url(name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    async fn seed_user(pool: &sqlx::PgPool, key_hash: &str) -> Uuid {
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind(format!("revocation-test-{user_id}"))
            .bind(key_hash)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO user_acls (user_id, scope) VALUES ($1, 'read')")
            .bind(user_id)
            .execute(pool)
            .await
            .unwrap();
        user_id
    }

    /// The shape `mint_user_key` / `mint_user_token` produce: a `user_tokens`
    /// row and an `auth_keys` row for one secret, each with its own id.
    async fn mint_dual_written_token(pool: &sqlx::PgPool, user_id: Uuid) -> (String, Uuid) {
        let token = user_token(&mint_secret());
        let hash = sha256_hex(&token);
        let token_id: (Uuid,) = sqlx::query_as(
            "INSERT INTO user_tokens (user_id, token_hash, label, token_preview) \
             VALUES ($1, $2, 'test', 'prev') RETURNING id",
        )
        .bind(user_id)
        .bind(&hash)
        .fetch_one(pool)
        .await
        .unwrap();
        crate::auth::register_key(
            pool,
            crate::auth::NewKey {
                user_id,
                key_hash: &hash,
                key_preview: Some("prev"),
                label: Some("test"),
                kind: "user",
                machine_id: None,
                dispatcher_id: None,
                expires_at: None,
                passkey_id: None,
            },
            std::iter::once(Scope::Read),
        )
        .await
        .unwrap();
        (token, token_id.0)
    }

    /// A fresh config, so the answer comes from the database and not the cache.
    async fn resolves(pool: &sqlx::PgPool, token: &str) -> bool {
        crate::auth::AuthConfig::new(vec![], pool.clone()).validate(token).await.is_some()
    }

    #[tokio::test]
    async fn a_revoked_user_token_stops_authenticating() {
        let name = "a_revoked_user_token_stops_authenticating";
        let Some(pool) = test_pool(name).await else { return };
        let user_id = seed_user(&pool, &format!("not-a-sha-{}", Uuid::new_v4())).await;
        let (token, token_id) = mint_dual_written_token(&pool, user_id).await;
        assert!(resolves(&pool, &token).await, "the token works before revoke");

        let hash = user_token_hash(&pool, user_id, token_id).await.unwrap();
        crate::routes::me::revoke_by_hash(&pool, &hash).await.unwrap();

        assert!(!resolves(&pool, &token).await, "a revoked token must not authenticate");
        let live: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar("SELECT revoked_at FROM auth_keys WHERE key_hash = $1")
                .bind(&hash)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(live.is_some(), "the auth_keys row auth reads first is revoked too");
    }

    #[tokio::test]
    async fn a_deleted_user_token_stops_authenticating() {
        let name = "a_deleted_user_token_stops_authenticating";
        let Some(pool) = test_pool(name).await else { return };
        let user_id = seed_user(&pool, &format!("not-a-sha-{}", Uuid::new_v4())).await;
        let (token, token_id) = mint_dual_written_token(&pool, user_id).await;
        assert!(resolves(&pool, &token).await, "the token works before delete");

        let hash = user_token_hash(&pool, user_id, token_id).await.unwrap();
        delete_token_rows(&pool, user_id, token_id, &hash).await.unwrap();

        assert!(!resolves(&pool, &token).await, "a deleted token must not authenticate");
        let remaining: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM user_tokens WHERE token_hash = $1) \
                  + (SELECT count(*) FROM auth_keys WHERE key_hash = $1)",
        )
        .bind(&hash)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0, "neither row survives the purge");

        let again = user_token_hash(&pool, user_id, token_id).await;
        assert_eq!(
            again.err().map(|e| e.status()),
            Some(StatusCode::NOT_FOUND),
            "a second delete is a 404, not a 500"
        );
    }

    #[tokio::test]
    async fn rotating_a_user_key_retires_the_old_secret() {
        let name = "rotating_a_user_key_retires_the_old_secret";
        let Some(pool) = test_pool(name).await else { return };
        // The pre-rotation state `create_user` leaves behind: the primary secret
        // in `users.key_hash` AND in an `auth_keys` row.
        let old = user_token(&mint_secret());
        let old_hash = sha256_hex(&old);
        let user_id = seed_user(&pool, &old_hash).await;
        crate::auth::register_key(
            &pool,
            crate::auth::NewKey {
                user_id,
                key_hash: &old_hash,
                key_preview: Some("prev"),
                label: Some("primary"),
                kind: "user",
                machine_id: None,
                dispatcher_id: None,
                expires_at: None,
                passkey_id: None,
            },
            std::iter::once(Scope::Read),
        )
        .await
        .unwrap();
        assert!(resolves(&pool, &old).await, "the old key works before rotation");

        let new = rotate_primary_key(&pool, user_id).await.unwrap();

        assert!(!resolves(&pool, &old).await, "the rotated-out key must not authenticate");
        let ctx = crate::auth::AuthConfig::new(vec![], pool.clone())
            .validate(&new)
            .await
            .expect("the replacement key authenticates");
        assert_eq!(ctx.user_id, user_id);
        assert!(ctx.has(Scope::Read), "the replacement carries the owner's ceiling");
        assert!(!ctx.has(Scope::Admin));
    }
}
