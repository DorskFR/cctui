use std::collections::BTreeMap;

use axum::http::StatusCode;
use uuid::Uuid;

use super::{acting_user, mint_env_for_account};
use crate::error::AppError;
use crate::state::AppState;

/// Bind a fork before dispatch, with fresh child-scoped tokens for every bound
/// family. Revoked parent tokens still identify an archived parent's account;
/// they are never copied or revived. An unbound parent stays unbound.
pub async fn fork_env_for_session(
    state: &AppState,
    parent: &str,
    child: &str,
) -> Result<BTreeMap<String, String>, AppError> {
    let bindings: Vec<(Uuid, Option<String>, Option<Uuid>)> = sqlx::query_as(
        "SELECT DISTINCT ON (ap.family) b.account_id, ap.provider, b.pool_id \
         FROM ( \
           SELECT account_id, pool_id, revoked_at, created_at FROM session_tokens \
           WHERE session_id = $1 \
           UNION ALL \
           SELECT account_id::uuid, NULL::uuid, now(), registered_at FROM sessions \
           WHERE id = $1 AND account_id IS NOT NULL \
             AND NOT EXISTS (SELECT 1 FROM session_tokens WHERE session_id = $1) \
         ) b LEFT JOIN account_providers ap ON ap.id = b.account_id \
         ORDER BY ap.family, (b.revoked_at IS NULL) DESC, b.created_at DESC",
    )
    .bind(parent)
    .fetch_all(&state.pool)
    .await?;
    let acting = acting_user(state, parent, None).await;
    let mut env = BTreeMap::new();
    for (provider_id, provider, pool_id) in bindings {
        let Some(provider) = provider else {
            return Err(AppError::new(
                StatusCode::CONFLICT,
                "the parent's account no longer exists",
            ));
        };
        let minted = mint_env_for_account(state, provider_id, &provider, child, acting).await?;
        if minted.is_empty() {
            return Err(AppError::new(
                StatusCode::CONFLICT,
                "the parent's account is no longer available to fork",
            ));
        }
        sqlx::query(
            "UPDATE session_tokens SET pool_id = $3 \
             WHERE session_id = $1 AND account_id = $2 AND revoked_at IS NULL",
        )
        .bind(child)
        .bind(provider_id)
        .bind(pool_id)
        .execute(&state.pool)
        .await?;
        env.extend(minted);
    }
    Ok(env)
}
