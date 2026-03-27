use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use uuid::Uuid;

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

pub async fn list_api_keys(
    State(state): State<AppState>,
) -> Result<Json<Vec<ApiKeyInfo>>, StatusCode> {
    let rows: Vec<ApiKeyInfo> =
        sqlx::query_as("SELECT id, name, provider, created_at FROM api_keys ORDER BY name")
            .fetch_all(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!("db error: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    Ok(Json(rows))
}

pub async fn create_api_key(
    State(state): State<AppState>,
    Json(req): Json<CreateApiKey>,
) -> Result<(StatusCode, Json<ApiKeyInfo>), StatusCode> {
    let vault_key = crate::crypto::vault_key();
    let encrypted = crate::crypto::obfuscate(&req.key, &vault_key);

    let row: ApiKeyInfo = sqlx::query_as(
        "INSERT INTO api_keys (name, provider, encrypted_key) VALUES ($1, $2, $3) \
         RETURNING id, name, provider, created_at",
    )
    .bind(&req.name)
    .bind(&req.provider)
    .bind(&encrypted)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok((StatusCode::CREATED, Json(row)))
}

pub async fn delete_api_key(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    sqlx::query("DELETE FROM api_keys WHERE id = $1").bind(id).execute(&state.pool).await.map_err(
        |e| {
            tracing::error!("db error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        },
    )?;
    Ok(StatusCode::NO_CONTENT)
}

/// Get the decrypted key value (admin only — use carefully)
pub async fn get_api_key_value(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let row: Option<(String,)> = sqlx::query_as("SELECT encrypted_key FROM api_keys WHERE id = $1")
        .bind(id)
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
