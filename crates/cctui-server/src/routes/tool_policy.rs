//! `/accounts/{id}/tool-policy` — the gateway tool-call policy of an account.
//! Only its owner (or an admin) reads or edits it; it is never sent to agents
//! or daemons.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use uuid::Uuid;

use super::gateway::toolguard::{ToolPolicy, invalidate_policy_cache};
use crate::auth::AuthContext;
use crate::error::err;
use crate::state::AppState;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn db_err(e: &sqlx::Error) -> ApiErr {
    tracing::error!("db error: {e}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

async fn owned(state: &AppState, ctx: &AuthContext, id: Uuid) -> Result<(), ApiErr> {
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    found.map(|_| ()).ok_or_else(|| err(StatusCode::NOT_FOUND, "no such account"))
}

pub async fn get_tool_policy(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<ToolPolicy>, ApiErr> {
    owned(&state, &ctx, id).await?;
    let row: Option<(Vec<String>, Vec<String>, Vec<String>, Vec<String>)> = sqlx::query_as(
        "SELECT terms, patterns, protected_owners, exempt_roots \
         FROM account_tool_policies WHERE account_id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    let policy = row.map_or_else(ToolPolicy::default, |(terms, patterns, owners, roots)| {
        ToolPolicy { terms, patterns, protected_owners: owners, exempt_roots: roots }
    });
    Ok(Json(policy))
}

pub async fn put_tool_policy(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<ToolPolicy>,
) -> Result<Json<ToolPolicy>, ApiErr> {
    owned(&state, &ctx, id).await?;
    let policy = req.normalized().map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
    if policy == ToolPolicy::default() {
        sqlx::query("DELETE FROM account_tool_policies WHERE account_id = $1")
            .bind(id)
            .execute(&state.pool)
            .await
            .map_err(|e| db_err(&e))?;
    } else {
        sqlx::query(
            "INSERT INTO account_tool_policies \
                 (account_id, terms, patterns, protected_owners, exempt_roots, updated_at) \
             VALUES ($1, $2, $3, $4, $5, now()) \
             ON CONFLICT (account_id) DO UPDATE SET terms = EXCLUDED.terms, \
                 patterns = EXCLUDED.patterns, protected_owners = EXCLUDED.protected_owners, \
                 exempt_roots = EXCLUDED.exempt_roots, updated_at = now()",
        )
        .bind(id)
        .bind(&policy.terms)
        .bind(&policy.patterns)
        .bind(&policy.protected_owners)
        .bind(&policy.exempt_roots)
        .execute(&state.pool)
        .await
        .map_err(|e| db_err(&e))?;
    }
    invalidate_policy_cache(id);
    Ok(Json(policy))
}
