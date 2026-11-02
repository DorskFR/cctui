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

/// SQL: how many follow-ups continue `sessions.id`, the latest included.
macro_rules! followup_count_sql {
    () => {
        "(SELECT count(*)::int4 FROM sessions f \
          WHERE f.parent_id = sessions.id AND f.metadata->>'relation' = 'followup')"
    };
}

pub(crate) use followup_count_sql;
pub(crate) use nests_under_parent;
pub(crate) use next_session_sql;

/// The latest follow-up continuing `id`, if any.
#[cfg(test)]
pub async fn next_of(pool: &PgPool, id: &str) -> Result<Option<String>, sqlx::Error> {
    Ok(followups_of(pool, id).await?.0)
}

/// The latest follow-up continuing `id` and how many continue it in all.
pub async fn followups_of(pool: &PgPool, id: &str) -> Result<(Option<String>, u32), sqlx::Error> {
    let row: Option<(Option<String>, i32)> = sqlx::query_as(concat!(
        "SELECT ",
        next_session_sql!(),
        ", ",
        followup_count_sql!(),
        " FROM sessions WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map_or((None, 0), |(next, count)| (next, u32::try_from(count).unwrap_or(0))))
}

/// Up to the thread's root through follow-up links, then every follow-up
/// below it: depth first, siblings oldest first. `seen` breaks link cycles.
/// No status filter: archived sessions stay in the thread.
const THREAD_SQL: &str = "WITH RECURSIVE up AS ( \
        SELECT id, metadata->>'relation' AS relation, parent_id, ARRAY[id] AS seen \
          FROM sessions WHERE id = $1 \
        UNION ALL \
        SELECT p.id, p.metadata->>'relation', p.parent_id, up.seen || p.id \
          FROM up JOIN sessions p ON p.id = up.parent_id \
         WHERE up.relation = 'followup' AND NOT p.id = ANY(up.seen) \
           AND cardinality(up.seen) < 500 \
    ), root AS ( \
        SELECT id FROM up ORDER BY cardinality(seen) DESC LIMIT 1 \
    ), tree AS ( \
        SELECT s.id, NULL::text AS parent_id, \
               ARRAY[to_char(s.registered_at AT TIME ZONE 'UTC', 'YYYYMMDDHH24MISSUS') \
                     || s.id] AS path, \
               ARRAY[s.id] AS seen \
          FROM root JOIN sessions s ON s.id = root.id \
        UNION ALL \
        SELECT f.id, f.parent_id, \
               tree.path || (to_char(f.registered_at AT TIME ZONE 'UTC', 'YYYYMMDDHH24MISSUS') \
                     || f.id), \
               tree.seen || f.id \
          FROM tree JOIN sessions f ON f.parent_id = tree.id \
         WHERE f.metadata->>'relation' = 'followup' AND NOT f.id = ANY(tree.seen) \
           AND cardinality(tree.seen) < 500 \
    ) \
    SELECT s.id, s.session_name, s.working_dir, s.registered_at, t.parent_id \
      FROM tree t JOIN sessions s ON s.id = t.id \
     ORDER BY t.path";

/// `id, session_name, working_dir, registered_at, parent_id` of a thread entry.
type ThreadRow = (String, Option<String>, String, DateTime<Utc>, Option<String>);

/// The follow-up tree `id` belongs to, root first; empty when `id` is unknown.
pub async fn thread_of(pool: &PgPool, id: &str) -> Result<Vec<ThreadSession>, sqlx::Error> {
    let rows: Vec<ThreadRow> = sqlx::query_as(THREAD_SQL).bind(id).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, working_dir, registered_at, parent_id)| ThreadSession {
            id,
            name,
            working_dir,
            registered_at,
            parent_id,
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
    use super::{followups_of, next_of, parent_of, thread_of};
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

    async fn test_pool(name: &str) -> Option<(sqlx::PgPool, Uuid, Uuid)> {
        let url = crate::routes::gateway::test_db_url(name)?;
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
        Some((pool, uid, mid))
    }

    async fn age(pool: &sqlx::PgPool, id: &str, mins: i32) {
        sqlx::query(
            "UPDATE sessions SET registered_at = now() - make_interval(mins => $2) WHERE id = $1",
        )
        .bind(id)
        .bind(mins)
        .execute(pool)
        .await
        .expect("age");
    }

    async fn cleanup(pool: &sqlx::PgPool, uid: Uuid, mid: Uuid, sessions: &[String]) {
        sqlx::query("DELETE FROM sessions WHERE id = ANY($1)")
            .bind(sessions)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM machines WHERE id = $1").bind(mid).execute(pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(pool).await.ok();
    }

    fn ids(t: &[cctui_proto::api::ThreadSession]) -> Vec<&str> {
        t.iter().map(|s| s.id.as_str()).collect()
    }

    fn parents(t: &[cctui_proto::api::ThreadSession]) -> Vec<Option<&str>> {
        t.iter().map(|s| s.parent_id.as_deref()).collect()
    }

    #[tokio::test]
    async fn an_archived_session_keeps_its_previous_and_next() {
        let Some((pool, uid, mid)) = test_pool("followup_thread_archived").await else {
            return;
        };
        let root = seed(&pool, uid, mid, "archived").await;
        let older = seed(&pool, uid, mid, "archived").await;
        let fork = seed(&pool, uid, mid, "active").await;
        let middle = seed(&pool, uid, mid, "archived").await;
        let subagent = seed(&pool, uid, mid, "active").await;
        let latest = seed(&pool, uid, mid, "active").await;
        age(&pool, &root, 60).await;
        link(&pool, &older, &root, "followup", 50).await;
        link(&pool, &fork, &root, "fork", 45).await;
        link(&pool, &middle, &root, "followup", 40).await;
        link(&pool, &subagent, &middle, "subagent", 35).await;
        link(&pool, &latest, &middle, "followup", 30).await;

        let full = [root.as_str(), older.as_str(), middle.as_str(), latest.as_str()];
        for id in full {
            assert_eq!(ids(&thread_of(&pool, id).await.expect("thread")), full, "from {id}");
        }
        assert_eq!(next_of(&pool, &middle).await.expect("next"), Some(latest.clone()));
        assert_eq!(ids(&thread_of(&pool, &fork).await.expect("thread")), [fork.as_str()]);
        assert!(thread_of(&pool, "no-such-session").await.expect("thread").is_empty());

        cleanup(&pool, uid, mid, &[root, older, fork, middle, subagent, latest]).await;
    }

    #[tokio::test]
    async fn a_root_followed_up_twice_lists_both_branches() {
        let Some((pool, uid, mid)) = test_pool("followup_thread_forks").await else {
            return;
        };
        let r = seed(&pool, uid, mid, "archived").await;
        let a = seed(&pool, uid, mid, "archived").await;
        let a2 = seed(&pool, uid, mid, "active").await;
        let b = seed(&pool, uid, mid, "active").await;
        age(&pool, &r, 60).await;
        link(&pool, &a, &r, "followup", 50).await;
        link(&pool, &a2, &a, "followup", 45).await;
        link(&pool, &b, &r, "followup", 40).await;

        let order = [r.as_str(), a.as_str(), a2.as_str(), b.as_str()];
        for id in order {
            let thread = thread_of(&pool, id).await.expect("thread");
            assert_eq!(ids(&thread), order, "from {id}");
            assert_eq!(
                parents(&thread),
                [None, Some(r.as_str()), Some(a.as_str()), Some(r.as_str())],
                "A and B are siblings under R, A2 under A, from {id}"
            );
        }
        assert_eq!(
            followups_of(&pool, &r).await.expect("followups"),
            (Some(b.clone()), 2),
            "R still continues in the latest, B, and counts both"
        );
        assert_eq!(next_of(&pool, &r).await.expect("next"), Some(b.clone()));
        assert_eq!(followups_of(&pool, &a).await.expect("followups"), (Some(a2.clone()), 1));
        assert_eq!(followups_of(&pool, &b).await.expect("followups"), (None, 0));
        assert_eq!(followups_of(&pool, "no-such-session").await.expect("followups"), (None, 0));

        cleanup(&pool, uid, mid, &[r, a, a2, b]).await;
    }
}
