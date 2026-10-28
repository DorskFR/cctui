//! Session credential revocation and spawn-key rebinding for execution hosts.
use cctui_proto::ids::{SessionId, SpawnKey};

/// Execution maps plus a host hook for records outside core.
pub trait SessionCredentialContext: Sync {
    fn token_pool(&self) -> &sqlx::PgPool;
    fn spawn_capabilities(&self) -> &dashmap::DashMap<String, cctui_proto::api::SpawnCapability>;
    fn session_budgets(&self) -> &dashmap::DashMap<String, f64>;
    fn after_execution_rebind(
        &self,
        spawn: &str,
        session: &str,
    ) -> impl std::future::Future<Output = Result<(), sqlx::Error>> + Send;
}

async fn rebind_records(
    state: &impl SessionCredentialContext,
    spawn: &str,
    session: &str,
) -> Result<(), sqlx::Error> {
    crate::store::tokens::rebind_session_id(state.token_pool(), spawn, session).await?;
    state.after_execution_rebind(spawn, session).await
}

/// Revoke every session token bound to a session — called when a
/// session ends so the gateway can no longer be used under that token.
pub async fn revoke_session_tokens(state: &impl SessionCredentialContext, session_id: &str) {
    if let Err(e) = crate::store::tokens::revoke_by_session(state.token_pool(), session_id).await {
        tracing::error!(
            %session_id,
            error = %e,
            "failed to revoke session tokens — a live gateway credential may remain usable"
        );
    }
    state.spawn_capabilities().remove(session_id);
    state.session_budgets().remove(session_id);
    if let Err(e) = crate::store::spawn_capabilities::delete(state.token_pool(), session_id).await {
        tracing::warn!(%session_id, error = %e, "spawn-capability cleanup failed");
    }
}

#[must_use]
pub fn needs_rebind(spawn_key: &str, session_id: &str) -> bool {
    !spawn_key.is_empty() && !session_id.is_empty() && spawn_key != session_id
}

/// Re-key a session's gateway token, and anything already metered under it, from
/// the spawn key onto the id the harness registered under.
///
/// An adapter whose
/// harness names its own session (opencode returns `ses_…`) pulls gateway env
/// under the spawn key, binding the token to an id no later lookup uses. Adapters
/// whose local id IS the spawn key pass equal ids and no-op.
pub async fn rebind_spawn_key(
    state: &impl SessionCredentialContext,
    spawn_key: SpawnKey,
    session_id: SessionId,
) {
    let (spawn_key, session_id) = (spawn_key.as_str(), session_id.as_str());
    if !needs_rebind(spawn_key, session_id) {
        return;
    }
    if let Err(e) = rebind_records(state, spawn_key, session_id).await {
        tracing::error!(
            %spawn_key,
            %session_id,
            error = %e,
            "spawn-key rebind failed — this session's usage will not be metered"
        );
        return;
    }
    // The in-memory maps are keyed the same way the token was, so they have to
    // move too: a budget or capability left under the spawn key is invisible to
    // every later lookup, which goes through the rebound id.
    if let Some((_, budget)) = state.session_budgets().remove(spawn_key) {
        state.session_budgets().insert(session_id.to_owned(), budget);
    }
    if let Some((_, cap)) = state.spawn_capabilities().remove(spawn_key) {
        state.spawn_capabilities().insert(session_id.to_owned(), cap);
    }
    if let Err(e) =
        crate::store::spawn_capabilities::rebind(state.token_pool(), spawn_key, session_id).await
    {
        tracing::error!(%spawn_key, %session_id, error = %e, "spawn-capability rebind failed");
    }
}
