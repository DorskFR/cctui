//! Follow-up sessions: a spawn whose first prompt embeds the parent's
//! transcript brief. The parent link is remembered under the spawn key (like
//! labels and auto-archive) and claimed when the worker registers; a daemon
//! announcing it as a root never takes the relation back.

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use cctui_proto::api::{SessionThreadResponse, SpawnRequest, ThreadSession};

use crate::error::AppError;
use crate::state::AppState;

pub const RELATION: &str = "followup";

/// SQL: the session nests under its `parent_id` as a child. A fork or a
/// follow-up only links to the session it came from; it is never its child.
macro_rules! nests_under_parent {
    ($alias:literal) => {
        concat!("COALESCE(", $alias, ".metadata->>'relation', '') NOT IN ('fork', 'followup')")
    };
}

/// SQL: the latest follow-up continuing `sessions.id`.
macro_rules! next_session_sql {
    () => {
        "(SELECT f.id FROM sessions f \
          WHERE f.parent_id = sessions.id AND f.metadata->>'relation' = 'followup' \
          ORDER BY f.registered_at DESC LIMIT 1)"
    };
}

pub(crate) use {nests_under_parent, next_session_sql};

/// The latest follow-up continuing `id`, if any.
pub async fn next_of(pool: &PgPool, id: &str) -> Result<Option<String>, sqlx::Error> {
    let next: Option<Option<String>> =
        sqlx::query_scalar(concat!("SELECT ", next_session_sql!(), " FROM sessions WHERE id = $1"))
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(next.flatten())
}

/// Follow-up ancestors up, then the latest follow-up down as in
/// `next_session_sql!`. No status filter: archived sessions stay in the thread.
const THREAD_SQL: &str = "WITH RECURSIVE up AS ( \
        SELECT id, parent_id, metadata->>'relation' AS relation, 0 AS depth \
          FROM sessions WHERE id = $1 \
        UNION ALL \
        SELECT p.id, p.parent_id, p.metadata->>'relation', up.depth - 1 \
          FROM up JOIN sessions p ON p.id = up.parent_id \
         WHERE up.relation = 'followup' AND up.depth > -500 \
    ), down AS ( \
        SELECT id, 0 AS depth FROM sessions WHERE id = $1 \
        UNION ALL \
        SELECT n.id, down.depth + 1 FROM down CROSS JOIN LATERAL ( \
            SELECT f.id FROM sessions f \
             WHERE f.parent_id = down.id AND f.metadata->>'relation' = 'followup' \
             ORDER BY f.registered_at DESC LIMIT 1) n \
         WHERE down.depth < 500 \
    ) \
    SELECT s.id, s.session_name, s.working_dir, s.registered_at \
      FROM (SELECT id, depth FROM up UNION SELECT id, depth FROM down) t \
      JOIN sessions s ON s.id = t.id \
     ORDER BY t.depth";

/// The follow-up thread through `id`, oldest first; empty when `id` is unknown.
pub async fn thread_of(pool: &PgPool, id: &str) -> Result<Vec<ThreadSession>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, String, DateTime<Utc>)> =
        sqlx::query_as(THREAD_SQL).bind(id).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, working_dir, registered_at)| ThreadSession {
            id,
            name,
            working_dir,
            registered_at,
        })
        .collect())
}

/// `GET /sessions/{id}/thread`.
pub async fn session_thread(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionThreadResponse>, AppError> {
    Ok(Json(SessionThreadResponse { sessions: thread_of(&state.pool, &session_id).await? }))
}

/// Unclaimed intents older than this are dropped: the spawn never registered.
const INTENT_TTL_SECS: i64 = 6 * 3600;

/// The parent a follow-up spawn continues, if the request is one.
#[must_use]
pub fn parent_of(req: &SpawnRequest) -> Option<&str> {
    if req.relation.as_deref() != Some(RELATION) {
        return None;
    }
    req.parent_session_id.as_deref().map(str::trim).filter(|p| !p.is_empty())
}

/// Remember which session the spawn under `spawn_key` follows up on.
/// Best-effort: a lost intent leaves the session unlinked.
pub async fn remember_intent(pool: &PgPool, spawn_key: &str, req: &SpawnRequest) {
    let Some(parent) = parent_of(req) else { return };
    if let Err(e) = sqlx::query(
        "INSERT INTO session_followup_intents (spawn_key, parent_session_id) VALUES ($1, $2) \
         ON CONFLICT (spawn_key) DO UPDATE SET parent_session_id = EXCLUDED.parent_session_id",
    )
    .bind(spawn_key)
    .bind(parent)
    .execute(pool)
    .await
    {
        tracing::warn!(%spawn_key, error = %e, "could not record the follow-up intent");
    }
}

/// Link the session that just registered to the parent a pending intent
/// names. Looked up by its own id and by the spawn key the adapter echoed, so
/// both pre-minted (claude-code) and self-minted (codex) ids resolve. The
/// parent must exist (FK), else the link is skipped.
pub async fn claim_intent(pool: &PgPool, session_id: &str, spawn_key: Option<&str>) {
    let key = spawn_key.unwrap_or(session_id);
    let parent = match sqlx::query_scalar::<_, String>(
        "DELETE FROM session_followup_intents WHERE spawn_key = $1 OR spawn_key = $2 \
         RETURNING parent_session_id",
    )
    .bind(session_id)
    .bind(key)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(parent)) => parent,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "follow-up intent lookup failed");
            return;
        }
    };
    if let Err(e) = sqlx::query(
        "UPDATE sessions SET \
             parent_id = COALESCE(parent_id, (SELECT id FROM sessions WHERE id = $2)), \
             metadata = COALESCE(metadata, '{}'::jsonb) || jsonb_build_object('relation', $3::text) \
         WHERE id = $1",
    )
    .bind(session_id)
    .bind(&parent)
    .bind(RELATION)
    .execute(pool)
    .await
    {
        tracing::warn!(%session_id, %parent, error = %e, "could not link the follow-up session");
    }
}

/// Reaper tick: drop intents whose spawn never registered.
pub async fn sweep(pool: &PgPool) {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(INTENT_TTL_SECS);
    if let Err(e) = sqlx::query("DELETE FROM session_followup_intents WHERE created_at < $1")
        .bind(cutoff)
        .execute(pool)
        .await
    {
        tracing::warn!(error = %e, "follow-up intent prune failed");
    }
}

#[cfg(test)]
mod tests {
    use super::{next_of, parent_of, thread_of};
    use cctui_proto::api::SpawnRequest;
    use uuid::Uuid;

    fn req(relation: Option<&str>, parent: Option<&str>) -> SpawnRequest {
        let mut r: SpawnRequest =
            serde_json::from_value(serde_json::json!({"machine_id": "m", "working_dir": "/w"}))
                .unwrap();
        r.relation = relation.map(str::to_owned);
        r.parent_session_id = parent.map(str::to_owned);
        r
    }

    #[test]
    fn a_followup_names_its_parent() {
        assert_eq!(parent_of(&req(Some("followup"), Some(" p1 "))), Some("p1"));
    }

    #[test]
    fn other_relations_and_blank_parents_do_not_link() {
        assert_eq!(parent_of(&req(None, Some("p1"))), None);
        assert_eq!(parent_of(&req(Some("fork"), Some("p1"))), None);
        assert_eq!(parent_of(&req(Some("followup"), Some("  "))), None);
        assert_eq!(parent_of(&req(Some("followup"), None)), None);
    }

    #[test]
    fn spawn_request_decodes_without_the_new_fields() {
        let r: SpawnRequest =
            serde_json::from_value(serde_json::json!({"machine_id": "m", "working_dir": "/w"}))
                .unwrap();
        assert!(r.relation.is_none() && r.parent_session_id.is_none());
    }

    async fn seed(pool: &sqlx::PgPool, uid: Uuid, mid: Uuid, status: &str) -> String {
        let sid = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, working_dir, status, user_id, adapter_id) \
             VALUES ($1, $2, $3, '/w', $4, $5, 'claude-code')",
        )
        .bind(&sid)
        .bind(mid.to_string())
        .bind(mid)
        .bind(status)
        .bind(uid)
        .execute(pool)
        .await
        .expect("seed session");
        sid
    }

    async fn link(pool: &sqlx::PgPool, id: &str, parent: &str, relation: &str, age: i32) {
        sqlx::query(
            "UPDATE sessions SET parent_id = $2, metadata = jsonb_build_object('relation', $3::text), \
             registered_at = now() - make_interval(mins => $4) WHERE id = $1",
        )
        .bind(id)
        .bind(parent)
        .bind(relation)
        .bind(age)
        .execute(pool)
        .await
        .expect("link");
    }

    #[tokio::test]
    async fn an_archived_session_keeps_its_previous_and_next() {
        let Some(url) = crate::routes::gateway::test_db_url("followup_thread_archived") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, mid) = (Uuid::new_v4(), Uuid::new_v4());
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("thread-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(mid)
            .bind(uid)
            .bind(format!("m-{mid}"))
            .bind(format!("mk-{mid}"))
            .execute(&pool)
            .await
            .expect("seed machine");
        let root = seed(&pool, uid, mid, "archived").await;
        let older = seed(&pool, uid, mid, "archived").await;
        let fork = seed(&pool, uid, mid, "active").await;
        let middle = seed(&pool, uid, mid, "archived").await;
        let subagent = seed(&pool, uid, mid, "active").await;
        let latest = seed(&pool, uid, mid, "active").await;
        sqlx::query("UPDATE sessions SET registered_at = now() - interval '1 hour' WHERE id = $1")
            .bind(&root)
            .execute(&pool)
            .await
            .expect("age root");
        link(&pool, &older, &root, "followup", 50).await;
        link(&pool, &fork, &root, "fork", 45).await;
        link(&pool, &middle, &root, "followup", 40).await;
        link(&pool, &subagent, &middle, "subagent", 35).await;
        link(&pool, &latest, &middle, "followup", 30).await;

        let ids = |t: Vec<cctui_proto::api::ThreadSession>| {
            t.into_iter().map(|s| s.id).collect::<Vec<_>>()
        };
        let full = vec![root.clone(), middle.clone(), latest.clone()];
        for id in [&root, &middle, &latest] {
            assert_eq!(ids(thread_of(&pool, id).await.expect("thread")), full, "from {id}");
        }
        assert_eq!(next_of(&pool, &middle).await.expect("next"), Some(latest.clone()));
        assert_eq!(
            ids(thread_of(&pool, &older).await.expect("thread")),
            vec![root.clone(), older.clone()],
            "an earlier follow-up leads back to its source only"
        );
        assert_eq!(ids(thread_of(&pool, &fork).await.expect("thread")), vec![fork.clone()]);
        assert!(thread_of(&pool, "no-such-session").await.expect("thread").is_empty());

        let all = [root, older, fork, middle, subagent, latest];
        sqlx::query("DELETE FROM sessions WHERE id = ANY($1)")
            .bind(&all[..])
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM machines WHERE id = $1").bind(mid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}
