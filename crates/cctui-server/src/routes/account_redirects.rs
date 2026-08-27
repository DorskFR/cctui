//! `/api/v1/redirects` — expiring, launch-time account/model overrides.
//!
//! A rule either points every *new* session that asks for an account at
//! another account (`to_account`), or flips the model such a session spawns
//! with (`to_model`) — exactly one of the two. Applied where launches resolve
//! account names (`gateway::mint`); live sessions are never touched.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::accounts::{err, require_human, resolve_owner};
use crate::auth::AuthContext;
use crate::state::AppState;
use crate::store::account_redirects::{self, AccountRedirect};

type ApiErr = (StatusCode, Json<serde_json::Value>);

#[derive(serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PutRedirectRequest {
    #[ts(type = "string | null", optional)]
    pub to_account: Option<Uuid>,
    #[ts(optional)]
    pub to_model: Option<String>,
    pub family: String,
    #[ts(optional)]
    pub match_model: Option<String>,
    #[ts(type = "string | null", optional)]
    pub until: Option<DateTime<Utc>>,
    #[ts(optional)]
    pub reason: Option<String>,
    /// The admin token has no user identity and must name the rule's owner.
    #[ts(type = "string | null", optional)]
    pub user_id: Option<Uuid>,
}

/// `GET /redirects` — the caller's live rules (every user's for the admin
/// token).
pub async fn list_redirects(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<AccountRedirect>>, ApiErr> {
    require_human(&ctx)?;
    let rules = if ctx.is_admin() {
        account_redirects::live_all(&state.pool).await
    } else {
        account_redirects::live_for_user(&state.pool, ctx.user_id).await
    }
    .map_err(|e| {
        tracing::error!("listing account redirects: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "could not list redirects")
    })?;
    Ok(Json(rules))
}

/// `PUT /accounts/{id}/redirect` — create or overwrite the rule for
/// `(owner, account, family, match_model)`.
pub async fn put_redirect(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(from_account): Path<Uuid>,
    Json(req): Json<PutRedirectRequest>,
) -> Result<Json<AccountRedirect>, ApiErr> {
    require_human(&ctx)?;
    let owner = resolve_owner(&ctx, req.user_id)?;

    if !matches!(req.family.as_str(), "anthropic" | "openai" | "fireworks") {
        return Err(err(StatusCode::BAD_REQUEST, "unknown provider family"));
    }
    let to_model = req.to_model.as_deref().map(str::trim).filter(|m| !m.is_empty());
    match (req.to_account, to_model) {
        (Some(_), None) | (None, Some(_)) => {}
        _ => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "set exactly one of to_account (move new sessions) or to_model (flip the model)",
            ));
        }
    }
    if req.match_model.is_some() && to_model.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "match_model requires to_model"));
    }
    if req.to_account == Some(from_account) {
        return Err(err(StatusCode::BAD_REQUEST, "a redirect cannot target its own account"));
    }
    if let Some(until) = req.until
        && until <= Utc::now()
    {
        return Err(err(StatusCode::BAD_REQUEST, "until is in the past"));
    }

    if !account_usable(&state, owner, from_account).await {
        return Err(err(StatusCode::NOT_FOUND, "no such account"));
    }
    if let Some(target) = req.to_account {
        if !account_usable(&state, owner, target).await {
            return Err(err(StatusCode::NOT_FOUND, "target account not usable by this user"));
        }
        if !has_family_provider(&state, target, &req.family).await {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "target account has no provider for this family",
            ));
        }
    }

    let rule = account_redirects::upsert(
        &state.pool,
        account_redirects::NewRedirect {
            user_id: owner,
            from_account,
            to_account: req.to_account,
            family: &req.family,
            match_model: req.match_model.as_deref().map(str::trim).filter(|m| !m.is_empty()),
            to_model,
            expires_at: req.until,
            reason: req.reason.as_deref(),
        },
    )
    .await
    .map_err(|e| {
        tracing::error!("upserting account redirect: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "could not save redirect")
    })?;
    Ok(Json(rule))
}

/// `DELETE /redirects/{id}` — owner-scoped; the admin token may delete any.
pub async fn delete_redirect(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiErr> {
    require_human(&ctx)?;
    let owner = if ctx.is_admin() { None } else { Some(ctx.user_id) };
    let gone = account_redirects::delete(&state.pool, id, owner).await.map_err(|e| {
        tracing::error!("deleting account redirect: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "could not delete redirect")
    })?;
    if gone { Ok(StatusCode::NO_CONTENT) } else { Err(err(StatusCode::NOT_FOUND, "no such rule")) }
}

/// The same owned-or-shared predicate the launch-path name lookup applies.
async fn account_usable(state: &AppState, user_id: Uuid, account_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM accounts a \
          WHERE a.id = $2 \
            AND (a.user_id = $1 OR EXISTS ( \
                SELECT 1 FROM resource_shares s \
                 WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                   AND s.grantee_id = $1 AND s.revoked_at IS NULL)))",
    )
    .bind(user_id)
    .bind(account_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(false)
}

async fn has_family_provider(state: &AppState, account_id: Uuid, family: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM account_providers \
          WHERE account_id = $1 AND family = $2)",
    )
    .bind(account_id)
    .bind(family)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(false)
}
