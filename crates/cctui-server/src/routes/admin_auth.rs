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
    AuthConfig, AuthContext, machine_token, mint_secret, require_admin, sha256_hex, user_token,
};
use crate::state::AppState;

fn forbid_or(ctx: &AuthContext) -> Result<(), (StatusCode, Json<ApiError>)> {
    require_admin(ctx).map_err(|s| (s, Json(ApiError { error: "admin token required".into() })))
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
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct RenameMachineRequest {
    /// `None` clears the override so the UI falls back to `name`.
    pub display_name: Option<String>,
}

/// Partial update of a user (CCT-185). Any field left `None` is unchanged, so
/// the same endpoint serves both rename and the dispatch-permission toggle.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct UpdateUserRequest {
    /// Blank/whitespace is rejected (name is `NOT NULL`); `None` leaves it.
    pub name: Option<String>,
    pub can_dispatch: Option<bool>,
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
    tracing::info!(user_id = %id, name = %req.name, "user created");
    Ok(Json(CreateUserResponse { id, name: req.name, key: token }))
}

pub async fn list_users(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<UserRow>>, (StatusCode, Json<ApiError>)> {
    forbid_or(&ctx)?;
    let rows: Vec<UserRow> = sqlx::query_as(
        "SELECT id, name, created_at, revoked_at, can_dispatch FROM users ORDER BY created_at",
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
    let rows: Vec<MachineRow> = sqlx::query_as(
        "SELECT id, user_id, name, display_name, first_seen_at, last_seen_at, revoked_at, kind \
         FROM machines WHERE user_id = $1 AND deleted_at IS NULL ORDER BY first_seen_at",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
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
    let outcome = sqlx::query("UPDATE machines SET display_name = $1 WHERE id = $2")
        .bind(&trimmed)
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
    if name.is_none() && req.can_dispatch.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError { error: "nothing to update".into() }),
        ));
    }
    // COALESCE keeps the existing value when a field is NULL (not supplied).
    let outcome = sqlx::query(
        "UPDATE users SET \
            name = COALESCE($1, name), \
            can_dispatch = COALESCE($2, can_dispatch) \
         WHERE id = $3",
    )
    .bind(name)
    .bind(req.can_dispatch)
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    if outcome.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "user not found".into() })));
    }
    tracing::info!(user_id = %id, name, can_dispatch = ?req.can_dispatch, "user updated");
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
    sqlx::query("UPDATE machines SET key_hash = $1 WHERE id = $2")
        .bind(&hash)
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
