pub use cctui_server_core::gateway::mint::*;

use crate::state::AppState;
use cctui_proto::ids::{SessionId, SpawnKey};

impl MintContext for AppState {
    fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }
    fn vault_key(&self) -> Vec<u8> {
        crate::crypto::vault_key()
    }
    fn external_url(&self) -> &str {
        &self.config.external_url
    }
    fn clear_orphan_fingerprint(&self, fingerprint: &str) {
        super::clear_orphan_fingerprint(&self.gateway_orphan_spam, fingerprint);
    }
}

/// Revoke every session token bound to a session — called when a
/// session ends so the gateway can no longer be used under that token.
pub async fn revoke_session_tokens(state: &AppState, session_id: &str) {
    if let Err(e) = crate::store::tokens::revoke_by_session(&state.pool, session_id).await {
        tracing::error!(
            %session_id,
            error = %e,
            "failed to revoke session tokens — a live gateway credential may remain usable"
        );
    }
    state.spawn_capabilities.remove(session_id);
    state.session_usd_budgets.remove(session_id);
    if let Err(e) = crate::store::spawn_capabilities::delete(&state.pool, session_id).await {
        tracing::warn!(%session_id, error = %e, "spawn-capability cleanup failed");
    }
}

pub fn needs_rebind(spawn_key: &str, session_id: &str) -> bool {
    !spawn_key.is_empty() && !session_id.is_empty() && spawn_key != session_id
}

/// Re-key a session's gateway token, and anything already metered under it, from
/// the spawn key onto the id the harness registered under. An adapter whose
/// harness names its own session (opencode returns `ses_…`) pulls gateway env
/// under the spawn key, binding the token to an id no later lookup uses. Adapters
/// whose local id IS the spawn key pass equal ids and no-op.
pub async fn rebind_spawn_key(state: &AppState, spawn_key: SpawnKey, session_id: SessionId) {
    let (spawn_key, session_id) = (spawn_key.as_str(), session_id.as_str());
    if !needs_rebind(spawn_key, session_id) {
        return;
    }
    if let Err(e) =
        crate::store::tokens::rebind_session_id(&state.pool, spawn_key, session_id).await
    {
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
    if let Some((_, budget)) = state.session_usd_budgets.remove(spawn_key) {
        state.session_usd_budgets.insert(session_id.to_owned(), budget);
    }
    if let Some((_, cap)) = state.spawn_capabilities.remove(spawn_key) {
        state.spawn_capabilities.insert(session_id.to_owned(), cap);
    }
    if let Err(e) =
        crate::store::spawn_capabilities::rebind(&state.pool, spawn_key, session_id).await
    {
        tracing::error!(%spawn_key, %session_id, error = %e, "spawn-capability rebind failed");
    }
}
