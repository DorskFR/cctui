//! Provider reauthentication flags shared by all execution hosts.

use super::credentials::CredentialContext;
use uuid::Uuid;

/// Persist the reason an upstream credential needs reauthentication.
///
/// Only the false-to-true cache transition schedules a write, preventing a
/// flapping worker from rewriting the row on every rejected request.
pub fn flag_account_reauth(state: &impl CredentialContext, account_id: Uuid, reason: &str) {
    if state.account_reauth().insert(account_id, ()).is_some() {
        return; // already flagged in memory — no redundant write
    }
    let pool = state.pool().clone();
    let reason = reason.to_string();
    tokio::spawn(async move {
        if let Err(e) = sqlx::query(
            "UPDATE account_providers \
                SET needs_reauth = true, last_auth_error = $2, last_auth_error_at = now() \
             WHERE id = $1",
        )
        .bind(account_id)
        .bind(reason)
        .execute(&pool)
        .await
        {
            tracing::warn!(account = %account_id, error = %e, "failed to flag account reauth");
        }
    });
}

/// Clear an account's reauth flag after a successful upstream call.
/// Gated on the in-memory set so the common case (account healthy) costs nothing;
/// the DB write fires only on the true→false transition.
pub fn clear_account_reauth(state: &impl CredentialContext, account_id: Uuid) {
    if state.account_reauth().remove(&account_id).is_none() {
        return; // not flagged — nothing to clear
    }
    let pool = state.pool().clone();
    tokio::spawn(async move {
        if let Err(e) = sqlx::query(
            "UPDATE account_providers \
                SET needs_reauth = false, last_auth_error = NULL, last_auth_error_at = NULL \
             WHERE id = $1 AND needs_reauth",
        )
        .bind(account_id)
        .execute(&pool)
        .await
        {
            tracing::warn!(account = %account_id, error = %e, "failed to clear account reauth");
        }
    });
}
