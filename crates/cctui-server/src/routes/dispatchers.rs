//! Per-user enrolled-dispatcher management: `GET /api/v1/dispatchers`
//! (list with liveness), `PATCH /api/v1/dispatchers/{id}` (rename), and
//! `DELETE /api/v1/dispatchers/{id}` (remove). Peer of the machines management
//! surface.
//!
//! Enrollment itself (minting an identity + key) is `POST
//! /api/v1/dispatcher/enroll` ([`crate::routes::dispatcher`]); a dispatcher key
//! is returned once there and never echoed here.
//!
//! Auth: a user-scoped token operates on its own dispatchers; an admin token
//! (no owning user) may list across all users.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::auth::{AuthContext, Scope};
use crate::state::AppState;

#[derive(Debug, serde::Serialize)]
pub struct DispatcherInfo {
    pub id: Uuid,
    pub name: String,
    /// Reported by the binary at enroll: `kubernetes` | `docker` | `http`.
    pub kind: String,
    /// Non-secret fragment of the enrollment key, for display.
    pub key_preview: Option<String>,
    /// Liveness tier derived from `last_seen_at` age.
    pub liveness: cctui_proto::models::MachineLiveness,
    /// Whether a live WS connection is currently registered for this dispatcher.
    pub connected: bool,
    pub last_seen_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The account a dispatch that names none routes through.
    pub default_account: Option<String>,
    /// The pool such a dispatch elects within. Set with no `default_account`,
    /// or the account wins.
    pub default_pool: Option<String>,
}

#[derive(sqlx::FromRow)]
struct DispatcherRow {
    id: Uuid,
    name: String,
    kind: String,
    key_preview: Option<String>,
    last_seen_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    default_account: Option<String>,
    default_pool: Option<String>,
}

impl DispatcherRow {
    fn into_info(self, state: &AppState) -> DispatcherInfo {
        let connected = state.bus.dispatcher_connected(self.id);
        let liveness = crate::machine_liveness::derive(self.last_seen_at);
        DispatcherInfo {
            id: self.id,
            name: self.name,
            kind: self.kind,
            key_preview: self.key_preview,
            liveness,
            connected,
            last_seen_at: self.last_seen_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
            default_account: self.default_account,
            default_pool: self.default_pool,
        }
    }
}

/// A dispatcher edit. `name` renames; each binding field is left untouched when
/// absent, cleared by an empty string, and set otherwise — so the one-control
/// UI can express "bind this pool, drop the account" in a single call.
#[derive(Debug, serde::Deserialize)]
pub struct RenameDispatcher {
    pub name: String,
    #[serde(default)]
    pub default_account: Option<String>,
    #[serde(default)]
    pub default_pool: Option<String>,
}

fn db_err(e: &sqlx::Error) -> (StatusCode, Json<serde_json::Value>) {
    if let sqlx::Error::Database(dbe) = e
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

const SELECT_COLS: &str = "d.id, d.name, d.kind, d.key_preview, d.last_seen_at, \
     d.created_at, d.updated_at, a.name AS default_account, p.name AS default_pool \
     FROM dispatchers d \
     LEFT JOIN accounts a ON a.id = d.default_account_id \
     LEFT JOIN account_pools p ON p.id = d.default_pool_id";

pub async fn list_dispatchers(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<DispatcherInfo>>, (StatusCode, Json<serde_json::Value>)> {
    // Uniform god-view: admin (`owner_filter` = NULL) sees all
    // dispatchers; a user sees only their own.
    let rows: Vec<DispatcherRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SELECT_COLS} \
         WHERE ($1::uuid IS NULL OR d.user_id = $1 \
                OR EXISTS (SELECT 1 FROM resource_shares s \
                           WHERE s.resource_type = 'dispatcher' AND s.resource_id = d.id \
                             AND s.grantee_id = $1 AND s.revoked_at IS NULL)) \
         AND d.deleted_at IS NULL AND d.revoked_at IS NULL ORDER BY d.name"
    )))
    .bind(ctx.owner_filter())
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;

    Ok(Json(rows.into_iter().map(|r| r.into_info(&state)).collect()))
}

pub async fn update_dispatcher(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<RenameDispatcher>,
) -> Result<Json<DispatcherInfo>, (StatusCode, Json<serde_json::Value>)> {
    ctx.requires(Scope::Enroll).map_err(|s| {
        (
            s,
            Json(
                serde_json::json!({ "error": "the enroll scope is required to edit dispatchers" }),
            ),
        )
    })?;
    if req.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "name is required" })),
        ));
    }

    let not_found =
        || (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "dispatcher not found" })));

    // Names are resolved against the dispatcher's OWNER, not the caller: an
    // admin editing someone else's dispatcher must not bind its own accounts.
    let owner: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM dispatchers \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    let Some(owner) = owner else { return Err(not_found()) };

    let account = resolve_binding_id(
        &state,
        owner,
        parse_change(req.default_account.as_deref()),
        "SELECT id FROM accounts WHERE user_id = $1 AND name = $2",
        "account",
    )
    .await?;
    let pool = resolve_binding_id(
        &state,
        owner,
        parse_change(req.default_pool.as_deref()),
        "SELECT id FROM account_pools WHERE user_id = $1 AND lower(name) = lower($2)",
        "account pool",
    )
    .await?;

    sqlx::query(
        "UPDATE dispatchers SET name = $3, \
           default_account_id = CASE WHEN $4 THEN $5 ELSE default_account_id END, \
           default_pool_id = CASE WHEN $6 THEN $7 ELSE default_pool_id END, \
           updated_at = now() \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .bind(req.name.trim())
    .bind(account.is_some())
    .bind(account.flatten())
    .bind(pool.is_some())
    .bind(pool.flatten())
    .execute(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;

    let row: Option<DispatcherRow> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {SELECT_COLS} WHERE d.id = $1")))
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| db_err(&e))?;

    row.map(|r| Json(r.into_info(&state))).ok_or_else(not_found)
}

/// What an edit says about one binding field.
#[derive(Debug, PartialEq, Eq)]
enum Change<'a> {
    Keep,
    Clear,
    Set(&'a str),
}

/// Absent leaves the binding alone; a blank string unbinds it. Pure so the
/// three-way distinction — which JSON alone cannot express — is testable.
fn parse_change(raw: Option<&str>) -> Change<'_> {
    match raw.map(str::trim) {
        None => Change::Keep,
        Some("") => Change::Clear,
        Some(name) => Change::Set(name),
    }
}

/// `None` to leave the column alone, `Some(None)` to null it, `Some(Some(id))`
/// to point it somewhere. A named row that does not exist is a 404.
async fn resolve_binding_id(
    state: &AppState,
    owner: Uuid,
    change: Change<'_>,
    lookup_sql: &'static str,
    label: &str,
) -> Result<Option<Option<Uuid>>, (StatusCode, Json<serde_json::Value>)> {
    match change {
        Change::Keep => Ok(None),
        Change::Clear => Ok(Some(None)),
        Change::Set(name) => {
            let found: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(lookup_sql))
                .bind(owner)
                .bind(name)
                .fetch_optional(&state.pool)
                .await
                .map_err(|e| db_err(&e))?;
            found.map(|id| Some(Some(id))).ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": format!("no {label} named {name:?}") })),
                )
            })
        }
    }
}

pub async fn delete_dispatcher(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    ctx.requires(Scope::Enroll).map_err(|s| {
        (s, Json(serde_json::json!({ "error": "the enroll scope is required to delete dispatchers" })))
    })?;

    let res = sqlx::query(
        "UPDATE dispatchers SET revoked_at = COALESCE(revoked_at, now()), deleted_at = now() \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .execute(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;

    if res.rows_affected() == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "dispatcher not found" })),
        ));
    }
    // Drop any live connection so the dispatcher can't keep operating under a
    // removed identity (it'll fail to re-auth on reconnect).
    state.bus.evict_dispatcher(id);
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::{Change, parse_change};

    #[test]
    fn an_absent_field_keeps_a_binding_and_a_blank_one_clears_it() {
        // A rename that says nothing about the bindings must not silently drop
        // the dispatcher's pool.
        assert_eq!(parse_change(None), Change::Keep);
        assert_eq!(parse_change(Some("")), Change::Clear);
        assert_eq!(parse_change(Some("   ")), Change::Clear);
        assert_eq!(parse_change(Some(" work ")), Change::Set("work"));
    }
}
