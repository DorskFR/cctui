//! Sessions a client socket has subscribed to, stamped in `session_viewers`
//! so a sweep on one replica sees a viewer held by another.

use sqlx::PgPool;

use crate::state::AppState;

pub const LIVE_WITHIN_SECS: f64 = 45.0;
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
pub async fn opened(state: &AppState, session_id: &str) {
    if let Err(err) = stamp(&state.pool, &state.presence.pod, &[session_id.to_owned()]).await {
        tracing::warn!(%err, %session_id, "session viewer stamp failed");
    }
}

/// The last socket on this pod left `session_id`: call on the →0 edge.
pub async fn closed(state: &AppState, session_id: &str) {
    if let Err(err) = sqlx::query("DELETE FROM session_viewers WHERE session_id = $1 AND pod = $2")
        .bind(session_id)
        .bind(&state.presence.pod)
        .execute(&state.pool)
        .await
    {
        tracing::warn!(%err, %session_id, "session viewer clear failed");
    }
}

/// Re-stamp what this pod holds, drop what it no longer holds, and reap rows
/// a crashed replica left behind.
pub async fn heartbeat(state: &AppState) {
    let held = state.bus.viewed_sessions();
    let pod = &state.presence.pod;
    if !held.is_empty()
        && let Err(err) = stamp(&state.pool, pod, &held).await
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
    .execute(&state.pool)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_viewer_counts_until_its_socket_leaves_or_its_stamp_lapses() {
        let Some(url) = crate::routes::gateway::test_db_url("session_viewers") else {
            return;
        };
        let pool = PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let sid = uuid::Uuid::new_v4().to_string();
        let ids = [sid.clone()];

        assert!(!any_viewed(&pool, &ids).await.unwrap());
        assert!(state.bus.view_watch_inc(&sid));
        opened(&state, &sid).await;
        assert!(any_viewed(&pool, &ids).await.unwrap(), "an open session is viewed");

        heartbeat(&state).await;
        assert!(any_viewed(&pool, &ids).await.unwrap(), "the heartbeat keeps a held view");

        sqlx::query(
            "UPDATE session_viewers SET seen_at = now() - interval '1 minute' \
             WHERE session_id = $1",
        )
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();
        assert!(!any_viewed(&pool, &ids).await.unwrap(), "a lapsed stamp is no viewer");
        heartbeat(&state).await;
        assert!(any_viewed(&pool, &ids).await.unwrap(), "the heartbeat re-stamps it");

        assert!(state.bus.view_watch_dec(&sid));
        closed(&state, &sid).await;
        assert!(!any_viewed(&pool, &ids).await.unwrap(), "the viewer left");

        sqlx::query("INSERT INTO session_viewers (session_id, pod) VALUES ($1, $2)")
            .bind(&sid)
            .bind(&state.presence.pod)
            .execute(&pool)
            .await
            .unwrap();
        heartbeat(&state).await;
        assert!(!any_viewed(&pool, &ids).await.unwrap(), "a row this pod no longer holds goes");
    }
}
