use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::state::AppState;

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct ApiKeyInfo {
    pub id: Uuid,
    pub name: String,
    pub provider: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateApiKey {
    pub name: String,
    pub provider: String,
    pub key: String,
}

/// `GET /keys` — provider keys visible to the caller. Owner-scoped: a non-admin
/// sees only the keys they own; an admin sees all (including legacy NULL-owner
/// rows). The `$1::uuid IS NULL` god-view binding collapses both cases.
pub async fn list_api_keys(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<ApiKeyInfo>>, StatusCode> {
    let rows: Vec<ApiKeyInfo> = sqlx::query_as(
        "SELECT id, name, provider, created_at FROM api_keys \
         WHERE $1::uuid IS NULL OR user_id = $1 ORDER BY name",
    )
    .bind(ctx.owner_filter())
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(rows))
}

/// `POST /keys` — store a provider key owned by the caller.
pub async fn create_api_key(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateApiKey>,
) -> Result<(StatusCode, Json<ApiKeyInfo>), StatusCode> {
    let vault_key = crate::crypto::vault_key();
    let encrypted = crate::crypto::obfuscate(&req.key, &vault_key);

    let row: ApiKeyInfo = sqlx::query_as(
        "INSERT INTO api_keys (name, provider, encrypted_key, user_id) VALUES ($1, $2, $3, $4) \
         RETURNING id, name, provider, created_at",
    )
    .bind(&req.name)
    .bind(&req.provider)
    .bind(&encrypted)
    .bind(ctx.user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// `DELETE /keys/{id}` — owner-or-admin only. The god-view binding scopes the
/// `DELETE` so a non-admin can never remove another user's (or a legacy
/// admin-owned NULL) key; 0 rows affected → 404.
pub async fn delete_api_key(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let res =
        sqlx::query("DELETE FROM api_keys WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)")
            .bind(id)
            .bind(ctx.owner_filter())
            .execute(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!("db error: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    if res.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /keys/{id}/value` — the decrypted key. Owner-or-admin only; a row the
/// caller doesn't own is invisible (404), never 403, so existence isn't leaked.
pub async fn get_api_key_value(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT encrypted_key FROM api_keys WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let (encrypted,) = row.ok_or(StatusCode::NOT_FOUND)?;
    let vault_key = crate::crypto::vault_key();
    let decrypted = crate::crypto::deobfuscate(&encrypted, &vault_key)
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "key": decrypted })))
}
