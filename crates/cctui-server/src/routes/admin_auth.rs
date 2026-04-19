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

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
}

#[derive(Serialize)]
pub struct CreateUserResponse {
    pub id: Uuid,
    pub name: String,
    pub key: String,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct MachineRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
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
    let rows: Vec<UserRow> =
        sqlx::query_as("SELECT id, name, created_at, revoked_at FROM users ORDER BY created_at")
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
        "SELECT id, user_id, name, first_seen_at, last_seen_at, revoked_at \
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
