//! `/api/v1/accounts` — the OAuth account vault (CCT-232).
//!
//! Users register **named OAuth accounts** for Claude Code / Codex (e.g.
//! `personal`, `enterprise`) and pick one per job at spawn/dispatch time. The
//! OAuth refresh token is encrypted at rest with the vault key (`crate::crypto`,
//! same as `api_keys`/`dispatchers`) and is **never** returned over the API —
//! list/get only ever surface name/provider/expiry/last-used + lightweight
//! stats. Accounts belong to the registering user and are visible/usable only by
//! that user (`require_user`).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::auth::{AuthContext, require_user};
use crate::state::AppState;

/// API view of an account — secrets (tokens) deliberately absent.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct AccountInfo {
    pub id: Uuid,
    pub name: String,
    pub provider: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub request_count: i64,
    pub bytes_transferred: i64,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateAccount {
    pub name: String,
    /// `anthropic` | `openai`.
    pub provider: String,
    /// OAuth refresh token (pasted by the user). Stored encrypted; the gateway
    /// exchanges it for access tokens on demand.
    pub refresh_token: String,
    /// Optional initial access token (skips the first refresh round-trip).
    #[serde(default)]
    pub access_token: Option<String>,
    /// Optional access-token expiry (unix seconds). When absent the gateway
    /// refreshes on first use.
    #[serde(default)]
    pub expires_at: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
pub struct RenameAccount {
    pub name: String,
}

fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(serde_json::json!({ "error": msg })))
}

/// `GET /api/v1/accounts` — the caller's own accounts (tokens never returned).
pub async fn list_accounts(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<AccountInfo>>, (StatusCode, Json<serde_json::Value>)> {
    let uid = require_user(&ctx).map_err(|c| err(c, "user token required"))?;
    let rows: Vec<AccountInfo> = sqlx::query_as(
        "SELECT id, name, provider, expires_at, created_at, last_used_at, \
                request_count, bytes_transferred \
         FROM oauth_accounts WHERE user_id = $1 ORDER BY provider, name",
    )
    .bind(uid)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    Ok(Json(rows))
}

/// `POST /api/v1/accounts` — register a named OAuth account.
pub async fn create_account(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateAccount>,
) -> Result<(StatusCode, Json<AccountInfo>), (StatusCode, Json<serde_json::Value>)> {
    let uid = require_user(&ctx).map_err(|c| err(c, "user token required"))?;
    if req.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }
    if !matches!(req.provider.as_str(), "anthropic" | "openai") {
        return Err(err(StatusCode::BAD_REQUEST, "provider must be anthropic|openai"));
    }
    if req.refresh_token.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "refresh_token required"));
    }

    let key = crate::crypto::vault_key();
    let enc_refresh = crate::crypto::obfuscate(&req.refresh_token, &key);
    let enc_access = req.access_token.as_deref().map(|t| crate::crypto::obfuscate(t, &key));
    let expires_at = req.expires_at.and_then(|s| DateTime::<Utc>::from_timestamp(s, 0));

    let row: Result<AccountInfo, sqlx::Error> = sqlx::query_as(
        "INSERT INTO oauth_accounts \
            (user_id, name, provider, encrypted_refresh_token, encrypted_access_token, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, name, provider, expires_at, created_at, last_used_at, \
                   request_count, bytes_transferred",
    )
    .bind(uid)
    .bind(req.name.trim())
    .bind(&req.provider)
    .bind(&enc_refresh)
    .bind(&enc_access)
    .bind(expires_at)
    .fetch_one(&state.pool)
    .await;

    match row {
        Ok(info) => Ok((StatusCode::CREATED, Json(info))),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            Err(err(StatusCode::CONFLICT, "an account with that name+provider already exists"))
        }
        Err(e) => {
            tracing::error!("db error: {e}");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "database error"))
        }
    }
}

/// `PATCH /api/v1/accounts/{id}` — rename.
pub async fn rename_account(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<RenameAccount>,
) -> Result<Json<AccountInfo>, (StatusCode, Json<serde_json::Value>)> {
    let uid = require_user(&ctx).map_err(|c| err(c, "user token required"))?;
    if req.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }
    let row: Option<AccountInfo> = sqlx::query_as(
        "UPDATE oauth_accounts SET name = $3 WHERE id = $1 AND user_id = $2 \
         RETURNING id, name, provider, expires_at, created_at, last_used_at, \
                   request_count, bytes_transferred",
    )
    .bind(id)
    .bind(uid)
    .bind(req.name.trim())
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    row.map(Json).ok_or_else(|| err(StatusCode::NOT_FOUND, "no such account"))
}

/// `DELETE /api/v1/accounts/{id}` — delete (cascades session_tokens).
pub async fn delete_account(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let uid = require_user(&ctx).map_err(|c| err(c, "user token required"))?;
    let res = sqlx::query("DELETE FROM oauth_accounts WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(uid)
        .execute(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;
    if res.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such account"));
    }
    Ok(StatusCode::NO_CONTENT)
}
