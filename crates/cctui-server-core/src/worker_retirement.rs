//! Retire expired worker identities while preserving historical execution records.
use crate::events;
use crate::state::CoreState;

/// Retire ephemeral workers that have been silent past their TTL.
///
/// Preserve machine rows for historical foreign keys and revoke canonical
/// machine credentials atomically, then invalidate cached authentication.
pub async fn reap_ephemeral_machines(state: &CoreState, ttl_secs: u64) {
    if ttl_secs == 0 {
        return;
    }
    let cutoff =
        chrono::Utc::now() - chrono::Duration::seconds(i64::try_from(ttl_secs).unwrap_or(i64::MAX));
    match sqlx::query_scalar::<_, uuid::Uuid>(
        "WITH retired AS ( \
            UPDATE machines SET revoked_at = COALESCE(revoked_at, now()), deleted_at = now() \
            WHERE kind = 'ephemeral' AND deleted_at IS NULL AND last_seen_at < $1 RETURNING id \
         ), revoked AS ( \
            UPDATE auth_keys SET revoked_at = COALESCE(revoked_at, now()) \
            WHERE machine_id IN (SELECT id FROM retired) RETURNING id \
         ) SELECT id FROM retired",
    )
    .bind(cutoff)
    .fetch_all(&state.pool)
    .await
    {
        Ok(ids) if !ids.is_empty() => {
            state.auth_config.purge_all();
            tracing::info!(count = ids.len(), "reaped stale ephemeral machines");
            for id in ids {
                events::record(
                    &state.pool,
                    &state.bus,
                    events::Event::new(events::kind::MACHINE_DELETED, events::Actor::Reaper)
                        .machine(id)
                        .detail(serde_json::json!({ "reason": "ephemeral_ttl" })),
                );
            }
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(%err, "ephemeral machine reap failed"),
    }
}

/// Ephemeral dispatch keys are per-session credentials handed to worker pods:
/// revoke one once its session is archived, and delete keys past `expires_at`.
pub async fn reap_ephemeral_keys(state: &CoreState) {
    match sqlx::query(
        "UPDATE auth_keys SET revoked_at = now() \
         WHERE kind = 'ephemeral' AND revoked_at IS NULL AND session_id IN \
           (SELECT id FROM sessions WHERE status = 'archived')",
    )
    .execute(&state.pool)
    .await
    {
        Ok(res) if res.rows_affected() > 0 => {
            state.auth_config.purge_all();
            tracing::info!(
                count = res.rows_affected(),
                "revoked ephemeral dispatch keys for archived sessions"
            );
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(%err, "ephemeral key revoke sweep failed"),
    }
    match sqlx::query(
        "DELETE FROM auth_keys \
         WHERE kind = 'ephemeral' AND expires_at IS NOT NULL AND expires_at < now()",
    )
    .execute(&state.pool)
    .await
    {
        Ok(res) if res.rows_affected() > 0 => {
            state.auth_config.purge_all();
            tracing::info!(count = res.rows_affected(), "deleted expired ephemeral dispatch keys");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(%err, "expired ephemeral key delete sweep failed"),
    }
}
