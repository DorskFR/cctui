use std::future::Future;
use std::collections::HashSet;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use chrono::Utc;
use uuid::Uuid;

use crate::state::AppState;

/// How long a closed daemon WS may stay closed before its sessions are ended as
/// `daemon_lost`. Sized to outlast a rolling restart, a pod eviction or a
/// network blip, all of which reconnect within seconds.
const DAEMON_LOST_GRACE: Duration = Duration::from_secs(45);

/// A `machines.last_seen_at` younger than this means the daemon is heartbeating
/// at *some* replica. Two of the daemon's 20s heartbeat cadences, so one missed
/// heartbeat does not read as death, and under [`DAEMON_LOST_GRACE`] so a
/// machine that is genuinely gone is never held alive by its own last beat.
const DAEMON_SEEN_FRESH: Duration = Duration::from_secs(40);

/// One closed connection's deferred mark: the sessions it announced that no
/// connection has announced since.
struct PendingMark {
    machine_id: Uuid,
    sessions: HashSet<String>,
    task: tokio::task::AbortHandle,
}

/// Deferred `daemon_lost` marks, one per closed connection. Every dispatched
/// worker pod shares one machine id, so a mark is only ever cancelled for the
/// sessions a later connection announces, or for the whole machine when it is
/// not shared.
#[derive(Default)]
pub(super) struct PendingDaemonLost {
    marks: dashmap::DashMap<Uuid, PendingMark>,
}

impl PendingDaemonLost {
    fn schedule<F, Fut>(
        self: &Arc<Self>,
        conn_id: Uuid,
        machine_id: Uuid,
        sessions: Vec<String>,
        delay: Duration,
        mark: F,
    ) where
        F: FnOnce(Vec<String>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let this = Arc::clone(self);
        let task = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let Some((_, pending)) = this.marks.remove(&conn_id) else { return };
            mark(pending.sessions.into_iter().collect()).await;
        });
        let sessions = sessions.into_iter().collect();
        let pending = PendingMark { machine_id, sessions, task: task.abort_handle() };
        if let Some(superseded) = self.marks.insert(conn_id, pending) {
            superseded.task.abort();
        }
    }

    /// A reconnect of a machine only one daemon uses: its marks are moot.
    pub(super) fn cancel_machine(&self, machine_id: Uuid) {
        self.marks.retain(|_, pending| {
            let keep = pending.machine_id != machine_id;
            if !keep {
                pending.task.abort();
            }
            keep
        });
    }

    /// `session_id` was announced again, so it is alive on some connection.
    pub(super) fn announced(&self, session_id: &str) {
        self.marks.retain(|_, pending| {
            pending.sessions.remove(session_id);
            let keep = !pending.sessions.is_empty();
            if !keep {
                pending.task.abort();
            }
            keep
        });
    }
}

pub(super) static PENDING_DAEMON_LOST: LazyLock<Arc<PendingDaemonLost>> =
    LazyLock::new(|| Arc::new(PendingDaemonLost::default()));

/// The daemon's WS is gone: every session it announced is now unreachable,
/// so end them as `daemon_lost` — but only if no connection re-announced them
/// within [`DAEMON_LOST_GRACE`]. Soft — [`upsert_session`] reverts it when the
/// daemon reconnects and re-registers the session as alive. Only `sessions`
/// are eligible: other daemons on the same machine id keep theirs. A `shared`
/// machine's heartbeat is a sibling's, so it says nothing about this daemon.
pub(super) fn schedule_daemon_lost(
    state: &AppState,
    machine_id: Uuid,
    conn_id: Uuid,
    shared: bool,
    sessions: Vec<String>,
) {
    if sessions.is_empty() {
        tracing::info!(%machine_id, "daemon_lost mark skipped — connection announced no sessions");
        return;
    }
    let (completing, announced) = (state.clone(), sessions.clone());
    tokio::spawn(async move { complete_guard_exited(&completing, machine_id, &announced).await });
    let state = state.clone();
    let mark = move |left: Vec<String>| async move {
        if !shared && daemon_seen_recently(&state.pool, machine_id).await {
            tracing::info!(%machine_id, "daemon_lost mark skipped — machine heartbeating elsewhere");
            return;
        }
        for session_id in mark_daemon_lost(&state.pool, machine_id, &left).await {
            crate::events::record(
                &state,
                crate::events::Event::new(
                    crate::events::kind::SESSION_ENDED,
                    crate::events::Actor::Daemon,
                )
                .severity(crate::events::Severity::Warn)
                .session(session_id)
                .machine(machine_id)
                .detail(serde_json::json!({
                    "end_reason": "daemon_lost",
                    "end_detail": "daemon connection closed",
                })),
            );
        }
    };
    PENDING_DAEMON_LOST.schedule(conn_id, machine_id, sessions, DAEMON_LOST_GRACE, mark);
}

/// A worker tears its daemon down right after its workflow guard exits, so a
/// guard-exited session ends `completed` at the disconnect, not after the grace.
async fn complete_guard_exited(state: &AppState, machine_id: Uuid, sessions: &[String]) {
    let exited_here: Vec<String> = sessions
        .iter()
        .filter(|id| crate::guard_step::get(id).is_some_and(|step| step.exited()))
        .cloned()
        .collect();
    for session_id in mark_guard_exited(&state.pool, machine_id, sessions, &exited_here).await {
        crate::routes::gateway::revoke_session_tokens(state, &session_id).await;
        crate::events::record(
            state,
            crate::events::Event::new(
                crate::events::kind::SESSION_ENDED,
                crate::events::Actor::Daemon,
            )
            .session(session_id)
            .machine(machine_id)
            .detail(serde_json::json!({ "end_reason": "completed", "end_detail": null })),
        );
    }
}

/// The ids ended `completed`: those whose guard exited, per this replica's
/// mirror (`exited_here`) or the table any replica wrote.
async fn mark_guard_exited(
    pool: &sqlx::PgPool,
    machine_id: Uuid,
    sessions: &[String],
    exited_here: &[String],
) -> Vec<String> {
    match sqlx::query_scalar::<_, String>(
        "UPDATE sessions SET status = 'ended', ended_at = now(), end_reason = 'completed', \
             end_detail = NULL \
         WHERE machine_uuid = $1 AND id = ANY($2) AND status IN ('new', 'active', 'inactive') \
           AND end_reason IS NULL \
           AND (id = ANY($3) OR EXISTS (SELECT 1 FROM guard_steps g \
                WHERE g.session_id = sessions.id AND (g.state->>'step')::bigint < 0)) \
         RETURNING id",
    )
    .bind(machine_id)
    .bind(sessions)
    .bind(exited_here)
    .fetch_all(pool)
    .await
    {
        Ok(ids) => {
            if !ids.is_empty() {
                tracing::info!(
                    %machine_id,
                    count = ids.len(),
                    "guard-exited sessions completed on disconnect"
                );
            }
            ids
        }
        Err(err) => {
            tracing::warn!(%err, %machine_id, "guard-exited completion failed");
            Vec::new()
        }
    }
}

/// Cross-pod backstop for the local cancel: a daemon that dropped this pod's WS
/// and reconnected to a peer replica never cancels the mark scheduled here, but
/// its heartbeats keep `machines.last_seen_at` fresh from whichever pod they
/// land on. A DB error answers "not seen", so the mark still lands.
async fn daemon_seen_recently(pool: &sqlx::PgPool, machine_id: Uuid) -> bool {
    match sqlx::query_scalar::<_, chrono::DateTime<Utc>>(
        "SELECT last_seen_at FROM machines WHERE id = $1",
    )
    .bind(machine_id)
    .fetch_optional(pool)
    .await
    {
        Ok(last_seen_at) => seen_within(last_seen_at, Utc::now(), DAEMON_SEEN_FRESH),
        Err(err) => {
            tracing::warn!(%err, %machine_id, "last_seen_at lookup failed before daemon_lost mark");
            false
        }
    }
}

fn seen_within(
    last_seen_at: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
    window: Duration,
) -> bool {
    let window = chrono::Duration::seconds(i64::try_from(window.as_secs()).unwrap_or(i64::MAX));
    last_seen_at.is_some_and(|seen| now.signed_duration_since(seen) < window)
}

/// The ids actually ended. A session another replica holds a live
/// announcement for was re-announced there and is not lost.
async fn mark_daemon_lost(
    pool: &sqlx::PgPool,
    machine_id: Uuid,
    sessions: &[String],
) -> Vec<String> {
    match sqlx::query_scalar::<_, String>(
        "UPDATE sessions SET status = 'ended', ended_at = now(), end_reason = 'daemon_lost', \
             end_detail = 'daemon connection closed' \
         WHERE machine_uuid = $1 AND id = ANY($2) AND status IN ('new', 'active', 'inactive') \
           AND end_reason IS NULL AND last_heartbeat > now() - interval '1 hour' \
           AND NOT EXISTS (SELECT 1 FROM ws_presence p \
                WHERE p.kind = $3 AND p.entity_id::text = sessions.id \
                  AND p.heartbeat_at > now() - make_interval(secs => $4)) \
         RETURNING id",
    )
    .bind(machine_id)
    .bind(sessions)
    .bind(crate::presence::Kind::Session.as_str())
    .bind(f64::from(crate::presence::LIVE_WITHIN_SECS))
    .fetch_all(pool)
    .await
    {
        Ok(ids) => {
            if !ids.is_empty() {
                tracing::info!(%machine_id, count = ids.len(), "sessions marked daemon_lost");
            }
            ids
        }
        Err(err) => {
            tracing::warn!(%err, %machine_id, "daemon_lost mark failed");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Marked = Arc<std::sync::Mutex<Vec<Vec<String>>>>;

    fn marker() -> (Arc<PendingDaemonLost>, Marked) {
        (Arc::new(PendingDaemonLost::default()), Marked::default())
    }

    fn mark(
        marked: &Marked,
    ) -> impl FnOnce(Vec<String>) -> futures_util::future::Ready<()> + Send + use<> {
        let marked = Arc::clone(marked);
        move |mut sessions| {
            sessions.sort();
            marked.lock().expect("marks").push(sessions);
            futures_util::future::ready(())
        }
    }

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|&n| n.to_owned()).collect()
    }

    fn marked_sessions(marked: &Marked) -> Vec<Vec<String>> {
        marked.lock().expect("marks").clone()
    }

    /// A spawned task only arms its `sleep` on its first poll, so the clock may
    /// not be advanced until every freshly scheduled task has run once.
    async fn settle() {
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn reconnect_inside_the_grace_window_cancels_the_mark() {
        let (pending, marked) = marker();
        let machine = Uuid::new_v4();

        pending.schedule(Uuid::new_v4(), machine, ids(&["a"]), DAEMON_LOST_GRACE, mark(&marked));
        settle().await;
        tokio::time::advance(DAEMON_LOST_GRACE / 2).await;
        pending.cancel_machine(machine);
        tokio::time::advance(DAEMON_LOST_GRACE * 4).await;
        settle().await;

        assert!(marked_sessions(&marked).is_empty());
        assert!(pending.marks.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_daemon_that_never_returns_is_marked_after_the_grace_window() {
        let (pending, marked) = marker();

        pending.schedule(
            Uuid::new_v4(),
            Uuid::new_v4(),
            ids(&["a", "b"]),
            DAEMON_LOST_GRACE,
            mark(&marked),
        );
        settle().await;
        tokio::time::advance(DAEMON_LOST_GRACE / 2).await;
        settle().await;
        assert!(marked_sessions(&marked).is_empty());

        tokio::time::advance(DAEMON_LOST_GRACE).await;
        settle().await;
        assert_eq!(marked_sessions(&marked), [ids(&["a", "b"])]);
        assert!(pending.marks.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn sibling_connections_on_one_machine_keep_their_own_marks() {
        let (pending, marked) = marker();
        let machine = Uuid::new_v4();

        pending.schedule(Uuid::new_v4(), machine, ids(&["a"]), DAEMON_LOST_GRACE, mark(&marked));
        settle().await;
        tokio::time::advance(DAEMON_LOST_GRACE / 2).await;
        pending.schedule(Uuid::new_v4(), machine, ids(&["b"]), DAEMON_LOST_GRACE, mark(&marked));
        pending.announced("c");
        assert_eq!(pending.marks.len(), 2, "a sibling's close or announce drops no mark");
        settle().await;

        tokio::time::advance(DAEMON_LOST_GRACE * 2).await;
        settle().await;
        let mut got = marked_sessions(&marked);
        got.sort();
        assert_eq!(got, [ids(&["a"]), ids(&["b"])]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_reannounced_session_leaves_the_pending_mark() {
        let (pending, marked) = marker();
        let machine = Uuid::new_v4();

        pending.schedule(
            Uuid::new_v4(),
            machine,
            ids(&["a", "b"]),
            DAEMON_LOST_GRACE,
            mark(&marked),
        );
        pending.schedule(Uuid::new_v4(), machine, ids(&["c"]), DAEMON_LOST_GRACE, mark(&marked));
        settle().await;
        pending.announced("a");
        pending.announced("c");
        assert_eq!(pending.marks.len(), 1, "a mark with nothing left to end is dropped");

        tokio::time::advance(DAEMON_LOST_GRACE * 2).await;
        settle().await;
        assert_eq!(marked_sessions(&marked), [ids(&["b"])]);
        assert!(pending.marks.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_machine_cancel_spares_other_machines() {
        let (pending, marked) = marker();
        let (machine, other) = (Uuid::new_v4(), Uuid::new_v4());

        pending.schedule(Uuid::new_v4(), machine, ids(&["a"]), DAEMON_LOST_GRACE, mark(&marked));
        pending.schedule(Uuid::new_v4(), other, ids(&["b"]), DAEMON_LOST_GRACE, mark(&marked));
        settle().await;
        pending.cancel_machine(machine);

        tokio::time::advance(DAEMON_LOST_GRACE * 2).await;
        settle().await;
        assert_eq!(marked_sessions(&marked), [ids(&["b"])]);
    }

    #[test]
    fn a_gone_daemons_own_last_beat_can_never_look_fresh() {
        assert!(
            DAEMON_SEEN_FRESH < DAEMON_LOST_GRACE,
            "a mark fires DAEMON_LOST_GRACE after the disconnect, so last_seen_at is at least \
             that old unless some other replica advanced it",
        );
    }

    #[test]
    fn freshness_reads_the_heartbeat_not_the_row() {
        let now = Utc::now();
        let fresh = now - chrono::Duration::seconds(4);
        let stale = now - chrono::Duration::seconds(600);

        assert!(seen_within(Some(fresh), now, DAEMON_SEEN_FRESH));
        assert!(!seen_within(Some(stale), now, DAEMON_SEEN_FRESH));
        assert!(!seen_within(None, now, DAEMON_SEEN_FRESH), "an unknown machine is not alive");
        assert!(
            seen_within(Some(now + chrono::Duration::seconds(5)), now, DAEMON_SEEN_FRESH),
            "clock skew must not read as death",
        );
        assert!(
            !seen_within(
                Some(now - chrono::Duration::from_std(DAEMON_LOST_GRACE).expect("grace fits")),
                now,
                DAEMON_SEEN_FRESH,
            ),
            "a beat older than the grace window is exactly the gone-daemon case",
        );
    }

    /// DB-gated: the cross-pod backstop. A daemon that reconnected to a peer
    /// replica keeps `last_seen_at` fresh, and this pod's pending mark must
    /// stand down; a machine nobody has heard from is still marked.
    #[tokio::test]
    async fn a_machine_heartbeating_at_a_peer_replica_is_not_marked() {
        let Some(url) = crate::routes::gateway::test_db_url("daemon_seen_recently") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("seen-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");

        let seed_machine = async |last_seen_at: chrono::DateTime<Utc>| {
            let mid = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash, last_seen_at) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(mid)
            .bind(uid)
            .bind(format!("m-{mid}"))
            .bind(format!("mk-{mid}"))
            .bind(last_seen_at)
            .execute(&pool)
            .await
            .expect("seed machine");
            mid
        };

        let live = seed_machine(Utc::now() - chrono::Duration::seconds(4)).await;
        let gone = seed_machine(Utc::now() - chrono::Duration::seconds(600)).await;

        assert!(
            super::daemon_seen_recently(&pool, live).await,
            "a 4s-old heartbeat means the daemon reconnected somewhere",
        );
        assert!(!super::daemon_seen_recently(&pool, gone).await);
        assert!(
            !super::daemon_seen_recently(&pool, Uuid::new_v4()).await,
            "an unknown machine is not alive",
        );
    }

    /// Two worker pods share one machine id. Closing one pod's WS must end
    /// only the sessions that pod announced, not its neighbour's.
    #[tokio::test]
    async fn daemon_lost_is_scoped_to_the_closing_connections_sessions() {
        let Some(url) = crate::routes::gateway::test_db_url("daemon_lost_scope") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("scope-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        let mid = Uuid::new_v4();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(mid)
            .bind(uid)
            .bind(format!("m-{mid}"))
            .bind(format!("mk-{mid}"))
            .execute(&pool)
            .await
            .expect("seed machine");
        let mine = format!("mine-{}", Uuid::new_v4());
        let neighbour = format!("neighbour-{}", Uuid::new_v4());
        for id in [&mine, &neighbour] {
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, working_dir, status, user_id, machine_uuid, adapter_id) \
                 VALUES ($1, $2, '/w', 'active', $3, $4, 'claude-code')",
            )
            .bind(id)
            .bind(mid.to_string())
            .bind(uid)
            .bind(mid)
            .execute(&pool)
            .await
            .expect("seed session");
        }

        super::mark_daemon_lost(&pool, mid, std::slice::from_ref(&mine)).await;

        let status = async |id: &str| -> (String, Option<String>) {
            sqlx::query_as("SELECT status, end_reason FROM sessions WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .expect("session row")
        };
        assert_eq!(status(&mine).await, ("ended".into(), Some("daemon_lost".into())));
        assert_eq!(status(&neighbour).await, ("active".into(), None));
    }

    /// A dispatched worker SIGTERMs its daemon once the guard exits: the
    /// disconnect must end that session `completed`, and the later
    /// `daemon_lost` mark only the sessions with no completion evidence.
    #[tokio::test]
    async fn a_guard_exited_session_ends_completed_not_daemon_lost() {
        let Some(url) = crate::routes::gateway::test_db_url("daemon_lost_guard_exited") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let (uid, mid) = crate::events::tests::seed_user_machine(&pool, "dl-guard-exit").await;
        let seed = async |step: Option<i64>| {
            let sid = crate::events::tests::seed_session(&pool, uid, mid, None).await;
            if let Some(step) = step {
                let guard = cctui_proto::guard_step::GuardStepState { step, ..Default::default() };
                sqlx::query("INSERT INTO guard_steps (session_id, state) VALUES ($1, $2)")
                    .bind(&sid)
                    .bind(sqlx::types::Json(&guard))
                    .execute(&pool)
                    .await
                    .expect("seed guard step");
            }
            sid
        };
        let exited = seed(Some(-1)).await;
        let exited_here = seed(None).await;
        let mid_workflow = seed(Some(3)).await;
        let unguarded = seed(None).await;
        let sessions =
            vec![exited.clone(), exited_here.clone(), mid_workflow.clone(), unguarded.clone()];

        let mut completed =
            super::mark_guard_exited(&pool, mid, &sessions, std::slice::from_ref(&exited_here))
                .await;
        completed.sort();
        let mut want = vec![exited.clone(), exited_here.clone()];
        want.sort();
        assert_eq!(completed, want);

        let mut lost = super::mark_daemon_lost(&pool, mid, &sessions).await;
        lost.sort();
        let mut want = vec![mid_workflow.clone(), unguarded.clone()];
        want.sort();
        assert_eq!(lost, want, "completed sessions are never re-marked daemon_lost");

        let end_reason = async |id: &str| -> Option<String> {
            sqlx::query_scalar("SELECT end_reason FROM sessions WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .expect("session row")
        };
        assert_eq!(end_reason(&exited).await.as_deref(), Some("completed"));
        assert_eq!(end_reason(&exited_here).await.as_deref(), Some("completed"));
        assert_eq!(end_reason(&mid_workflow).await.as_deref(), Some("daemon_lost"));
        assert_eq!(end_reason(&unguarded).await.as_deref(), Some("daemon_lost"));
    }

    /// A dispatched pod that reconnected to a peer replica re-announced its
    /// session there; this replica's pending mark must not end it.
    #[tokio::test]
    async fn a_session_live_on_a_peer_replica_is_not_marked() {
        let Some(url) = crate::routes::gateway::test_db_url("daemon_lost_presence") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let (uid, mid) = crate::events::tests::seed_user_machine(&pool, "dl-presence").await;
        let moved = crate::events::tests::seed_session(&pool, uid, mid, None).await;
        let stale = crate::events::tests::seed_session(&pool, uid, mid, None).await;
        let lost = crate::events::tests::seed_session(&pool, uid, mid, None).await;
        for (sid, age_secs) in [(&moved, 1), (&stale, 600)] {
            sqlx::query(
                "INSERT INTO ws_presence (kind, entity_id, pod, pod_ip, heartbeat_at) \
                 VALUES ('session', $1::uuid, 'peer', '10.0.0.2', \
                         now() - make_interval(secs => $2))",
            )
            .bind(sid)
            .bind(f64::from(age_secs))
            .execute(&pool)
            .await
            .expect("seed presence");
        }

        let mut marked = super::mark_daemon_lost(
            &pool,
            mid,
            &[moved.clone(), stale.clone(), lost.clone()],
        )
        .await;
        marked.sort();
        let mut want = vec![stale.clone(), lost.clone()];
        want.sort();
        assert_eq!(marked, want, "a stale presence row is a crashed pod, not a live session");

        let cleanup = [moved, stale];
        sqlx::query("DELETE FROM ws_presence WHERE entity_id::text = ANY($1)")
            .bind(&cleanup[..])
            .execute(&pool)
            .await
            .expect("drop presence");
    }
}
