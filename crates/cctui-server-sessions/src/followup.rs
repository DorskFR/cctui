//! Follow-up sessions: a spawn whose first prompt embeds the parent's
//! transcript brief.
//!
//! The parent link is remembered under the spawn key (like
//! labels and auto-archive) and claimed when the worker registers; a daemon
//! announcing it as a root never takes the relation back.

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use cctui_proto::api::{BriefResponse, SessionThreadResponse, SpawnRequest, ThreadSession};

use crate::brief;
use crate::context::SessionsContext;
use crate::error::AppError;

#[path = "followup_thread.rs"]
pub mod thread;

pub const RELATION: &str = "followup";

/// SQL: the session nests under its `parent_id` as a child. A fork or a
/// follow-up only links to the session it came from; it is never its child.
macro_rules! nests_under_parent {
    ($alias:literal) => {
        concat!("COALESCE(", $alias, ".metadata->>'relation', '') NOT IN ('fork', 'followup')")
    };
}

/// SQL: the model session `s` was launched with. The harness reports it
/// without its `[1m]` suffix. Claude pre-mints its id, so the launch row's
/// `session_id` is the session's.
macro_rules! launched_model_sql {
    () => {
        "(SELECT ml.model FROM machine_launches ml WHERE ml.session_id = s.id)"
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
pub(crate) use launched_model_sql;
pub(crate) use nests_under_parent;
pub(crate) use next_session_sql;

/// The latest follow-up continuing `id`, if any.
#[cfg(any(test, feature = "test-support"))]
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

/// SQL: `$1` and its ancestors through follow-up links, each step also meeting
/// `$step` (on `p`, the parent, and `up`); `seen` breaks link cycles, the
/// longest `seen` is the root.
macro_rules! up_cte_sql {
    ($step:literal) => {
        concat!(
        "up AS ( \
            SELECT id, user_id, metadata->>'relation' AS relation, parent_id, ARRAY[id] AS seen \
              FROM sessions WHERE id = $1 \
            UNION ALL \
            SELECT p.id, p.user_id, p.metadata->>'relation', p.parent_id, up.seen || p.id \
              FROM up JOIN sessions p ON p.id = up.parent_id \
             WHERE up.relation = 'followup' AND NOT p.id = ANY(up.seen) ",
        $step,
        " AND cardinality(up.seen) < 500 \
        )")
    };
}

/// Up to the thread's root through follow-up links, then every follow-up
/// below it: depth first, siblings oldest first. Only `$1`'s owner's sessions,
/// since the route shows each entry's name and directory unchecked.
/// No status filter: archived sessions stay in the thread.
const THREAD_SQL: &str = concat!(
    "WITH RECURSIVE ",
    up_cte_sql!("AND p.user_id IS NOT DISTINCT FROM up.user_id"),
    ", root AS ( \
        SELECT id, user_id FROM up ORDER BY cardinality(seen) DESC LIMIT 1 \
    ), tree AS ( \
        SELECT s.id, s.user_id, NULL::text AS parent_id, \
               ARRAY[to_char(s.registered_at AT TIME ZONE 'UTC', 'YYYYMMDDHH24MISSUS') \
                     || s.id] AS path, \
               ARRAY[s.id] AS seen \
          FROM root JOIN sessions s ON s.id = root.id \
        UNION ALL \
        SELECT f.id, f.user_id, f.parent_id, \
               tree.path || (to_char(f.registered_at AT TIME ZONE 'UTC', 'YYYYMMDDHH24MISSUS') \
                     || f.id), \
               tree.seen || f.id \
          FROM tree JOIN sessions f ON f.parent_id = tree.id \
         WHERE f.metadata->>'relation' = 'followup' AND NOT f.id = ANY(tree.seen) \
           AND f.user_id IS NOT DISTINCT FROM tree.user_id \
           AND cardinality(tree.seen) < 500 \
    ) \
    SELECT s.id, s.session_name, s.working_dir, s.registered_at, t.parent_id \
      FROM tree t JOIN sessions s ON s.id = t.id \
     ORDER BY t.path"
);

/// The follow-up line that leads to `$1`, root first, across owners: its
/// readers check each session. Sibling branches and descendants are not on it.
const CHAIN_SQL: &str = concat!(
    "WITH RECURSIVE ",
    up_cte_sql!(""),
    " SELECT id FROM up ORDER BY cardinality(seen) DESC"
);

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

/// The session ids from the chain's root down to `id`; empty when `id` is unknown.
pub async fn chain_of(pool: &PgPool, id: &str) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(CHAIN_SQL).bind(id).fetch_all(pool).await
}

/// `GET /sessions/{id}/thread`.
pub async fn session_thread<S: SessionsContext>(
    State(state): State<S>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionThreadResponse>, AppError> {
    Ok(Json(SessionThreadResponse { sessions: thread_of(state.pool(), &session_id).await? }))
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
    if let Err(e) = cctui_telemetry::db::traced(
        "INSERT INTO session_followup_intents (spawn_key, parent_session_id) VALUES ($1, $2) \
         ON CONFLICT (spawn_key) DO UPDATE SET parent_session_id = EXCLUDED.parent_session_id",
        |sql| sqlx::query(sql).bind(spawn_key).bind(parent).execute(pool),
    )
    .await
    {
        tracing::warn!(%spawn_key, error = %e, "could not record the follow-up intent");
    }
}

/// Link the session that just registered to the parent a pending intent
/// names.
///
/// Looked up by its own id and by the spawn key the adapter echoed, so
/// both pre-minted (claude-code) and self-minted (codex) ids resolve. The
/// parent must exist and share the session's owner, else the link is skipped.
pub async fn claim_intent(pool: &PgPool, session_id: &str, spawn_key: Option<&str>) {
    crate::followup_op::claim(pool, session_id, spawn_key).await;
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
    let linked = sqlx::query_scalar::<_, Option<String>>(
        "UPDATE sessions SET \
             parent_id = COALESCE(parent_id, (SELECT p.id FROM sessions p \
                 WHERE p.id = $2 AND p.user_id IS NOT DISTINCT FROM sessions.user_id)), \
             metadata = COALESCE(metadata, '{}'::jsonb) || jsonb_build_object('relation', $3::text) \
         WHERE id = $1 RETURNING parent_id",
    )
    .bind(session_id)
    .bind(&parent)
    .bind(RELATION)
    .fetch_optional(pool)
    .await;
    match linked {
        Ok(Some(Some(linked_parent))) => {
            if let Err(e) = stamp_thread_root(pool, session_id, &linked_parent).await {
                tracing::warn!(%session_id, error = %e, "could not record the thread root");
            }
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(
                %session_id, %parent, error = %e, "could not link the follow-up session"
            );
        }
    }
}

/// `session_id` joins the line of `parent`: the parent becomes a root when it
/// had no line yet, and the session takes the parent's root.
async fn stamp_thread_root(
    pool: &PgPool,
    session_id: &str,
    parent: &str,
) -> Result<(), sqlx::Error> {
    let root = match thread_root_of(pool, parent).await? {
        Some(root) => root,
        None => {
            chain_of(pool, parent).await?.into_iter().next().unwrap_or_else(|| parent.to_owned())
        }
    };
    sqlx::query(
        "UPDATE sessions SET thread_root_id = $3 \
         WHERE (id = $1 AND thread_root_id IS NULL) OR id = $2",
    )
    .bind(parent)
    .bind(session_id)
    .bind(&root)
    .execute(pool)
    .await?;
    Ok(())
}

/// The recorded root of the line through `id`; `None` until a follow-up joins it.
pub async fn thread_root_of(pool: &PgPool, id: &str) -> Result<Option<String>, sqlx::Error> {
    let root: Option<Option<String>> =
        sqlx::query_scalar("SELECT thread_root_id FROM sessions WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(root.flatten())
}

/// Reaper tick: drop intents whose spawn never registered and finish the
/// follow-up ops their watcher lost.
pub async fn sweep<S: SessionsContext>(state: &S) {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(INTENT_TTL_SECS);
    if let Err(e) = sqlx::query("DELETE FROM session_followup_intents WHERE created_at < $1")
        .bind(cutoff)
        .execute(state.pool())
        .await
    {
        tracing::warn!(error = %e, "follow-up intent prune failed");
    }
    crate::followup_op::sweep(state).await;
}

pub const DEFAULT_INSTRUCTION: &str = "Continue from this.";

fn ask(instruction: &str) -> &str {
    match instruction.trim() {
        "" => DEFAULT_INSTRUCTION,
        ask => ask,
    }
}

/// A follow-up's first prompt and the [`brief::BRIEF_FILE_NAME`] file it
/// points at, staged as a bootstrap upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowupPrompt {
    pub prompt: String,
    pub file: Option<String>,
}

/// The same prompt and file the webui builds from this chain brief.
#[must_use]
pub fn followup_from_brief(chain: &BriefResponse, instruction: &str) -> FollowupPrompt {
    let envelope = brief::followup_envelope(chain.sessions.as_deref().unwrap_or_default());
    FollowupPrompt {
        prompt: format!("{}\n\n{}", envelope.inline, ask(instruction)),
        file: envelope.file,
    }
}

/// The follow-up of the stored chain ending at `parent_session_id`, with
/// ancestors `reader` may not read as one line. `None` when the parent does
/// not exist.
pub async fn followup_for(
    pool: &PgPool,
    reader: brief::Reader<'_>,
    parent_session_id: &str,
    instruction: &str,
) -> Result<Option<FollowupPrompt>, sqlx::Error> {
    let links = brief::load_chain(pool, reader, parent_session_id).await?;
    if links.last().is_none_or(|l| l.transcript.is_none()) {
        return Ok(None);
    }
    let chain = brief::assemble(&links, brief::chain_max_bytes());
    Ok(Some(followup_from_brief(&chain, instruction)))
}

/// [`followup_for`] a follow-up spawned without a client, read as the
/// parent's owner.
pub async fn render_followup_prompt(
    pool: &PgPool,
    parent_session_id: &str,
    instruction: &str,
) -> Result<Option<FollowupPrompt>, sqlx::Error> {
    let owner = crate::authz::session_owner(parent_session_id, pool).await?;
    followup_for(pool, brief::Reader::Owner(owner), parent_session_id, instruction).await
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
        let url = cctui_server_kernel::test_support::db_url(name)?;
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

    async fn parent_id(pool: &sqlx::PgPool, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT parent_id FROM sessions WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("parent")
    }

    async fn claim_followup_of(pool: &sqlx::PgPool, id: &str, parent: &str) {
        sqlx::query(
            "INSERT INTO session_followup_intents (spawn_key, parent_session_id) VALUES ($1, $2)",
        )
        .bind(id)
        .bind(parent)
        .execute(pool)
        .await
        .expect("intent");
        super::claim_intent(pool, id, None).await;
    }

    fn registration(id: &str, parent: &str, mid: Uuid) -> cctui_proto::models::Session {
        let now = chrono::Utc::now();
        cctui_proto::models::Session {
            id: id.to_owned(),
            parent_id: Some(parent.to_owned()),
            account_id: None,
            machine_id: mid.to_string(),
            working_dir: "/w".into(),
            status: cctui_proto::models::SessionStatus::New,
            registered_at: now,
            last_heartbeat: now,
            metadata: serde_json::json!({"relation": "followup"}),
            adapter_id: None,
        }
    }

    #[tokio::test]
    async fn a_session_never_links_to_or_threads_through_another_users_session() {
        let Some((pool, victim, victim_m)) = test_pool("followup_foreign_parent_victim").await
        else {
            return;
        };
        let Some((_, me, my_m)) = test_pool("followup_foreign_parent_attacker").await else {
            return;
        };
        let theirs = seed(&pool, victim, victim_m, "active").await;
        let their_next = seed(&pool, victim, victim_m, "active").await;
        age(&pool, &theirs, 60).await;
        link(&pool, &their_next, &theirs, "followup", 50).await;

        let planted = seed(&pool, me, my_m, "active").await;
        link(&pool, &planted, &theirs, "followup", 40).await;
        assert_eq!(ids(&thread_of(&pool, &planted).await.expect("thread")), [planted.as_str()]);
        let chain = super::chain_of(&pool, &planted).await.expect("chain");
        assert_eq!(chain, [theirs.clone(), planted.clone()], "readers check each link");
        let their_thread = [theirs.as_str(), their_next.as_str()];
        assert_eq!(ids(&thread_of(&pool, &theirs).await.expect("thread")), their_thread);

        let mine = seed(&pool, me, my_m, "active").await;
        let claimed = seed(&pool, me, my_m, "active").await;
        claim_followup_of(&pool, &claimed, &theirs).await;
        assert_eq!(parent_id(&pool, &claimed).await, None, "a foreign intent parent is dropped");
        let own_claim = seed(&pool, me, my_m, "active").await;
        claim_followup_of(&pool, &own_claim, &mine).await;
        assert_eq!(parent_id(&pool, &own_claim).await, Some(mine.clone()));

        let registered = Uuid::new_v4().to_string();
        let foreign = registration(&registered, &theirs, my_m);
        crate::store::sessions::upsert_registered(&pool, &foreign, my_m, me)
            .await
            .expect("register")
            .expect("written");
        assert_eq!(
            parent_id(&pool, &registered).await,
            None,
            "a foreign register parent is dropped"
        );
        let own_registered = Uuid::new_v4().to_string();
        let own = registration(&own_registered, &mine, my_m);
        crate::store::sessions::upsert_registered(&pool, &own, my_m, me)
            .await
            .expect("register")
            .expect("written");
        assert_eq!(parent_id(&pool, &own_registered).await, Some(mine.clone()));

        let all = [planted, mine, claimed, own_claim, registered, own_registered];
        cleanup(&pool, me, my_m, &all).await;
        cleanup(&pool, victim, victim_m, &[their_next, theirs]).await;
    }
}

#[cfg(test)]
#[path = "followup_prompt_tests.rs"]
mod prompt_tests;
