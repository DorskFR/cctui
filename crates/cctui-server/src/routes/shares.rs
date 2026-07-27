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

use crate::auth::{AuthContext, Scope};
use crate::state::AppState;

/// The resource kinds that may be shared, keyed by the `resource_type` stored on
/// a `resource_shares` row. `context_pack` lands with ; it is accepted
/// here so the table/route are ready, but its owner lookup returns `None` until
/// the table exists (so no route 500s on an unknown table).
pub const SHAREABLE_TYPES: &[&str] = &["account", "machine", "dispatcher", "context_pack"];

/// Is `resource_type` a known shareable kind? Anything else is a 404 (an unknown
/// resource type never leaks as a distinct error).
#[must_use]
pub fn is_shareable(resource_type: &str) -> bool {
    SHAREABLE_TYPES.contains(&resource_type)
}

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

/// Resolve the owning user of a shareable resource by its `resource_type`. The
/// owner is derived from the resource's own row (no denormalized owner column),
/// matching `Resource::owner_of` in authz.rs. `Ok(None)` = the resource does not
/// exist (or its kind has no backing table yet) → the caller maps that to 404.
pub async fn resource_owner(
    pool: &sqlx::PgPool,
    resource_type: &str,
    id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    match resource_type {
        "account" => {
            sqlx::query_scalar("SELECT user_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await
        }
        "machine" => {
            sqlx::query_scalar("SELECT user_id FROM machines WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await
        }
        "dispatcher" => {
            sqlx::query_scalar(
                "SELECT user_id FROM dispatchers WHERE id = $1 AND deleted_at IS NULL",
            )
            .bind(id)
            .fetch_optional(pool)
            .await
        }
        // context_pack has no table yet — treat as absent so the route
        // 404s cleanly rather than erroring on a missing relation.
        _ => Ok(None),
    }
}

fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(serde_json::json!({ "error": msg })))
}

fn db_err(e: &sqlx::Error) -> (StatusCode, Json<serde_json::Value>) {
    tracing::error!("shares db error: {e}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

fn require_human(ctx: &AuthContext) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if ctx.machine_id.is_some() || !ctx.has(Scope::Read) {
        return Err(err(StatusCode::FORBIDDEN, "user or admin token required"));
    }
    Ok(())
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
    if !is_shareable(resource_type) {
        return Err(err(StatusCode::NOT_FOUND, "no such resource"));
    }
    let owner = resource_owner(&state.pool, resource_type, id).await.map_err(|e| db_err(&e))?;
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
    require_human(&ctx)?;
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
    require_human(&ctx)?;
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
    require_human(&ctx)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shareable_type_matrix() {
        for t in ["account", "machine", "dispatcher", "context_pack"] {
            assert!(is_shareable(t), "{t} should be shareable");
        }
        for t in ["", "session", "user", "prompt", "api_key", "Account"] {
            assert!(!is_shareable(t), "{t} must not be shareable");
        }
    }
}
