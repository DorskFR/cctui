use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::state::AppState;

#[derive(Debug, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct Prompt {
    pub id: Uuid,
    pub name: String,
    pub content: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreatePrompt {
    pub name: String,
    pub content: String,
    pub description: Option<String>,
}

pub async fn list_prompts(State(state): State<AppState>) -> Result<Json<Vec<Prompt>>, StatusCode> {
    let rows: Vec<Prompt> = sqlx::query_as(
        "SELECT id, name, content, description, created_at, updated_at FROM prompts ORDER BY name",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(rows))
}

pub async fn create_prompt(
    State(state): State<AppState>,
    Json(req): Json<CreatePrompt>,
) -> Result<(StatusCode, Json<Prompt>), StatusCode> {
    let row: Prompt = sqlx::query_as(
        "INSERT INTO prompts (name, content, description) VALUES ($1, $2, $3) \
         RETURNING id, name, content, description, created_at, updated_at",
    )
    .bind(&req.name)
    .bind(&req.content)
    .bind(&req.description)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok((StatusCode::CREATED, Json(row)))
}

pub async fn get_prompt(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Prompt>, StatusCode> {
    let row: Option<Prompt> = sqlx::query_as(
        "SELECT id, name, content, description, created_at, updated_at FROM prompts WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    row.map(Json).ok_or(StatusCode::NOT_FOUND)
}

pub async fn delete_prompt(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    sqlx::query("DELETE FROM prompts WHERE id = $1").bind(id).execute(&state.pool).await.map_err(
        |e| {
            tracing::error!("db error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        },
    )?;
    Ok(StatusCode::NO_CONTENT)
}
