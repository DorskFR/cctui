//! Admin-only CRUD for users and machines.
//!
//! All handlers require `TokenRole::Admin` (bootstrap env token).
//! Keys are returned in plaintext exactly once — on create or rotate —
//! and stored only as `sha256(token)`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use cctui_proto::api::ApiError;

use crate::auth::{
    AuthConfig, AuthContext, Scope, machine_token, mint_secret, sha256_hex, user_token,
};
use crate::state::AppState;

fn forbid_or(ctx: &AuthContext) -> Result<(), (StatusCode, Json<ApiError>)> {
    ctx.requires(Scope::Admin)
        .map_err(|s| (s, Json(ApiError { error: "admin token required".into() })))
}

fn db_err(e: &sqlx::Error) -> (StatusCode, Json<ApiError>) {
    tracing::error!("db error: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct CreateUserRequest {
    pub name: String,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct CreateUserResponse {
    pub id: Uuid,
    pub name: String,
    pub key: String,
}

#[derive(Serialize, sqlx::FromRow, TS)]
#[ts(export)]
pub struct UserRow {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// Temporary off switch (CCT-251) — auth fails while set, nothing is
    /// invalidated, clearing restores. Distinct from the permanent revoke.
    pub disabled_at: Option<DateTime<Utc>>,
    /// Per-user dispatch permission (CCT-185). Enforced on `POST
    /// /sessions/dispatch`; defaults TRUE.
    pub can_dispatch: bool,
}

#[derive(Serialize, sqlx::FromRow, TS)]
#[ts(export)]
pub struct MachineRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub display_name: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// `persistent` (a real daemon) or `ephemeral` (a dispatch/worker pod).
    /// The New-session picker hides `ephemeral` machines (CCT-183).
    pub kind: String,
    /// Operator-set badge hue (0-359, CCT-222). `None` = hash of the name.
    pub hue: Option<i16>,
    /// Non-secret machine-key fragment, e.g. `cctui_m_ab1234…ef34` (CCT-251).
    /// `None` for machines enrolled before the preview column existed.
    pub key_preview: Option<String>,
    /// Derived online/stale/offline tier from `last_seen_at` age (CCT-255).
    /// Not a DB column — `#[sqlx(skip)]` makes `query_as` ignore it (filled via
    /// `Default`); the handler fills it in from `last_seen_at` after the fetch.
    #[sqlx(skip)]
    pub liveness: cctui_proto::models::MachineLiveness,
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct RenameMachineRequest {
    /// `None` clears the override so the UI falls back to `name`.
    pub display_name: Option<String>,
    /// Badge hue override (0-359, CCT-222). `None` clears it (hash fallback).
    /// The PATCH replaces both fields, so callers send the full pair.
    #[serde(default)]
    pub hue: Option<i16>,
}

/// Partial update of a user (CCT-185). Any field left `None` is unchanged, so
/// the same endpoint serves both rename and the dispatch-permission toggle.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct UpdateUserRequest {
    /// Blank/whitespace is rejected (name is `NOT NULL`); `None` leaves it.
    pub name: Option<String>,
    pub can_dispatch: Option<bool>,
    /// `true` sets `disabled_at = now()`, `false` clears it (CCT-251).
    pub disabled: Option<bool>,
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct RelabelTokenRequest {
    /// `None`/blank clears the label.
    pub label: Option<String>,
}

#[derive(Serialize, sqlx::FromRow, TS)]
#[ts(export)]
pub struct UserTokenRow {
    pub id: Uuid,
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// Non-secret fragment for display (CCT-185), e.g. `cctui_u_ab12…ef34`.
    /// `None` for tokens minted before the preview column existed.
    pub token_preview: Option<String>,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct RotateResponse {
    pub id: Uuid,
    pub key: String,
}

pub async fn create_user(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateUserRequest>,
) -> Result<Json<CreateUserResponse>, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    if req.name.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, Json(ApiError { error: "name required".into() })));
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
        .await
        .map_err(|e| db_err(&e))?;
    // Seed the new user's ceiling (CCT-410): the default capability set a fresh
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
) -> Result<Json<Vec<UserRow>>, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let rows: Vec<UserRow> = sqlx::query_as(
        "SELECT id, name, created_at, revoked_at, disabled_at, can_dispatch \
         FROM users ORDER BY created_at",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    Ok(Json(rows))
}

pub async fn revoke_user(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let res =
        sqlx::query("UPDATE users SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
            .bind(id)
            .execute(&state.pool)
            .await
            .map_err(|e| db_err(&e))?;
    if res.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "user not found".into() })));
    }
    purge_user_cache(&state.auth_config, id, &state.pool).await;
    tracing::info!(user_id = %id, "user revoked");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rotate_user(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<RotateResponse>, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let old_hash: Option<(String,)> =
        sqlx::query_as("SELECT key_hash FROM users WHERE id = $1 AND revoked_at IS NULL")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| db_err(&e))?;
    let Some((old_hash,)) = old_hash else {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "user not found".into() })));
    };
    let secret = mint_secret();
    let token = user_token(&secret);
    let hash = sha256_hex(&token);
    sqlx::query("UPDATE users SET key_hash = $1 WHERE id = $2")
        .bind(&hash)
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|e| db_err(&e))?;
    state.auth_config.purge(&old_hash);
    tracing::info!(user_id = %id, "user key rotated");
    Ok(Json(RotateResponse { id, key: token }))
}

pub async fn list_user_machines(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<Vec<MachineRow>>, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let mut rows: Vec<MachineRow> = sqlx::query_as(
        "SELECT id, user_id, name, display_name, first_seen_at, last_seen_at, revoked_at, kind, \
                hue, key_preview \
         FROM machines WHERE user_id = $1 AND deleted_at IS NULL ORDER BY first_seen_at",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    // Derive the online/stale/offline tier from `last_seen_at` age (CCT-255) so
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
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let res = sqlx::query(
        "UPDATE machines SET deleted_at = now() \
         WHERE id = $1 AND revoked_at IS NOT NULL AND deleted_at IS NULL",
    )
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    if res.rows_affected() == 0 {
        return Err((
            StatusCode::CONFLICT,
            Json(ApiError { error: "machine must be revoked before delete".into() }),
        ));
    }
    tracing::info!(machine_id = %id, "machine deleted (soft)");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn revoke_machine(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let old_hash: Option<(String,)> =
        sqlx::query_as("SELECT key_hash FROM machines WHERE id = $1 AND revoked_at IS NULL")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| db_err(&e))?;
    let Some((old_hash,)) = old_hash else {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "machine not found".into() })));
    };
    sqlx::query("UPDATE machines SET revoked_at = now() WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|e| db_err(&e))?;
    state.auth_config.purge(&old_hash);
    tracing::info!(machine_id = %id, "machine revoked");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rename_machine(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<RenameMachineRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let trimmed = req.display_name.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(h) = req.hue
        && !(0..360).contains(&h)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError { error: "hue must be in 0..360".into() }),
        ));
    }
    let outcome = sqlx::query("UPDATE machines SET display_name = $1, hue = $2 WHERE id = $3")
        .bind(&trimmed)
        .bind(req.hue)
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|e| db_err(&e))?;
    if outcome.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "machine not found".into() })));
    }
    tracing::info!(machine_id = %id, display_name = ?trimmed, "machine renamed");
    Ok(StatusCode::NO_CONTENT)
}

/// Update a user's mutable fields (CCT-150 rename + CCT-185 dispatch toggle).
/// `name` is `NOT NULL`, so a blank name is rejected rather than cleared;
/// `can_dispatch` flips the per-user dispatch permission. Fields left `None`
/// are untouched, so the UI can PATCH just the field it changed.
pub async fn update_user(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let name = match req.name.as_deref().map(str::trim) {
        Some("") => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiError { error: "name required".into() }),
            ));
        }
        other => other,
    };
    if name.is_none() && req.can_dispatch.is_none() && req.disabled.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError { error: "nothing to update".into() }),
        ));
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
    .await
    .map_err(|e| db_err(&e))?;
    if outcome.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "user not found".into() })));
    }
    // Disabling must take effect immediately, not after the auth-cache TTL.
    if req.disabled == Some(true) {
        purge_user_cache(&state.auth_config, id, &state.pool).await;
    }
    tracing::info!(user_id = %id, name, can_dispatch = ?req.can_dispatch, disabled = ?req.disabled, "user updated");
    Ok(StatusCode::NO_CONTENT)
}

/// Permanently delete a revoked user and everything owned by it (CCT-185).
/// Mirrors `delete_machine`'s "must be revoked first" guard so a live user is
/// never destroyed by a mis-click. `machines`, `user_tokens`, `triggers` and
/// uploaded skills cascade on the FK; `sessions` reference the user/machine
/// WITHOUT cascade, so we null those references first (history is preserved,
/// just disowned). All in one transaction so a partial failure rolls back.
pub async fn purge_user(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    // Gather hashes up-front so we can evict them from the auth cache after the
    // row is gone (the rows themselves are about to be deleted).
    purge_user_cache(&state.auth_config, id, &state.pool).await;

    let mut tx = state.pool.begin().await.map_err(|e| db_err(&e))?;
    let revoked: Option<(Option<DateTime<Utc>>,)> =
        sqlx::query_as("SELECT revoked_at FROM users WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| db_err(&e))?;
    let Some((revoked_at,)) = revoked else {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "user not found".into() })));
    };
    if revoked_at.is_none() {
        return Err((
            StatusCode::CONFLICT,
            Json(ApiError { error: "user must be revoked before delete".into() }),
        ));
    }
    // Disown sessions that reference this user or any of its machines (no FK
    // cascade there — preserve the transcript rows, just drop ownership).
    sqlx::query(
        "UPDATE sessions SET machine_uuid = NULL \
         WHERE machine_uuid IN (SELECT id FROM machines WHERE user_id = $1)",
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(|e| db_err(&e))?;
    sqlx::query("UPDATE sessions SET user_id = NULL WHERE user_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| db_err(&e))?;
    let outcome = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| db_err(&e))?;
    if outcome.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "user not found".into() })));
    }
    tx.commit().await.map_err(|e| db_err(&e))?;
    tracing::info!(user_id = %id, "user purged");
    Ok(StatusCode::NO_CONTENT)
}

/// List a user's tokens (CCT-150). Token secrets are never recoverable —
/// this returns only metadata so the UI can relabel/revoke them.
pub async fn list_user_tokens(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<UserTokenRow>>, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let rows: Vec<UserTokenRow> = sqlx::query_as(
        "SELECT id, label, created_at, expires_at, revoked_at, token_preview \
         FROM user_tokens WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    Ok(Json(rows))
}

/// Relabel a token (CCT-150). `None`/blank clears the label.
pub async fn relabel_user_token(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((user_id, token_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<RelabelTokenRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let trimmed = req.label.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let outcome = sqlx::query("UPDATE user_tokens SET label = $1 WHERE id = $2 AND user_id = $3")
        .bind(&trimmed)
        .bind(token_id)
        .bind(user_id)
        .execute(&state.pool)
        .await
        .map_err(|e| db_err(&e))?;
    if outcome.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "token not found".into() })));
    }
    tracing::info!(%user_id, %token_id, label = ?trimmed, "token relabeled");
    Ok(StatusCode::NO_CONTENT)
}

/// Revoke a single token (CCT-150). Mirrors `revoke_user`; purges the
/// auth cache so the token stops working immediately.
pub async fn revoke_user_token(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((user_id, token_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let outcome = sqlx::query(
        "UPDATE user_tokens SET revoked_at = now() \
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(token_id)
    .bind(user_id)
    .execute(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    if outcome.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "token not found".into() })));
    }
    purge_user_cache(&state.auth_config, user_id, &state.pool).await;
    tracing::info!(%user_id, %token_id, "token revoked");
    Ok(StatusCode::NO_CONTENT)
}

/// Hard-delete a token row — "revoke + purge" in one go (CCT-185). Unlike
/// `revoke_user_token` (which keeps the row around showing `revoked`), this
/// removes it entirely. A token is pure auth surface with no historical FK, so
/// deleting the row is safe and equivalent to revoking from a security view.
/// The auth cache is purged so the secret stops working immediately.
pub async fn delete_user_token(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((user_id, token_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let outcome = sqlx::query("DELETE FROM user_tokens WHERE id = $1 AND user_id = $2")
        .bind(token_id)
        .bind(user_id)
        .execute(&state.pool)
        .await
        .map_err(|e| db_err(&e))?;
    if outcome.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "token not found".into() })));
    }
    purge_user_cache(&state.auth_config, user_id, &state.pool).await;
    tracing::info!(%user_id, %token_id, "token purged");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rotate_machine(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<RotateResponse>, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let old_hash: Option<(String,)> =
        sqlx::query_as("SELECT key_hash FROM machines WHERE id = $1 AND revoked_at IS NULL")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| db_err(&e))?;
    let Some((old_hash,)) = old_hash else {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "machine not found".into() })));
    };
    let secret = mint_secret();
    let token = machine_token(&secret);
    let hash = sha256_hex(&token);
    sqlx::query("UPDATE machines SET key_hash = $1, key_preview = $2 WHERE id = $3")
        .bind(&hash)
        .bind(crate::auth::token_preview(&token))
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|e| db_err(&e))?;
    state.auth_config.purge(&old_hash);
    tracing::info!(machine_id = %id, "machine key rotated");
    Ok(Json(RotateResponse { id, key: token }))
}

/// After revoking a user, purge all of that user's machine hashes from cache
/// so machine keys stop working immediately rather than after TTL.
async fn purge_user_cache(auth: &AuthConfig, user_id: Uuid, pool: &sqlx::PgPool) {
    let hashes: Vec<(String,)> = sqlx::query_as("SELECT key_hash FROM machines WHERE user_id = $1")
        .bind(user_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    for (h,) in hashes {
        auth.purge(&h);
    }
    // Also purge the user key itself — need to fetch.
    let user_hash: Option<(String,)> = sqlx::query_as("SELECT key_hash FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);
    if let Some((h,)) = user_hash {
        auth.purge(&h);
    }
}

// ===========================================================================
// CCT-410: per-user scope (ceiling) + per-key (grant) management for the
// Users page. Cross-user actions require the `admin` scope; a user may always
// manage its OWN ceiling (read-only) and its OWN keys (mint/revoke/edit scopes).
// Edits are plain INSERT/DELETE on the acl tables, constrained key ⊆ user, and
// the auth cache is purged so a change takes effect immediately for live keys.
// ===========================================================================

/// Allow if the caller is admin, or is acting on its own account.
fn self_or_admin(ctx: &AuthContext, target: Uuid) -> Result<(), (StatusCode, Json<ApiError>)> {
    if ctx.is_admin() || ctx.user_id == target {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, Json(ApiError { error: "admin scope required".into() })))
    }
}

async fn load_user_acls(pool: &sqlx::PgPool, user_id: Uuid) -> Result<Vec<Scope>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT scope FROM user_acls WHERE user_id = $1")
        .bind(user_id)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().filter_map(|(s,)| Scope::parse(s)).collect())
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct UserAclsResponse {
    pub user_id: Uuid,
    /// The user's ceiling (what its keys may be granted), as scope strings.
    pub scopes: Vec<String>,
}

/// `GET /users/{id}/acls` — the user's ceiling. Self or admin.
pub async fn get_user_acls(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<UserAclsResponse>, (StatusCode, Json<ApiError>)> {
    self_or_admin(&ctx, user_id)?;
    let scopes = load_user_acls(&state.pool, user_id).await.map_err(|e| db_err(&e))?;
    Ok(Json(UserAclsResponse { user_id, scopes: scopes.iter().map(ToString::to_string).collect() }))
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct SetAclsRequest {
    /// The full desired scope set (replaces the existing rows). Strings from the
    /// `read|dispatch|enroll|admin` set; unknown values are rejected.
    pub scopes: Vec<String>,
}

fn parse_scopes(raw: &[String]) -> Result<Vec<Scope>, (StatusCode, Json<ApiError>)> {
    raw.iter()
        .map(|s| {
            Scope::parse(s).ok_or_else(|| {
                (StatusCode::BAD_REQUEST, Json(ApiError { error: format!("unknown scope: {s}") }))
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
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?; // admin only
    let scopes = parse_scopes(&req.scopes)?;
    let mut tx = state.pool.begin().await.map_err(|e| db_err(&e))?;
    sqlx::query("DELETE FROM user_acls WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| db_err(&e))?;
    for scope in &scopes {
        sqlx::query("INSERT INTO user_acls (user_id, scope) VALUES ($1, $2)")
            .bind(user_id)
            .bind(scope.as_str())
            .execute(&mut *tx)
            .await
            .map_err(|e| db_err(&e))?;
    }
    // Keep the legacy can_dispatch flag in sync (CCT-410): the dispatch scope
    // supersedes it, but other code paths / older clients may still read it.
    let can_dispatch = scopes.contains(&Scope::Dispatch);
    sqlx::query("UPDATE users SET can_dispatch = $1 WHERE id = $2")
        .bind(can_dispatch)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| db_err(&e))?;
    tx.commit().await.map_err(|e| db_err(&e))?;
    // A ceiling change affects every key the user owns; we don't track which
    // hashes are cached, so drop the whole cache (short TTL, repopulates fast).
    state.auth_config.purge_all();
    tracing::info!(%user_id, ?scopes, "user ceiling updated");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, sqlx::FromRow, TS)]
#[ts(export)]
pub struct ApiKeyRow {
    pub id: Uuid,
    pub label: Option<String>,
    pub key_preview: Option<String>,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// The key's granted scopes (`key_acls`), filled by the handler.
    #[sqlx(skip)]
    pub scopes: Vec<String>,
}

/// `GET /users/{id}/keys` — the user's `auth_keys` with their granted scopes. Self
/// or admin.
pub async fn list_user_keys(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<Vec<ApiKeyRow>>, (StatusCode, Json<ApiError>)> {
    self_or_admin(&ctx, user_id)?;
    let mut rows: Vec<ApiKeyRow> = sqlx::query_as(
        "SELECT id, label, key_preview, kind, created_at, expires_at, revoked_at \
         FROM auth_keys WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
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

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct MintKeyRequest {
    pub label: Option<String>,
    /// Scopes to grant — must be ⊆ the owner's ceiling (enforced server-side).
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct MintKeyResponse {
    pub id: Uuid,
    /// The plaintext token — returned ONCE, never recoverable after.
    pub key: String,
    pub scopes: Vec<String>,
}

/// `POST /users/{id}/keys` — mint a scoped key for the user. Self or admin. The
/// grant is intersected with the owner's ceiling (`key ⊆ user`, the drift
/// rule): requesting a scope the user doesn't hold is silently dropped.
pub async fn mint_user_key(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<MintKeyRequest>,
) -> Result<Json<MintKeyResponse>, (StatusCode, Json<ApiError>)> {
    self_or_admin(&ctx, user_id)?;
    let requested = parse_scopes(&req.scopes)?;
    let ceiling = load_user_acls(&state.pool, user_id).await.map_err(|e| db_err(&e))?;
    let granted: Vec<Scope> = requested.into_iter().filter(|s| ceiling.contains(s)).collect();

    let token = user_token(&mint_secret());
    let hash = sha256_hex(&token);
    let preview = crate::auth::token_preview(&token);

    // Legacy mirror so the dual-read path also sees it during cutover.
    sqlx::query(
        "INSERT INTO user_tokens (user_id, token_hash, label, expires_at, token_preview) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(&hash)
    .bind(req.label.as_deref())
    .bind(req.expires_at)
    .bind(&preview)
    .execute(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;

    let key_id = crate::auth::register_key(
        &state.pool,
        crate::auth::NewKey {
            user_id,
            key_hash: &hash,
            key_preview: Some(&preview),
            label: req.label.as_deref(),
            kind: "user",
            machine_id: None,
            dispatcher_id: None,
        },
        granted.clone(),
    )
    .await
    .map_err(|e| db_err(&e))?;
    if let Some(exp) = req.expires_at {
        let _ = sqlx::query("UPDATE auth_keys SET expires_at = $1 WHERE id = $2")
            .bind(exp)
            .bind(key_id)
            .execute(&state.pool)
            .await;
    }
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
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    self_or_admin(&ctx, user_id)?;
    // The key must belong to the named user.
    let owns: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM auth_keys WHERE id = $1 AND user_id = $2")
            .bind(key_id)
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| db_err(&e))?;
    if owns.is_none() {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "key not found".into() })));
    }
    let requested = parse_scopes(&req.scopes)?;
    let ceiling = load_user_acls(&state.pool, user_id).await.map_err(|e| db_err(&e))?;
    let granted: Vec<Scope> = requested.into_iter().filter(|s| ceiling.contains(s)).collect();

    let mut tx = state.pool.begin().await.map_err(|e| db_err(&e))?;
    sqlx::query("DELETE FROM key_acls WHERE key_id = $1")
        .bind(key_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| db_err(&e))?;
    for scope in &granted {
        sqlx::query("INSERT INTO key_acls (key_id, scope) VALUES ($1, $2)")
            .bind(key_id)
            .bind(scope.as_str())
            .execute(&mut *tx)
            .await
            .map_err(|e| db_err(&e))?;
    }
    tx.commit().await.map_err(|e| db_err(&e))?;
    state.auth_config.purge_all();
    tracing::info!(%user_id, %key_id, ?granted, "key scopes edited");
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /users/{id}/keys/{kid}` — revoke a key (sets `revoked_at`; cascades
/// drop its `key_acls` on hard-delete, but revoke preserves the audit row). Self
/// or admin. Also revokes the legacy mirror rows by hash so the dual-read path
/// stops accepting it. Cache purged so it stops working immediately.
pub async fn revoke_user_key(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((user_id, key_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    self_or_admin(&ctx, user_id)?;
    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE auth_keys SET revoked_at = now() \
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL RETURNING key_hash",
    )
    .bind(key_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    let Some((hash,)) = row else {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "key not found".into() })));
    };
    // Revoke the legacy mirrors sharing this hash (transparency cutover).
    let _ = sqlx::query("UPDATE user_tokens SET revoked_at = now() WHERE token_hash = $1")
        .bind(&hash)
        .execute(&state.pool)
        .await;
    let _ = sqlx::query("UPDATE machines SET revoked_at = now() WHERE key_hash = $1")
        .bind(&hash)
        .execute(&state.pool)
        .await;
    state.auth_config.purge(&hash);
    tracing::info!(%user_id, %key_id, "key revoked");
    Ok(StatusCode::NO_CONTENT)
}
