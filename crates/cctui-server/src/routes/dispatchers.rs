//! Per-user named dispatcher CRUD (CCT-235): `GET/POST /api/v1/dispatchers`
//! and `PATCH/DELETE /api/v1/dispatchers/{id}`.
//!
//! Auth: a user-scoped token operates on its own dispatchers; an admin token
//! (no owning user) may list across all users. Secrets in the stored `config`
//! (the http bearer token) are encrypted at rest and never echoed back — reads
//! report a `<redacted>` sentinel, writes carry the cleartext in.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::auth::{AuthContext, TokenRole};
use crate::dispatchers::stored::StoredConfig;
use crate::state::AppState;

#[derive(Debug, serde::Serialize)]
pub struct DispatcherInfo {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    /// Type-specific config with secrets redacted.
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct DispatcherRow {
    id: Uuid,
    name: String,
    kind: String,
    config: serde_json::Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl DispatcherRow {
    fn into_info(self) -> DispatcherInfo {
        // Parse the stored (encrypted-at-rest) blob back into the typed shape so
        // we can redact secrets uniformly. A blob that fails to parse falls back
        // to an empty view rather than leaking the raw config.
        let config = build_tagged(&self.kind, &self.config)
            .map_or_else(|| serde_json::json!({}), |c| c.redacted_json());
        DispatcherInfo {
            id: self.id,
            name: self.name,
            kind: self.kind,
            config,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// Merge a stored `kind` + untagged `config` blob into the internally-tagged
/// [`StoredConfig`] enum shape and deserialize it.
fn build_tagged(kind: &str, config: &serde_json::Value) -> Option<StoredConfig> {
    let mut tagged = config.clone();
    if let Some(obj) = tagged.as_object_mut() {
        obj.insert("kind".into(), serde_json::Value::String(kind.to_owned()));
    } else {
        return None;
    }
    serde_json::from_value(tagged).ok()
}

#[derive(Debug, serde::Deserialize)]
pub struct UpsertDispatcher {
    pub name: String,
    /// `http` | `kubernetes`.
    pub kind: String,
    /// Type-specific params (untagged; `kind` selects the shape). Secrets are
    /// cleartext on the way in and get encrypted before storage.
    pub config: serde_json::Value,
}

fn db_err(e: sqlx::Error) -> (StatusCode, Json<serde_json::Value>) {
    // Unique-violation on (user_id, name) → 409.
    if let sqlx::Error::Database(dbe) = &e
        && dbe.code().as_deref() == Some("23505")
    {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "a dispatcher with that name already exists" })),
        );
    }
    tracing::error!("dispatchers db error: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "db error" })))
}

fn parse_config(
    kind: &str,
    config: &serde_json::Value,
) -> Result<StoredConfig, (StatusCode, Json<serde_json::Value>)> {
    build_tagged(kind, config).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("invalid config for kind '{kind}' (expected http or kubernetes)")
            })),
        )
    })
}

pub async fn list_dispatchers(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<DispatcherInfo>>, (StatusCode, Json<serde_json::Value>)> {
    let rows: Vec<DispatcherRow> = if let Some(uid) = ctx.user_id {
        sqlx::query_as(
            "SELECT id, name, kind, config, created_at, updated_at FROM dispatchers \
             WHERE user_id = $1 AND deleted_at IS NULL ORDER BY name",
        )
        .bind(uid)
        .fetch_all(&state.pool)
        .await
    } else if ctx.role == TokenRole::Admin {
        sqlx::query_as(
            "SELECT id, name, kind, config, created_at, updated_at FROM dispatchers \
             WHERE deleted_at IS NULL ORDER BY name",
        )
        .fetch_all(&state.pool)
        .await
    } else {
        Ok(Vec::new())
    }
    .map_err(db_err)?;

    Ok(Json(rows.into_iter().map(DispatcherRow::into_info).collect()))
}

pub async fn create_dispatcher(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<UpsertDispatcher>,
) -> Result<(StatusCode, Json<DispatcherInfo>), (StatusCode, Json<serde_json::Value>)> {
    let uid = ctx.user_id.ok_or((
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "error": "a user token is required to create dispatchers" })),
    ))?;

    let mut cfg = parse_config(&req.kind, &req.config)?;
    cfg.encrypt_secrets(&crate::crypto::vault_key());
    // Store the untagged blob (the `kind` column carries the tag).
    let mut stored = serde_json::to_value(&cfg).map_err(|e| {
        tracing::error!("serialize dispatcher config: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "config error" })))
    })?;
    if let Some(obj) = stored.as_object_mut() {
        obj.remove("kind");
    }

    let row: DispatcherRow = sqlx::query_as(
        "INSERT INTO dispatchers (user_id, name, kind, config) VALUES ($1, $2, $3, $4) \
         RETURNING id, name, kind, config, created_at, updated_at",
    )
    .bind(uid)
    .bind(&req.name)
    .bind(cfg.kind())
    .bind(&stored)
    .fetch_one(&state.pool)
    .await
    .map_err(db_err)?;

    Ok((StatusCode::CREATED, Json(row.into_info())))
}

pub async fn update_dispatcher(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpsertDispatcher>,
) -> Result<Json<DispatcherInfo>, (StatusCode, Json<serde_json::Value>)> {
    let uid = ctx.user_id.ok_or((
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "error": "a user token is required to edit dispatchers" })),
    ))?;

    let mut cfg = parse_config(&req.kind, &req.config)?;
    cfg.encrypt_secrets(&crate::crypto::vault_key());
    let mut stored = serde_json::to_value(&cfg).map_err(|e| {
        tracing::error!("serialize dispatcher config: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "config error" })))
    })?;
    if let Some(obj) = stored.as_object_mut() {
        obj.remove("kind");
    }

    let row: Option<DispatcherRow> = sqlx::query_as(
        "UPDATE dispatchers SET name = $3, kind = $4, config = $5, updated_at = now() \
         WHERE id = $1 AND user_id = $2 AND deleted_at IS NULL \
         RETURNING id, name, kind, config, created_at, updated_at",
    )
    .bind(id)
    .bind(uid)
    .bind(&req.name)
    .bind(cfg.kind())
    .bind(&stored)
    .fetch_optional(&state.pool)
    .await
    .map_err(db_err)?;

    row.map(|r| Json(r.into_info())).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "dispatcher not found" })),
    ))
}

pub async fn delete_dispatcher(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let uid = ctx.user_id.ok_or((
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "error": "a user token is required to delete dispatchers" })),
    ))?;

    let res = sqlx::query(
        "UPDATE dispatchers SET deleted_at = now() \
         WHERE id = $1 AND user_id = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(uid)
    .execute(&state.pool)
    .await
    .map_err(db_err)?;

    if res.rows_affected() == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "dispatcher not found" })),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}
