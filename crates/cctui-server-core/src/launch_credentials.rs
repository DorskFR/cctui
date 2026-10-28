//! Ownership checks and durable capabilities for daemon launch credential pulls.
use crate::state::CoreState;
use uuid::Uuid;

pub trait LaunchCredentialsContext: Sync {
    fn launch_state(&self) -> &CoreState;
    fn default_spawn_capability(
        &self,
    ) -> impl std::future::Future<Output = cctui_proto::api::SpawnCapability> + Send;
}

/// Check whether a daemon may resolve a session's gateway credentials.
///
/// The row belongs to `user_id`, and to `machine_id` when it names a machine. A NULL owner is
/// foreign. An `archived` session is parked and never relaunched, so it mints
/// nothing — unlike `ended`, which the caller un-sticks.
///
/// A MISSING row is the spawn-time race before register, and is allowed only
/// while nothing yet contradicts the caller: a token already minted for another
/// user means this id belongs to that user's session, and a machine key that
/// knows the id is not thereby entitled to its credential.
pub async fn gateway_env_allowed(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    machine_id: Uuid,
    session_id: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(Option<Uuid>, Option<Uuid>, Option<String>)> =
        sqlx::query_as("SELECT user_id, machine_uuid, status FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(pool)
            .await?;
    let Some((owner, machine, status)) = row else {
        let foreign_token: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM session_tokens \
                             WHERE session_id = $1 AND user_id IS NOT NULL AND user_id <> $2)",
        )
        .bind(session_id)
        .bind(user_id)
        .fetch_one(pool)
        .await?;
        return Ok(!foreign_token);
    };
    Ok(owner == Some(user_id)
        && machine.is_none_or(|m| m == machine_id)
        && status.as_deref() != Some("archived"))
}

/// The session's `CctuiAgent` capability, as recorded by the spawn/dispatch that
/// launched it.
///
/// Falls back to the durable table so a server restart does not
/// disarm a live session's spawn tool, and to the machine default so a session
/// nobody granted anything — an adopted native session, an undeclared dispatch —
/// still gets the relay. Only a lookup failure yields `None`.
pub async fn spawn_capability_for(
    state: &impl LaunchCredentialsContext,
    session_id: &str,
) -> Option<cctui_proto::api::SpawnCapability> {
    if let Some(cap) = state.launch_state().spawn_capabilities.get(session_id) {
        return Some(cap.clone());
    }
    match crate::store::spawn_capabilities::get(&state.launch_state().pool, session_id).await {
        Ok(Some(cap)) => {
            state.launch_state().spawn_capabilities.insert(session_id.to_owned(), cap.clone());
            Some(cap)
        }
        Ok(None) => grant_default(state, session_id).await,
        Err(e) => {
            tracing::error!(
                %session_id,
                error = %e,
                "spawn-capability lookup failed — CctuiAgent will be withheld from this session"
            );
            None
        }
    }
}

/// The default grant for a session launched without one. It carries no mode
/// ceiling: spawn-child then caps children at the session's live
/// `permission_mode`. A persist failure still serves the grant for this launch.
/// A policy-bound owner's session may spawn no children; a failed policy
/// lookup withholds the tool.
async fn grant_default(
    state: &impl LaunchCredentialsContext,
    session_id: &str,
) -> Option<cctui_proto::api::SpawnCapability> {
    let mut cap = state.default_spawn_capability().await;
    match crate::store::dispatch_policies::session_is_policy_bound(
        &state.launch_state().pool,
        session_id,
    )
    .await
    {
        Ok(false) => {}
        Ok(true) => cap.max_children = Some(0),
        Err(e) => {
            tracing::error!(
                %session_id,
                error = %e,
                "dispatch policy lookup failed — CctuiAgent will be withheld from this session"
            );
            return None;
        }
    }
    if let Err(e) =
        crate::store::spawn_capabilities::upsert(&state.launch_state().pool, session_id, &cap).await
    {
        tracing::error!(%session_id, error = %e, "default spawn-capability persist failed");
    }
    state.launch_state().spawn_capabilities.insert(session_id.to_owned(), cap.clone());
    Some(cap)
}

/// A foreign session is unknown rather than invalid: false would trigger a destructive worker heal.
pub async fn token_valid(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    session_id: &str,
    hash: &str,
) -> Result<Option<bool>, sqlx::Error> {
    // Only answer for sessions
    // owned by the machine's user. A foreign session 404s — NOT `valid:false`,
    // which would trigger a destructive kill + cold-resume daemon-side.
    let owner: Option<Uuid> = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = $1")
        .bind(session_id)
        .fetch_optional(pool)
        .await?;
    if owner.is_some_and(|o| o != user_id) {
        return Ok(None);
    }

    // Valid = a live token row with this hash, scoped to this session, that
    // still joins a live account (same join semantics as the gateway's
    // `resolve_account`, but by hash equality — the daemon sends the sha256
    // hex of the token it launched the worker with, never the token itself).
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM session_tokens t JOIN account_providers a ON a.id = t.account_id
            WHERE t.token_hash = $1 AND t.session_id = $2 AND t.revoked_at IS NULL
              AND (t.expires_at IS NULL OR t.expires_at > now())
         )",
    )
    .bind(hash)
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    Ok(Some(valid))
}
