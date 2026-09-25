//! `/api/v1/{resource_type}/{id}/shares` — the generic resource-sharing CRUD
//! family, the polymorphic generalization of the bespoke account
//! sharing (510). One `resource_shares` table backs every shareable
//! kind (account | machine | dispatcher | `context_pack`). A live grant row lets a
//! NON-owner `use` the resource without transferring ownership; a grant confers
//! `use` only, NEVER re-sharing — share management stays owner-or-admin.
//!
//! The single grant-lookup primitive [`granted`] is what `Resource::authorize`
//! (authz.rs) composes onto ownership for every shareable kind, so there is one
//! enforcement path. These handlers manage the rows; the authz guard reads them.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::authz::{Shareable, shareable_owner};
use crate::error::err;
use crate::state::AppState;

/// The single grant-lookup primitive: does `grantee` hold a LIVE `use` grant on
/// `(resource_type, resource_id)`? Called from `Resource::authorize` (the
/// sharing seam) and the gateway resolution SQL alike, so ownership and grants
/// compose on one path. Only `action = 'use'` exists today.
pub async fn granted(
    pool: &sqlx::PgPool,
    resource_type: &str,
    resource_id: Uuid,
    grantee: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM resource_shares \
         WHERE resource_type = $1 AND resource_id = $2 AND grantee_id = $3 \
           AND revoked_at IS NULL LIMIT 1",
    )
    .bind(resource_type)
    .bind(resource_id)
    .bind(grantee)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

fn db_err(e: &sqlx::Error) -> (StatusCode, Json<serde_json::Value>) {
    tracing::error!("shares db error: {e}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

/// Confirm the caller owns the resource (admin sees any); returns the owner's id.
/// Returns 404 (not 403) for a non-owner OR an unknown type/id so a resource id's
/// existence never leaks. Share management is owner-only — a grant does NOT
/// confer the right to manage shares.
async fn require_owner(
    state: &AppState,
    ctx: &AuthContext,
    resource_type: &str,
    id: Uuid,
) -> Result<Uuid, (StatusCode, Json<serde_json::Value>)> {
    let Ok(kind) = resource_type.parse::<Shareable>() else {
        return Err(err(StatusCode::NOT_FOUND, "no such resource"));
    };
    let owner = shareable_owner(kind, id, &state.pool).await.map_err(|e| db_err(&e))?;
    match owner {
        Some(uid) if ctx.is_admin() || uid == ctx.user_id => Ok(uid),
        _ => Err(err(StatusCode::NOT_FOUND, "no such resource")),
    }
}

/// API view of one live share grant. Safe to return — no secrets, just who the
/// resource is shared with and since when.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct ShareInfo {
    pub resource_type: String,
    pub resource_id: Uuid,
    pub user_id: Uuid,
    /// The grantee's login (`users.name`), joined for display.
    pub user_name: String,
    pub action: String,
    pub granted_at: DateTime<Utc>,
}

/// `POST /api/v1/{resource_type}/{id}/shares` payload. `user` is the grantee,
/// accepted as either a UUID or a login (`users.name`). `action` defaults to
/// `use` (the only action today).
#[derive(Debug, serde::Deserialize)]
pub struct GrantShare {
    pub user: String,
    #[serde(default)]
    pub action: Option<String>,
}

/// `GET /api/v1/{resource_type}/{id}/shares` — who the resource is shared with
/// (owner-scoped). Lists only live grants.
pub async fn list_shares(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((resource_type, id)): Path<(String, Uuid)>,
) -> Result<Json<Vec<ShareInfo>>, (StatusCode, Json<serde_json::Value>)> {
    require_owner(&state, &ctx, &resource_type, id).await?;
    let rows: Vec<ShareInfo> = sqlx::query_as(
        "SELECT s.resource_type, s.resource_id, s.grantee_id AS user_id, \
                u.name AS user_name, s.action, s.granted_at \
         FROM resource_shares s JOIN users u ON u.id = s.grantee_id \
         WHERE s.resource_type = $1 AND s.resource_id = $2 AND s.revoked_at IS NULL \
         ORDER BY u.name",
    )
    .bind(&resource_type)
    .bind(id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    Ok(Json(rows))
}

/// `POST /api/v1/{resource_type}/{id}/shares` — grant `use` to another user
/// (owner-scoped). Idempotent: re-granting a revoked share un-revokes it.
pub async fn grant_share(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((resource_type, id)): Path<(String, Uuid)>,
    Json(req): Json<GrantShare>,
) -> Result<(StatusCode, Json<ShareInfo>), (StatusCode, Json<serde_json::Value>)> {
    require_owner(&state, &ctx, &resource_type, id).await?;

    // Only `use` today; reject anything else so a typo doesn't store a dead
    // action no code path honours.
    let action = req.action.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("use");
    if action != "use" {
        return Err(err(StatusCode::BAD_REQUEST, "action must be 'use'"));
    }

    let ident = req.user.trim();
    if ident.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "user required"));
    }
    let target: Option<Uuid> = Uuid::parse_str(ident)
        .map_or_else(
            |_| {
                sqlx::query_scalar("SELECT id FROM users WHERE name = $1 AND revoked_at IS NULL")
                    .bind(ident)
            },
            |uuid| {
                sqlx::query_scalar("SELECT id FROM users WHERE id = $1 AND revoked_at IS NULL")
                    .bind(uuid)
            },
        )
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| db_err(&e))?;
    let Some(target) = target else {
        return Err(err(StatusCode::NOT_FOUND, "no such user"));
    };

    sqlx::query(
        "INSERT INTO resource_shares (resource_type, resource_id, grantee_id, action) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (resource_type, resource_id, grantee_id, action) \
         DO UPDATE SET revoked_at = NULL, granted_at = now()",
    )
    .bind(&resource_type)
    .bind(id)
    .bind(target)
    .bind(action)
    .execute(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;

    let info: ShareInfo = sqlx::query_as(
        "SELECT s.resource_type, s.resource_id, s.grantee_id AS user_id, \
                u.name AS user_name, s.action, s.granted_at \
         FROM resource_shares s JOIN users u ON u.id = s.grantee_id \
         WHERE s.resource_type = $1 AND s.resource_id = $2 AND s.grantee_id = $3 AND s.action = $4",
    )
    .bind(&resource_type)
    .bind(id)
    .bind(target)
    .bind(action)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    Ok((StatusCode::CREATED, Json(info)))
}

/// `DELETE /api/v1/{resource_type}/{id}/shares/{user_id}` — revoke a grant
/// (owner-scoped) by setting `revoked_at`. 404 if there was no live share.
pub async fn revoke_share(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((resource_type, id, user_id)): Path<(String, Uuid, Uuid)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    require_owner(&state, &ctx, &resource_type, id).await?;
    let res = sqlx::query(
        "UPDATE resource_shares SET revoked_at = now() \
         WHERE resource_type = $1 AND resource_id = $2 AND grantee_id = $3 AND revoked_at IS NULL",
    )
    .bind(&resource_type)
    .bind(id)
    .bind(user_id)
    .execute(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    if res.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such share"));
    }
    Ok(StatusCode::NO_CONTENT)
}
