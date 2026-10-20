//! Ends a dispatched session whose dispatcher reports its workload dead
//! (`OOMKilled`, crash loop, un-pullable image, stuck `Pending`): a worker killed
//! by the kernel never reaches its exit trap, so nothing else reports it.

use cctui_proto::models::SessionEndReason;
use uuid::Uuid;

use crate::dispatchers::HandleStatus;
use crate::events::{self, Actor, Event, kind};
use crate::state::AppState;
use crate::store::sessions::SessionRowStatus;

const SWEEP_LIMIT: i64 = 100;
const PROBE_CONCURRENCY: usize = 8;

/// A finished Job is garbage-collected within an hour, so an older handle only
/// ever answers `gone`.
const HANDLE_MAX_AGE_HOURS: i64 = 48;

/// Server guesses at a lost worker, overwritten by a reported cause.
const TRANSPORT_ENDS: &[&str] = &["daemon_lost", "machine_offline"];

/// The persisted end reason for a dispatcher's verbatim failure reason.
///
/// Whatever kept the worker from starting (image, container config,
/// unschedulable pod) is a failed start, so its detail shows on the badge.
#[must_use]
pub fn end_reason_for(reason: &str) -> SessionEndReason {
    let reason = reason.trim();
    let head = reason.split(|c: char| c.is_whitespace() || c == '(').next().unwrap_or(reason);
    match head {
        "OOMKilled" => SessionEndReason::OomKilled,
        "ImagePullBackOff"
        | "ErrImagePull"
        | "InvalidImageName"
        | "CreateContainerError"
        | "CreateContainerConfigError" => SessionEndReason::SpawnFailed,
        _ if reason.starts_with("pod pending") => SessionEndReason::SpawnFailed,
        _ => SessionEndReason::Crashed,
    }
}

#[derive(sqlx::FromRow)]
struct LiveHandle {
    session_id: String,
    user_id: Option<Uuid>,
    machine_uuid: Option<Uuid>,
    dispatcher_name: String,
    handle: String,
}

async fn live_handles(pool: &sqlx::PgPool) -> sqlx::Result<Vec<LiveHandle>> {
    sqlx::query_as(
        "SELECT dh.session_id, s.user_id, s.machine_uuid, dh.dispatcher_name, dh.handle \
         FROM dispatch_handles dh \
         JOIN sessions s ON s.id = dh.session_id \
         WHERE dh.created_at > now() - ($1 || ' hours')::interval \
           AND (s.status = ANY($2) OR (s.status = 'ended' AND s.end_reason = ANY($3))) \
         ORDER BY dh.created_at \
         LIMIT $4",
    )
    .bind(HANDLE_MAX_AGE_HOURS.to_string())
    .bind(SessionRowStatus::names(SessionRowStatus::RUNNING))
    .bind(TRANSPORT_ENDS)
    .bind(SWEEP_LIMIT)
    .fetch_all(pool)
    .await
}

pub async fn sweep(state: &AppState) {
    use futures_util::StreamExt;
    let rows = match live_handles(&state.pool).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(%err, "dispatch liveness sweep query failed");
            return;
        }
    };
    futures_util::stream::iter(rows)
        .for_each_concurrent(PROBE_CONCURRENCY, |row| probe(state, row))
        .await;
}

async fn probe(state: &AppState, row: LiveHandle) {
    let Ok(Some(dispatcher)) =
        crate::routes::dispatch::resolve_dispatcher(state, row.user_id, &row.dispatcher_name).await
    else {
        return;
    };
    let Ok(HandleStatus::Failed(reason)) = dispatcher.status(&row.handle).await else {
        return;
    };
    let detail = reason.unwrap_or_else(|| "workload failed".to_owned());
    let end_reason = end_reason_for(&detail);
    if !end_session(&state.pool, &row.session_id, end_reason, &detail).await {
        return;
    }
    tracing::warn!(
        session_id = %row.session_id,
        dispatcher = %row.dispatcher_name,
        handle = %row.handle,
        end_reason = end_reason.as_str(),
        "dispatched session ended: {detail}"
    );
    let mut event = Event::new(kind::SESSION_ENDED, Actor::Reaper)
        .severity(events::Severity::Warn)
        .session(row.session_id)
        .detail(serde_json::json!({
            "end_reason": end_reason.as_str(),
            "end_detail": detail,
            "dispatcher": row.dispatcher_name,
            "handle": row.handle,
        }));
    if let Some(machine) = row.machine_uuid {
        event = event.machine(machine);
    }
    events::record(state, event);
}

async fn end_session(
    pool: &sqlx::PgPool,
    session_id: &str,
    reason: SessionEndReason,
    detail: &str,
) -> bool {
    match sqlx::query(
        "UPDATE sessions \
         SET status = 'ended', ended_at = COALESCE(ended_at, now()), end_reason = $2, \
             end_detail = $3 \
         WHERE id = $1 \
           AND ((status = ANY($4) AND end_reason IS NULL) \
                OR (status = 'ended' AND end_reason = ANY($5)))",
    )
    .bind(session_id)
    .bind(reason.as_str())
    .bind(detail)
    .bind(SessionRowStatus::names(SessionRowStatus::RUNNING))
    .bind(TRANSPORT_ENDS)
    .execute(pool)
    .await
    {
        Ok(done) => done.rows_affected() > 0,
        Err(err) => {
            tracing::warn!(%err, %session_id, "dispatched session end failed");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::ws::{DispatcherFrameDown, DispatcherFrameUp};
    use tokio::sync::mpsc;

    use super::*;

    #[test]
    fn every_dispatcher_failure_reason_maps_to_an_end_reason() {
        for (reason, want) in [
            ("OOMKilled", SessionEndReason::OomKilled),
            ("OOMKilled (memory limit 8Gi)", SessionEndReason::OomKilled),
            ("CrashLoopBackOff", SessionEndReason::Crashed),
            ("ImagePullBackOff", SessionEndReason::SpawnFailed),
            ("ErrImagePull", SessionEndReason::SpawnFailed),
            ("InvalidImageName", SessionEndReason::SpawnFailed),
            ("CreateContainerConfigError", SessionEndReason::SpawnFailed),
            ("pod pending for 300s (unschedulable?)", SessionEndReason::SpawnFailed),
            ("pod failed", SessionEndReason::Crashed),
            ("container restart-looping", SessionEndReason::Crashed),
            ("container exited with code 137", SessionEndReason::Crashed),
            ("Job has reached the specified backoff limit", SessionEndReason::Crashed),
            ("workload failed", SessionEndReason::Crashed),
            ("", SessionEndReason::Crashed),
        ] {
            assert_eq!(end_reason_for(reason), want, "{reason:?}");
        }
    }

    async fn status(pool: &sqlx::PgPool, id: &str) -> (String, Option<String>, Option<String>) {
        sqlx::query_as("SELECT status, end_reason, end_detail FROM sessions WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn seed_dispatcher(pool: &sqlx::PgPool, uid: Uuid, name: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO dispatchers (user_id, name, kind, key_hash, key_preview) \
             VALUES ($1, $2, 'kubernetes', $3, 'dl-test') RETURNING id",
        )
        .bind(uid)
        .bind(name)
        .bind(format!("dk-{uid}-{name}"))
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn seed_handle(pool: &sqlx::PgPool, session: &str, dispatcher: &str, handle: &str) {
        sqlx::query(
            "INSERT INTO dispatch_handles (session_id, dispatcher_name, handle) \
             VALUES ($1, $2, $3)",
        )
        .bind(session)
        .bind(dispatcher)
        .bind(handle)
        .execute(pool)
        .await
        .unwrap();
    }

    fn fake_dispatcher(
        state: &AppState,
        id: Uuid,
        verdict: (Option<&'static str>, Option<&'static str>),
    ) -> tokio::task::JoinHandle<()> {
        let (tx, mut rx) = mpsc::channel::<DispatcherFrameDown>(8);
        state.bus.register_dispatcher(id, tx);
        let bus = state.bus.clone();
        tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                let DispatcherFrameDown::Status { request_id, handle } = frame else {
                    panic!("expected Status, got {frame:?}");
                };
                assert!(bus.resolve_dispatcher_reply(
                    request_id,
                    DispatcherFrameUp::StatusResult {
                        request_id,
                        handle,
                        state: verdict.0.map(Into::into),
                        error: verdict.1.map(Into::into),
                    },
                ));
            }
        })
    }

    #[tokio::test]
    async fn an_oom_killed_worker_ends_its_session_with_the_reason() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_liveness_oom") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, machine) = crate::events::tests::seed_user_machine(&pool, "dl-oom").await;
        let name = format!("k8s-{uid}");
        let dispatcher = seed_dispatcher(&pool, uid, &name).await;

        let oom = crate::events::tests::seed_session(&pool, uid, machine, Some("oom")).await;
        let lost = crate::events::tests::seed_session(&pool, uid, machine, Some("lost")).await;
        let done = crate::events::tests::seed_session(&pool, uid, machine, Some("done")).await;
        let bare = crate::events::tests::seed_session(&pool, uid, machine, Some("bare")).await;
        sqlx::query(
            "UPDATE sessions SET status = 'ended', end_reason = 'daemon_lost', \
             end_detail = 'daemon connection closed' WHERE id = $1",
        )
        .bind(&lost)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE sessions SET status = 'ended', end_reason = 'completed' WHERE id = $1")
            .bind(&done)
            .execute(&pool)
            .await
            .unwrap();
        for sid in [&oom, &lost, &done] {
            seed_handle(&pool, sid, &name, &format!("jobs/cctui-worker-{sid}")).await;
        }

        let fake = fake_dispatcher(
            &state,
            dispatcher,
            (Some("failed"), Some("OOMKilled (memory limit 8Gi)")),
        );
        sweep(&state).await;
        fake.abort();

        let want = (
            "ended".to_owned(),
            Some("oom_killed".to_owned()),
            Some("OOMKilled (memory limit 8Gi)".to_owned()),
        );
        assert_eq!(status(&pool, &oom).await, want, "a live session ends with the cause");
        assert_eq!(status(&pool, &lost).await, want, "a daemon_lost end is upgraded to the cause");
        assert_eq!(
            status(&pool, &done).await,
            ("ended".into(), Some("completed".into()), None),
            "a clean end is never rewritten"
        );
        assert_eq!(
            status(&pool, &bare).await,
            ("active".into(), None, None),
            "a session without a dispatch handle is never probed"
        );
    }

    #[tokio::test]
    async fn a_running_or_gone_workload_leaves_the_session_alone() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_liveness_running") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, machine) = crate::events::tests::seed_user_machine(&pool, "dl-run").await;
        let name = format!("k8s-{uid}");
        let dispatcher = seed_dispatcher(&pool, uid, &name).await;
        let sid = crate::events::tests::seed_session(&pool, uid, machine, Some("run")).await;
        seed_handle(&pool, &sid, &name, "jobs/cctui-worker-run").await;

        for verdict in [(Some("running"), None), (Some("gone"), None), (None, Some("api down"))] {
            let fake = fake_dispatcher(&state, dispatcher, verdict);
            sweep(&state).await;
            fake.abort();
            assert_eq!(status(&pool, &sid).await, ("active".into(), None, None), "{verdict:?}");
        }
    }
}
