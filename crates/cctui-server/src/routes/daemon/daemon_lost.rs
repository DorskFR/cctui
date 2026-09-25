use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
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

/// Deferred `daemon_lost` marks, one at most per machine, cancelled by a
/// reconnect. A machine that never comes back still gets marked once the delay
/// elapses.
#[derive(Default)]
pub(super) struct PendingDaemonLost {
    marks: dashmap::DashMap<Uuid, (u64, tokio::task::AbortHandle)>,
    seq: AtomicU64,
}

impl PendingDaemonLost {
    fn schedule<F>(self: &Arc<Self>, machine_id: Uuid, delay: Duration, mark: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let this = Arc::clone(self);
        let task = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            this.marks.remove_if(&machine_id, |_, (pending, _)| *pending == seq);
            mark.await;
        });
        if let Some((_, superseded)) = self.marks.insert(machine_id, (seq, task.abort_handle())) {
            superseded.abort();
        }
    }

    pub(super) fn cancel(&self, machine_id: Uuid) {
        if let Some((_, (_, task))) = self.marks.remove(&machine_id) {
            task.abort();
        }
    }
}

pub(super) static PENDING_DAEMON_LOST: LazyLock<Arc<PendingDaemonLost>> =
    LazyLock::new(|| Arc::new(PendingDaemonLost::default()));

/// The daemon's WS is gone: every session it announced is now unreachable,
/// so end them as `daemon_lost` — but only if it has not reconnected within
/// [`DAEMON_LOST_GRACE`]. Soft — [`upsert_session`] reverts it when the daemon
/// reconnects and re-registers the session as alive. Only `sessions` are
/// eligible: other daemons on the same machine id keep theirs.
pub(super) fn schedule_daemon_lost(state: &AppState, machine_id: Uuid, sessions: Vec<String>) {
    if sessions.is_empty() {
        tracing::info!(%machine_id, "daemon_lost mark skipped — connection announced no sessions");
        return;
    }
    let state = state.clone();
    PENDING_DAEMON_LOST.schedule(machine_id, DAEMON_LOST_GRACE, async move {
        if daemon_seen_recently(&state.pool, machine_id).await {
            tracing::info!(%machine_id, "daemon_lost mark skipped — machine heartbeating elsewhere");
            return;
        }
        mark_daemon_lost(&state.pool, machine_id, &sessions).await;
    });
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

async fn mark_daemon_lost(pool: &sqlx::PgPool, machine_id: Uuid, sessions: &[String]) {
    match sqlx::query(
        "UPDATE sessions SET status = 'ended', ended_at = now(), end_reason = 'daemon_lost', \
             end_detail = 'daemon connection closed' \
         WHERE machine_uuid = $1 AND id = ANY($2) AND status IN ('new', 'active', 'inactive') \
           AND end_reason IS NULL AND last_heartbeat > now() - interval '1 hour'",
    )
    .bind(machine_id)
    .bind(sessions)
    .execute(pool)
    .await
    {
        Ok(res) if res.rows_affected() > 0 => {
            tracing::info!(%machine_id, count = res.rows_affected(), "sessions marked daemon_lost");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(%err, %machine_id, "daemon_lost mark failed"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn marker() -> (Arc<PendingDaemonLost>, Arc<AtomicUsize>) {
        (Arc::new(PendingDaemonLost::default()), Arc::new(AtomicUsize::new(0)))
    }

    fn mark(marks: &Arc<AtomicUsize>) -> impl Future<Output = ()> + Send + 'static {
        let marks = Arc::clone(marks);
        async move {
            marks.fetch_add(1, Ordering::Relaxed);
        }
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

        pending.schedule(machine, DAEMON_LOST_GRACE, mark(&marked));
        settle().await;
        tokio::time::advance(DAEMON_LOST_GRACE / 2).await;
        pending.cancel(machine);
        tokio::time::advance(DAEMON_LOST_GRACE * 4).await;
        settle().await;

        assert_eq!(marked.load(Ordering::Relaxed), 0);
        assert!(pending.marks.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_daemon_that_never_returns_is_marked_after_the_grace_window() {
        let (pending, marked) = marker();
        let machine = Uuid::new_v4();

        pending.schedule(machine, DAEMON_LOST_GRACE, mark(&marked));
        settle().await;
        tokio::time::advance(DAEMON_LOST_GRACE / 2).await;
        settle().await;
        assert_eq!(marked.load(Ordering::Relaxed), 0);

        tokio::time::advance(DAEMON_LOST_GRACE).await;
        settle().await;
        assert_eq!(marked.load(Ordering::Relaxed), 1);
        assert!(pending.marks.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_second_disconnect_leaves_exactly_one_pending_mark() {
        let (pending, marked) = marker();
        let machine = Uuid::new_v4();
        let other = Uuid::new_v4();

        pending.schedule(machine, DAEMON_LOST_GRACE, mark(&marked));
        pending.cancel(machine);
        pending.schedule(machine, DAEMON_LOST_GRACE, mark(&marked));
        pending.schedule(other, DAEMON_LOST_GRACE, mark(&marked));
        assert_eq!(pending.marks.len(), 2);
        settle().await;

        pending.cancel(other);
        tokio::time::advance(DAEMON_LOST_GRACE * 2).await;
        settle().await;

        assert_eq!(marked.load(Ordering::Relaxed), 1);
        assert!(pending.marks.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_disconnect_without_a_cancel_supersedes_its_predecessor() {
        let (pending, marked) = marker();
        let machine = Uuid::new_v4();

        pending.schedule(machine, DAEMON_LOST_GRACE, mark(&marked));
        settle().await;
        tokio::time::advance(DAEMON_LOST_GRACE / 2).await;
        pending.schedule(machine, DAEMON_LOST_GRACE, mark(&marked));
        assert_eq!(pending.marks.len(), 1);
        settle().await;

        tokio::time::advance(DAEMON_LOST_GRACE).await;
        settle().await;
        assert_eq!(marked.load(Ordering::Relaxed), 1);
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
}
