//! GitHub HTTP handlers.
//!
//! GH-CONN-1 lands real connector CRUD on `/api/v1/github/connectors`: create
//! (encrypts the credential at rest with the vault key, same as the OAuth-account
//! vault), list, update, delete, and an on-demand sync. The credential is
//! **never** returned — list/get only surface a masked preview + whether a
//! webhook secret is set. The webhook ingress (`triggers/github`) is implemented
//! in [`crate::webhook`] (GH-CONN-2).
//!
//! Auth: the nested GitHub router is wrapped (in `cctui-server::main`) with the
//! same auth middleware as the rest of `/api/v1`, plus a thin layer that maps the
//! server's `AuthContext` into a [`CallerIdentity`] extension. A user acts as
//! itself; the admin token has no user identity and must name the owner.
#![allow(clippy::unused_async)]

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::github::{CallerIdentity, ConnectorInfo, CreateConnector, GithubCredentialKind};
use uuid::Uuid;

use crate::{GithubState, crypto};

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err(code: StatusCode, msg: &str) -> ApiError {
    (code, Json(serde_json::json!({ "error": msg })))
}

const fn kind_str(k: GithubCredentialKind) -> &'static str {
    match k {
        GithubCredentialKind::Pat => "pat",
        GithubCredentialKind::AppInstallation => "app_installation",
    }
}

fn kind_from_str(s: &str) -> GithubCredentialKind {
    match s {
        "app_installation" => GithubCredentialKind::AppInstallation,
        _ => GithubCredentialKind::Pat,
    }
}

/// Resolve which user a connector operation targets. A user always acts as
/// itself; the admin token has no user identity, so it must name the owner
/// explicitly (mirrors the OAuth-account vault, CCT-251).
fn resolve_owner(ctx: &CallerIdentity, explicit: Option<Uuid>) -> Result<Uuid, ApiError> {
    if let Some(uid) = ctx.user_id {
        return Ok(uid);
    }
    if ctx.is_admin {
        return explicit.ok_or_else(|| {
            err(StatusCode::BAD_REQUEST, "user_id required when using the admin token")
        });
    }
    Err(err(StatusCode::FORBIDDEN, "user or admin token required"))
}

/// One connector row, as stored. The credential columns hold ciphertext only.
struct ConnectorRow {
    id: Uuid,
    user_id: Uuid,
    name: String,
    credential_kind: String,
    encrypted_credential: String,
    encrypted_webhook_secret: Option<String>,
    repos: Vec<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    last_polled_at: Option<chrono::DateTime<chrono::Utc>>,
    last_error: Option<String>,
}

impl ConnectorRow {
    /// Project to the API view. Decrypts the credential **only** to derive a
    /// masked preview — the plaintext is dropped immediately and never sent.
    fn into_info(self) -> ConnectorInfo {
        let key = crypto::vault_key();
        let preview = crypto::decrypt(&self.encrypted_credential, &key)
            .as_deref()
            .map_or_else(|| "•••".to_string(), crypto::credential_preview);
        ConnectorInfo {
            id: self.id,
            name: self.name,
            credential_kind: kind_from_str(&self.credential_kind),
            credential_preview: preview,
            has_webhook_secret: self.encrypted_webhook_secret.is_some(),
            repos: self.repos,
            user_id: self.user_id,
            created_at: self.created_at.to_rfc3339(),
            last_polled_at: self.last_polled_at.map(|t| t.to_rfc3339()),
            last_error: self.last_error,
        }
    }
}

impl sqlx::FromRow<'_, sqlx::postgres::PgRow> for ConnectorRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            name: row.try_get("name")?,
            credential_kind: row.try_get("credential_kind")?,
            encrypted_credential: row.try_get("encrypted_credential")?,
            encrypted_webhook_secret: row.try_get("encrypted_webhook_secret")?,
            repos: row.try_get("repos")?,
            created_at: row.try_get("created_at")?,
            last_polled_at: row.try_get("last_polled_at")?,
            last_error: row.try_get("last_error")?,
        })
    }
}

const SELECT_COLS: &str = "id, user_id, name, credential_kind, encrypted_credential, \
                           encrypted_webhook_secret, repos, created_at, last_polled_at, \
                           last_error";

/// `GET /api/v1/github/connectors` — the caller's connectors (credential masked).
/// Admin sees every connector.
pub async fn list_connectors(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
) -> Result<Json<Vec<ConnectorInfo>>, ApiError> {
    // Admin (user_id = None) sees all; a user only its own.
    let rows: Vec<ConnectorRow> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM github.connectors \
         WHERE $1::uuid IS NULL OR user_id = $1 ORDER BY name"
    ))
    .bind(ctx.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("github connectors list db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    Ok(Json(rows.into_iter().map(ConnectorRow::into_info).collect()))
}

/// `POST /api/v1/github/connectors` — register a connector with an encrypted
/// credential. The plaintext credential and webhook secret are encrypted at rest
/// and never returned.
pub async fn create_connector(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Json(req): Json<CreateConnector>,
) -> Result<(StatusCode, Json<ConnectorInfo>), ApiError> {
    let uid = resolve_owner(&ctx, req.user_id)?;
    if req.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }
    if req.credential.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "credential required"));
    }

    let key = crypto::vault_key();
    let enc_credential = crypto::encrypt(req.credential.trim(), &key);
    let enc_webhook = req
        .webhook_secret
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| crypto::encrypt(s, &key));
    let repos: Vec<String> =
        req.repos.iter().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();

    let row: Result<ConnectorRow, sqlx::Error> = sqlx::query_as(&format!(
        "INSERT INTO github.connectors \
            (user_id, name, credential_kind, encrypted_credential, \
             encrypted_webhook_secret, repos) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING {SELECT_COLS}"
    ))
    .bind(uid)
    .bind(req.name.trim())
    .bind(kind_str(req.credential_kind))
    .bind(&enc_credential)
    .bind(&enc_webhook)
    .bind(&repos)
    .fetch_one(&state.pool)
    .await;

    match row {
        Ok(r) => Ok((StatusCode::CREATED, Json(r.into_info()))),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            Err(err(StatusCode::CONFLICT, "a connector with that name already exists"))
        }
        Err(e) => {
            tracing::error!("github connector create db error: {e}");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "database error"))
        }
    }
}

/// `PATCH /api/v1/github/connectors/{id}` — update a connector's name, tracked
/// repos, credential, and/or webhook secret. Only the fields present in the body
/// change. Rotating the credential clears the cached `viewer_login` (and the
/// stale `last_error`) so the next poll re-resolves against the new token. A
/// user may edit only its own connectors; admin may edit any.
pub async fn update_connector(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path(id): Path<Uuid>,
    Json(req): Json<cctui_proto::github::UpdateConnector>,
) -> Result<Json<ConnectorInfo>, ApiError> {
    scope_connector(&state, &ctx, id).await?;

    if let Some(name) = req.name.as_deref()
        && name.trim().is_empty()
    {
        return Err(err(StatusCode::BAD_REQUEST, "name cannot be empty"));
    }

    let key = crypto::vault_key();
    // Build the SET clause from the present fields only. `COALESCE($n, col)`
    // leaves a column unchanged when the bind is NULL.
    let new_name = req.name.as_deref().map(str::trim).map(str::to_string);
    let new_repos = req.repos.as_ref().map(|rs| {
        rs.iter().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect::<Vec<_>>()
    });
    // Only rotate the credential when a non-empty one is supplied.
    let new_credential = req
        .credential
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| crypto::encrypt(s, &key));
    let rotating = new_credential.is_some();
    // Webhook secret: Some("") clears, Some(non-empty) sets, None leaves as-is.
    // `set_webhook` flags whether to touch the column at all.
    let (set_webhook, new_webhook) = match req.webhook_secret.as_deref().map(str::trim) {
        None => (false, None),
        Some("") => (true, None),
        Some(s) => (true, Some(crypto::encrypt(s, &key))),
    };

    let row: Result<ConnectorRow, sqlx::Error> = sqlx::query_as(&format!(
        "UPDATE github.connectors SET \
            name = COALESCE($2, name), \
            repos = COALESCE($3, repos), \
            encrypted_credential = COALESCE($4, encrypted_credential), \
            encrypted_webhook_secret = CASE WHEN $5 THEN $6 ELSE encrypted_webhook_secret END, \
            viewer_login = CASE WHEN $7 THEN NULL ELSE viewer_login END, \
            last_error = CASE WHEN $7 THEN NULL ELSE last_error END, \
            updated_at = now() \
         WHERE id = $1 RETURNING {SELECT_COLS}"
    ))
    .bind(id)
    .bind(new_name)
    .bind(new_repos)
    .bind(new_credential)
    .bind(set_webhook)
    .bind(new_webhook)
    .bind(rotating)
    .fetch_one(&state.pool)
    .await;

    match row {
        Ok(r) => Ok(Json(r.into_info())),
        Err(sqlx::Error::RowNotFound) => Err(err(StatusCode::NOT_FOUND, "no such connector")),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            Err(err(StatusCode::CONFLICT, "a connector with that name already exists"))
        }
        Err(e) => {
            tracing::error!("github connector update db error: {e}");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "database error"))
        }
    }
}

/// `DELETE /api/v1/github/connectors/{id}` — delete a connector and its
/// encrypted credential. A user may delete only its own; admin may delete any.
pub async fn delete_connector(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    if ctx.user_id.is_none() && !ctx.is_admin {
        return Err(err(StatusCode::FORBIDDEN, "user or admin token required"));
    }
    let res = sqlx::query(
        "DELETE FROM github.connectors WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(ctx.user_id)
    .execute(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("github connector delete db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    if res.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such connector"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/github/connectors/{id}/sync` — run the reconcile poll for one
/// connector immediately, instead of waiting for the next scheduled tick
/// (CCT-396). Scoped to the caller (admin may sync any). Returns the updated
/// connector view, whose `last_polled_at`/`last_error` reflect this attempt — so
/// a bad credential surfaces right away rather than only in the server log.
pub async fn sync_connector(
    State(state): State<GithubState>,
    Extension(ctx): Extension<CallerIdentity>,
    Path(id): Path<Uuid>,
) -> Result<Json<ConnectorInfo>, ApiError> {
    scope_connector(&state, &ctx, id).await?;
    crate::reconcile::sync_now(&state, id).await;
    let row: Option<ConnectorRow> =
        sqlx::query_as(&format!("SELECT {SELECT_COLS} FROM github.connectors WHERE id = $1"))
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!("github connector sync reload db error: {e}");
                err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;
    row.map(|r| Json(r.into_info())).ok_or_else(|| err(StatusCode::NOT_FOUND, "no such connector"))
}

/// Assert the caller may act on the connector. A user may act only on its own
/// connectors; the admin token may act on any. A connector the caller can't see
/// is reported as `404` (not `403`) so its existence isn't leaked.
async fn scope_connector(
    state: &GithubState,
    ctx: &CallerIdentity,
    connector_id: Uuid,
) -> Result<(), ApiError> {
    if ctx.user_id.is_none() && !ctx.is_admin {
        return Err(err(StatusCode::FORBIDDEN, "user or admin token required"));
    }
    let owned: Option<bool> = sqlx::query_scalar(
        "SELECT true FROM github.connectors \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(connector_id)
    .bind(ctx.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("github connector scope db error: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    if owned.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "no such connector"));
    }
    Ok(())
}
