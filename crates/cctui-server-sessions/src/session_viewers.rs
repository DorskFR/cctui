//! Sessions a client socket has subscribed to, stamped in `session_viewers`
//! so a sweep on one replica sees a viewer held by another.

use sqlx::PgPool;

use crate::context::SessionsContext;

pub use cctui_server_kernel::sessions_types::LIVE_WITHIN_SECS;
pub const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(15);
const REAP_AFTER_SECS: f64 = 600.0;

async fn stamp(pool: &PgPool, pod: &str, sessions: &[String]) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO session_viewers (session_id, pod, seen_at) \
         SELECT unnest($1::text[]), $2, now() \
         ON CONFLICT (session_id, pod) DO UPDATE SET seen_at = now()",
    )
    .bind(sessions)
    .bind(pod)
    .execute(pool)
    .await?;
    Ok(())
}

/// A socket on this pod opened `session_id`: call on the 0→1 edge.
pub async fn opened<S: SessionsContext>(state: &S, session_id: &str) {
    if let Err(err) = stamp(state.pool(), state.presence_pod(), &[session_id.to_owned()]).await {
        tracing::warn!(%err, %session_id, "session viewer stamp failed");
    }
}

/// The last socket on this pod left `session_id`: call on the →0 edge.
pub async fn closed<S: SessionsContext>(state: &S, session_id: &str) {
    if let Err(err) = sqlx::query("DELETE FROM session_viewers WHERE session_id = $1 AND pod = $2")
        .bind(session_id)
        .bind(state.presence_pod())
        .execute(state.pool())
        .await
    {
        tracing::warn!(%err, %session_id, "session viewer clear failed");
    }
}

/// Re-stamp what this pod holds, drop what it no longer holds, and reap rows
/// a crashed replica left behind.
pub async fn heartbeat<S: SessionsContext>(state: &S) {
    let held = state.bus().viewed_sessions();
    let pod = state.presence_pod();
    if !held.is_empty()
        && let Err(err) = stamp(state.pool(), pod, &held).await
    {
        tracing::warn!(%err, "session viewer heartbeat failed");
    }
    if let Err(err) = sqlx::query(
        "DELETE FROM session_viewers \
         WHERE (pod = $1 AND session_id <> ALL($2)) \
            OR seen_at < now() - make_interval(secs => $3)",
    )
    .bind(pod)
    .bind(&held)
    .bind(REAP_AFTER_SECS)
    .execute(state.pool())
    .await
    {
        tracing::warn!(%err, "session viewer prune failed");
    }
}

/// Whether any of `sessions` is open in a client on any replica.
pub async fn any_viewed(pool: &PgPool, sessions: &[String]) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM session_viewers \
                        WHERE session_id = ANY($1) \
                          AND seen_at > now() - make_interval(secs => $2))",
    )
    .bind(sessions)
    .bind(LIVE_WITHIN_SECS)
    .fetch_one(pool)
    .await
}
