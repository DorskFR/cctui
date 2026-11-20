//! Archive-on-done for one-shot spawns (macros).
//!
//! A spawn that carries `auto_archive: true` leaves an intent row keyed by
//! its spawn key (the pre-minted session id for claude-code, the command id
//! for adapters that mint their own id). When the daemon registers the
//! session, the intent is claimed and moves to `sessions.metadata.auto_archive`.
//! The reaper then archives the session the first time the classifier reads
//! it as *done* without a failure and nothing nested under it is still live:
//! a session that asks a question, is stopped by hand, or fails stays on the
//! list so the human can look, and one waiting on its children is re-checked
//! each tick until the last of them is done.
//!
//! The flag is consumed on archive, so un-archiving the session later does
//! not re-arm it.

use std::collections::{HashMap, HashSet};

use cctui_proto::classifier::{Bucket, ClassifyInput, PrStatus, classify};
use cctui_proto::models::SessionStatus;
use cctui_server_core::store::sessions::Child;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::live_sessions::live_sessions_predicate;
use crate::routes::sessions::ArchiveOutcome;
use crate::state::AppState;
use crate::store::sessions::SessionRowStatus;

/// Unclaimed intents older than this are dropped: the spawn never registered.
const INTENT_TTL_SECS: i64 = 6 * 3600;
const BATCH: i64 = 100;

/// Remember that the session spawned under `spawn_key` wants archiving once
/// its first turn ends. Best-effort: a lost intent leaves the session listed.
pub async fn remember_intent(state: &AppState, spawn_key: &str) {
    if let Err(e) = cctui_telemetry::db::traced(
        "INSERT INTO session_auto_archive (spawn_key) VALUES ($1) ON CONFLICT (spawn_key) DO NOTHING",
        |sql| sqlx::query(sql).bind(spawn_key).execute(&state.pool),
    )
    .await
    {
        tracing::warn!(%spawn_key, error = %e, "could not record the auto-archive intent");
    }
}

/// Move a pending intent onto the session that just registered. The session
/// is looked up by its own id and by the spawn key the adapter echoed, so
/// both pre-minted (claude-code) and self-minted (codex) ids resolve.
pub async fn claim_intent(state: &AppState, session_id: &str, spawn_key: Option<&str>) {
    let key = spawn_key.unwrap_or(session_id);
    let claimed = match sqlx::query_scalar::<_, String>(
        "DELETE FROM session_auto_archive WHERE spawn_key = $1 OR spawn_key = $2 RETURNING spawn_key",
    )
    .bind(session_id)
    .bind(key)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(row) => row.is_some(),
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "auto-archive intent lookup failed");
            false
        }
    };
    if !claimed {
        return;
    }
    if let Err(e) = sqlx::query(
        "UPDATE sessions SET metadata = COALESCE(metadata, '{}'::jsonb) || '{\"auto_archive\": true}'::jsonb \
         WHERE id = $1",
    )
    .bind(session_id)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%session_id, error = %e, "could not flag the session for auto-archive");
    } else {
        tracing::info!(%session_id, "session armed for auto-archive on done");
    }
}

/// A session nested under one being archived, with what it last reported.
#[derive(Debug, Clone, Default, sqlx::FromRow)]
struct Nested {
    id: String,
    parent_id: Option<String>,
    status: String,
    ended: bool,
    tempo: Option<String>,
    agent_state: Option<String>,
    activity: Option<String>,
    soft_limit_reason: Option<String>,
}

impl Nested {
    /// Anything but a finished turn: working, waiting on someone, or not
    /// reported yet. A draft never started, so it is not waited on.
    fn live(&self) -> bool {
        let input = ClassifyInput {
            tempo: self.tempo.as_deref(),
            state: self.agent_state.as_deref(),
            activity: self.activity.as_deref(),
            children: &[],
            q: None,
            soft_limit_blocked: self.soft_limit_reason.as_deref(),
            ended: self.ended
                || matches!(self.status.as_str(), "ended" | "failed" | "archived" | "draft"),
        };
        classify(&input, &HashMap::<String, PrStatus<'_>>::new()) != Bucket::Done
    }
}

async fn nested(pool: &sqlx::PgPool, ids: &[String]) -> sqlx::Result<Vec<Nested>> {
    sqlx::query_as(
        "SELECT id, parent_id, status, ended_at IS NOT NULL AS ended, tempo, agent_state, \
                activity, soft_limit_reason \
           FROM sessions WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
}

/// `id` and its ancestors inside the tree, nearest first.
fn lineage<'a>(id: &'a str, parent: &HashMap<&'a str, &'a str>, cap: usize) -> Vec<&'a str> {
    let mut chain = vec![id];
    let mut cur = id;
    while let Some(&up) = parent.get(cur) {
        if chain.len() > cap || chain.contains(&up) {
            break;
        }
        chain.push(up);
        cur = up;
    }
    chain
}

/// The descendants an automatic archive leaves running: every live one, the
/// chain that nests it under the archived root, and everything under it.
fn kept_alive(nodes: &[Nested]) -> HashSet<String> {
    let in_tree: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let parent: HashMap<&str, &str> = nodes
        .iter()
        .filter_map(|n| Some((n.id.as_str(), n.parent_id.as_deref()?)))
        .filter(|(_, p)| in_tree.contains(p))
        .collect();
    let live: HashSet<&str> = nodes.iter().filter(|n| n.live()).map(|n| n.id.as_str()).collect();
    let mut kept = HashSet::new();
    for node in nodes {
        let chain = lineage(&node.id, &parent, nodes.len());
        if chain.iter().any(|id| live.contains(id)) {
            kept.insert(node.id.clone());
        }
        if live.contains(node.id.as_str()) {
            kept.extend(chain.iter().map(|id| (*id).to_owned()));
        }
    }
    kept
}

/// [`kept_alive`] for `children`, read from the database. A failed read
/// keeps every child: an automatic archive never risks a live one.
pub async fn kept_alive_under(pool: &sqlx::PgPool, children: &[Child]) -> HashSet<String> {
    let ids: Vec<String> = children.iter().map(|c| c.id.clone()).collect();
    if ids.is_empty() {
        return HashSet::new();
    }
    match nested(pool, &ids).await {
        Ok(nodes) => kept_alive(&nodes),
        Err(e) => {
            tracing::warn!(error = %e, "could not read the children's signals; keeping them all");
            ids.into_iter().collect()
        }
    }
}

async fn live_children(pool: &sqlx::PgPool, id: &str) -> sqlx::Result<usize> {
    let ids: Vec<String> =
        crate::store::sessions::descendants(pool, id).await?.into_iter().map(|c| c.id).collect();
    if ids.is_empty() {
        return Ok(0);
    }
    Ok(nested(pool, &ids).await?.iter().filter(|n| n.live()).count())
}

/// A flagged session is archived once the classifier reads it as done,
/// nothing says the turn failed and no child is still live. `activity` is the
/// agent's own verdict: `failure` and `stopped` (killed by hand) keep the
/// session listed. An agent idle on a background shell or Monitor reports
/// `working`, which never reads as done.
fn should_archive(
    tempo: Option<&str>,
    agent_state: Option<&str>,
    activity: Option<&str>,
    soft_limit_blocked: Option<&str>,
    children_live: usize,
) -> bool {
    if children_live > 0 || matches!(activity, Some("failure" | "stopped")) {
        return false;
    }
    let bucket = crate::routes::sessions::bucket_from_signals(
        tempo,
        agent_state,
        activity,
        soft_limit_blocked,
        &[],
        &HashMap::new(),
    );
    bucket == Bucket::Done && cctui_proto::classifier::reports_finished(agent_state, activity)
}

type SignalRow = (String, Option<String>, Option<String>, Option<String>, Option<String>);

/// Reaper tick: archive every flagged session that is done, and drop stale
/// unclaimed intents.
pub async fn sweep(state: &AppState) {
    run(state, None).await;
}

async fn run(state: &AppState, owner: Option<Uuid>) {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(INTENT_TTL_SECS);
    if let Err(e) = sqlx::query("DELETE FROM session_auto_archive WHERE created_at < $1")
        .bind(cutoff)
        .execute(&state.pool)
        .await
    {
        tracing::warn!(error = %e, "auto-archive intent prune failed");
    }

    let rows = match sqlx::query_as::<_, SignalRow>(concat!(
        "SELECT id, tempo, agent_state, activity, soft_limit_reason FROM sessions \
             WHERE ",
        live_sessions_predicate!(),
        " AND metadata->>'auto_archive' = 'true' \
               AND status <> ALL($2) \
               AND ($3::uuid IS NULL OR user_id = $3) \
             LIMIT $1"
    ))
    .bind(BATCH)
    .bind(SessionRowStatus::names(SessionRowStatus::NOT_ARCHIVABLE))
    .bind(owner)
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "auto-archive sweep query failed");
            return;
        }
    };

    for (id, tempo, agent_state, activity, soft_limit) in rows {
        let children_live = match live_children(&state.pool, &id).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(session_id = %id, error = %e, "auto-archive child check failed");
                continue;
            }
        };
        if !should_archive(
            tempo.as_deref(),
            agent_state.as_deref(),
            activity.as_deref(),
            soft_limit.as_deref(),
            children_live,
        ) {
            continue;
        }
        // Consume the flag first so a failed archive is retried on the next
        // tick but an un-archive by the human is never undone.
        if let Err(e) = sqlx::query(
            "UPDATE sessions SET metadata = COALESCE(metadata, '{}'::jsonb) || '{\"auto_archive\": false}'::jsonb \
             WHERE id = $1",
        )
        .bind(&id)
        .execute(&state.pool)
        .await
        {
            tracing::warn!(session_id = %id, error = %e, "could not clear the auto-archive flag");
            continue;
        }
        // Never forced: a session the human pinned since the spawn stays.
        match crate::routes::sessions::archive_one(
            state,
            &id,
            false,
            cctui_proto::adapter::RemoveInitiator::Automatic,
            crate::events::Actor::Reaper,
        )
        .await
        {
            Ok(ArchiveOutcome::Archived) => {
                tracing::info!(session_id = %id, "auto-archived a finished macro session");
            }
            Ok(ArchiveOutcome::SkippedPinned) => {
                tracing::info!(session_id = %id, "auto-archive skipped: session is pinned");
            }
            Err(e) => tracing::warn!(session_id = %id, error = %e, "auto-archive failed"),
        }
    }
}

/// When the idle-TTL sweep (`auto_archive_stale`) will archive a session:
/// `ttl_secs` after its last heartbeat, unless pinned, already archived, a
/// draft, or the sweep is disabled (`0`).
pub fn stale_archive_due(
    status: SessionStatus,
    pinned: bool,
    last_heartbeat: Option<DateTime<Utc>>,
    ttl_secs: u64,
) -> Option<DateTime<Utc>> {
    if ttl_secs == 0
        || pinned
        || matches!(status, SessionStatus::Archived | SessionStatus::Draft | SessionStatus::Queued)
    {
        return None;
    }
    last_heartbeat?.checked_add_signed(chrono::Duration::seconds(i64::try_from(ttl_secs).ok()?))
}

#[cfg(test)]
mod tests {
    use super::{Nested, kept_alive, run, should_archive, stale_archive_due};
    use crate::events::tests::{rows_for_session, seed_session, seed_user_machine};
    use crate::state::AppState;
    use cctui_proto::adapter::{AdapterCommand, RemoveInitiator};
    use cctui_proto::models::SessionStatus;
    use cctui_proto::ws::DaemonFrameDown;
    use serde_json::{Value, json};

    #[test]
    fn stale_archive_is_due_one_ttl_after_the_last_heartbeat() {
        let hb = chrono::DateTime::parse_from_rfc3339("2026-09-24T10:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let due = stale_archive_due(SessionStatus::Inactive, false, Some(hb), 24 * 3600);
        assert_eq!(due, Some(hb + chrono::Duration::hours(24)));
        assert!(stale_archive_due(SessionStatus::Active, false, Some(hb), 3600).is_some());
    }

    #[test]
    fn stale_archive_never_due_for_protected_sessions() {
        let hb = Some(chrono::Utc::now());
        assert_eq!(stale_archive_due(SessionStatus::Inactive, true, hb, 3600), None);
        assert_eq!(stale_archive_due(SessionStatus::Archived, false, hb, 3600), None);
        assert_eq!(stale_archive_due(SessionStatus::Draft, false, hb, 3600), None);
        assert_eq!(stale_archive_due(SessionStatus::Inactive, false, hb, 0), None);
        assert_eq!(stale_archive_due(SessionStatus::Inactive, false, None, 3600), None);
    }

    #[test]
    fn archives_a_clean_done_turn() {
        assert!(should_archive(None, Some("stopped"), None, None, 0));
        assert!(should_archive(None, Some("done"), Some("success"), None, 0));
        assert!(should_archive(Some("idle"), None, Some("success"), None, 0));
    }

    #[test]
    fn keeps_working_blocked_and_failed_sessions() {
        assert!(!should_archive(None, None, None, None, 0));
        assert!(!should_archive(Some("idle"), Some("idle"), None, None, 0));
        assert!(!should_archive(Some("active"), Some("working"), None, None, 0));
        assert!(!should_archive(Some("blocked"), None, None, None, 0));
        assert!(!should_archive(None, Some("stopped"), None, Some("locked"), 0));
        assert!(!should_archive(None, Some("done"), Some("failure"), None, 0));
        assert!(!should_archive(None, Some("killed"), Some("stopped"), None, 0));
    }

    #[test]
    fn a_done_parent_waits_for_its_live_children() {
        assert!(!should_archive(Some("idle"), Some("done"), Some("success"), None, 1));
        assert!(!should_archive(None, Some("stopped"), None, None, 3));
        assert!(should_archive(Some("idle"), Some("done"), Some("success"), None, 0));
    }

    #[test]
    fn an_agent_idle_on_a_background_task_is_not_done() {
        assert!(!should_archive(Some("idle"), Some("working"), None, None, 0));
        assert!(!should_archive(Some("idle"), Some("running"), None, None, 0));
    }

    fn node(id: &str, parent: &str, tempo: Option<&str>, state: Option<&str>) -> Nested {
        Nested {
            id: id.to_owned(),
            parent_id: Some(parent.to_owned()),
            status: "active".to_owned(),
            tempo: tempo.map(str::to_owned),
            agent_state: state.map(str::to_owned),
            ..Nested::default()
        }
    }

    #[test]
    fn only_a_finished_child_reads_as_not_live() {
        assert!(node("w", "p", Some("active"), Some("working")).live());
        assert!(node("bg", "p", Some("idle"), Some("working")).live());
        assert!(node("q", "p", Some("blocked"), None).live());
        assert!(node("task", "p", None, None).live(), "an unreported Task subagent is running");
        assert!(!node("d", "p", Some("idle"), Some("done")).live());
        let mut ended = node("e", "p", Some("active"), Some("working"));
        ended.ended = true;
        assert!(!ended.live());
        for status in ["archived", "draft", "failed"] {
            let mut row = node("x", "p", None, None);
            row.status = status.to_owned();
            assert!(!row.live(), "{status}");
        }
    }

    #[test]
    fn an_automatic_archive_keeps_a_live_child_its_chain_and_its_subtree() {
        let nodes = [
            node("done-kid", "root", Some("idle"), Some("done")),
            node("mid", "root", Some("idle"), Some("done")),
            node("working", "mid", Some("active"), Some("working")),
            node("under-working", "working", Some("idle"), Some("done")),
            node("done-sibling", "mid", Some("idle"), Some("done")),
        ];
        let mut kept: Vec<String> = kept_alive(&nodes).into_iter().collect();
        kept.sort();
        assert_eq!(kept, ["mid", "under-working", "working"]);
        assert!(kept_alive(&nodes[..1]).is_empty());
    }

    async fn set_signals(
        pool: &sqlx::PgPool,
        id: &str,
        parent: Option<&str>,
        signals: [Option<&str>; 3],
        metadata: Value,
    ) {
        sqlx::query(
            "UPDATE sessions SET parent_id = COALESCE($2, parent_id), tempo = $3, \
                    agent_state = $4, activity = $5, metadata = $6 WHERE id = $1",
        )
        .bind(id)
        .bind(parent)
        .bind(signals[0])
        .bind(signals[1])
        .bind(signals[2])
        .bind(metadata)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn status_of(pool: &sqlx::PgPool, id: &str) -> String {
        sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn archived_detail(pool: &sqlx::PgPool, id: &str) -> Value {
        for _ in 0..100 {
            let rows = rows_for_session(pool, id).await;
            if let Some(row) =
                rows.into_iter().find(|r| r.kind == crate::events::kind::SESSION_ARCHIVED)
            {
                return row.detail;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("no session.archived event for {id}");
    }

    const DONE: [Option<&str>; 3] = [Some("idle"), Some("done"), Some("success")];
    const WORKING: [Option<&str>; 3] = [Some("active"), Some("working"), None];

    #[tokio::test]
    async fn sweep_waits_for_a_working_child_then_archives_once_both_are_done() {
        let Some(url) = crate::routes::gateway::test_db_url("auto_archive_waits_for_children")
        else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, machine) = seed_user_machine(&pool, "aa-kids").await;
        let parent = seed_session(&pool, uid, machine, Some("orch")).await;
        let child = seed_session(&pool, uid, machine, Some("kid")).await;
        set_signals(&pool, &parent, None, DONE, json!({"auto_archive": true})).await;
        set_signals(&pool, &child, Some(&parent), WORKING, json!({"relation": "subagent"})).await;

        run(&state, Some(uid)).await;
        assert_eq!(status_of(&pool, &parent).await, "active");
        assert_eq!(status_of(&pool, &child).await, "active");

        set_signals(&pool, &child, None, DONE, json!({"relation": "subagent"})).await;
        run(&state, Some(uid)).await;
        assert_eq!(status_of(&pool, &parent).await, "archived");
        assert_eq!(status_of(&pool, &child).await, "archived");
        let detail = archived_detail(&pool, &parent).await;
        assert_eq!(detail["initiator"], "automatic");
        assert_eq!(detail["children"], 1);
        assert_eq!(detail["children_live"], 0);
    }

    #[tokio::test]
    async fn an_automatic_archive_never_removes_a_working_child() {
        let Some(url) = crate::routes::gateway::test_db_url("auto_archive_spares_working_child")
        else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, machine) = seed_user_machine(&pool, "aa-spare").await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        state.bus.register_daemon(machine, uuid::Uuid::new_v4(), tx);
        let parent = seed_session(&pool, uid, machine, Some("orch")).await;
        let working = seed_session(&pool, uid, machine, Some("busy")).await;
        let finished = seed_session(&pool, uid, machine, Some("idle")).await;
        set_signals(&pool, &parent, None, DONE, json!({})).await;
        set_signals(&pool, &working, Some(&parent), WORKING, json!({"relation": "subagent"})).await;
        set_signals(&pool, &finished, Some(&parent), DONE, json!({"relation": "subagent"})).await;

        crate::routes::sessions::archive_one(
            &state,
            &parent,
            false,
            RemoveInitiator::Automatic,
            crate::events::Actor::Reaper,
        )
        .await
        .unwrap();

        let mut removed = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            if let DaemonFrameDown::Command { command, .. } = frame
                && let AdapterCommand::Remove { local_id, .. } = *command
            {
                removed.push(local_id);
            }
        }
        assert!(!removed.contains(&working), "a working child got a Remove: {removed:?}");
        assert!(removed.contains(&finished) && removed.contains(&parent), "{removed:?}");
        assert_eq!(status_of(&pool, &working).await, "active");
        assert_eq!(status_of(&pool, &finished).await, "archived");
        assert_eq!(status_of(&pool, &parent).await, "archived");
        let detail = archived_detail(&pool, &parent).await;
        assert_eq!(detail["children"], 1);
        assert_eq!(detail["children_live"], 1);
    }
}
