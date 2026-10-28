//! Dispatcher liveness with the full workspace's completion evidence.
use crate::state::AppState;
use cctui_server_core::dispatch_liveness::DispatchLivenessContext;
pub use cctui_server_core::dispatch_liveness::sweep;

impl DispatchLivenessContext for AppState {
    async fn execution_finished(&self, session: &str) -> bool {
        finished(&self.pool, session).await
    }
}

/// Whether the session's workflow guard reached its exit, read from this
/// replica's mirror, then the table any replica wrote. A session end by the
/// daemon forgets the step, but such a session is never probed.
async fn guard_exited(pool: &sqlx::PgPool, session_id: &str) -> bool {
    if crate::guard_step::get(session_id).is_some_and(|step| step.exited()) {
        return true;
    }
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM guard_steps \
                        WHERE session_id = $1 AND (state->>'step')::bigint < 0)",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|err| {
        tracing::warn!(%err, %session_id, "guard step read failed");
        false
    })
}

/// Whether the daemon reported the session `completed`: a guard-less
/// dispatch ends itself so at the worker's shutdown.
async fn daemon_completed(pool: &sqlx::PgPool, session_id: &str) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM stream_events \
                        WHERE session_id = $1 AND event_type = 'session_ended' \
                          AND payload->'reason'->>'kind' = 'completed')",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|err| {
        tracing::warn!(%err, %session_id, "session end read failed");
        false
    })
}

async fn finished(pool: &sqlx::PgPool, session_id: &str) -> bool {
    guard_exited(pool, session_id).await || daemon_completed(pool, session_id).await
}

#[cfg(test)]
use crate::store::dispatch_records;
#[cfg(test)]
use cctui_proto::api::WorkerExit;
#[cfg(test)]
use cctui_server_core::dispatch_liveness::EXITED_CLEANLY;
#[cfg(test)]
use uuid::Uuid;

#[cfg(test)]
mod tests {
    use cctui_proto::ws::{DispatcherFrameDown, DispatcherFrameUp};
    use tokio::sync::mpsc;

    use super::*;

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
        fake_dispatcher_exiting(state, id, verdict, None)
    }

    fn fake_dispatcher_exiting(
        state: &AppState,
        id: Uuid,
        verdict: (Option<&'static str>, Option<&'static str>),
        worker_exit: Option<cctui_proto::api::WorkerExit>,
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
                        worker_exit: worker_exit.clone(),
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

    /// A Job that ran out of backoff after its worker was OOM killed reports
    /// only the backoff; the container's termination names the cause.
    #[tokio::test]
    async fn an_oom_kill_behind_a_backoff_failure_ends_the_session_oom_killed() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_liveness_oom_backoff") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, machine) = crate::events::tests::seed_user_machine(&pool, "dl-oom-bo").await;
        let name = format!("k8s-{uid}");
        let dispatcher = seed_dispatcher(&pool, uid, &name).await;
        let live = crate::events::tests::seed_session(&pool, uid, machine, Some("live")).await;
        let lost = crate::events::tests::seed_session(&pool, uid, machine, Some("lost")).await;
        sqlx::query(
            "UPDATE sessions SET status = 'ended', end_reason = 'daemon_lost', \
             end_detail = 'daemon connection closed' WHERE id = $1",
        )
        .bind(&lost)
        .execute(&pool)
        .await
        .unwrap();
        for sid in [&live, &lost] {
            seed_handle(&pool, sid, &name, &format!("jobs/cctui-worker-{sid}")).await;
        }
        let exit = WorkerExit {
            container: Some("worker".into()),
            exit_code: Some(137),
            reason: Some("OOMKilled".into()),
            ..Default::default()
        };
        let verdict = (Some("failed"), Some(BACKOFF));
        let fake = fake_dispatcher_exiting(&state, dispatcher, verdict, Some(exit));
        sweep(&state).await;
        fake.abort();

        let want = ("ended".into(), Some("oom_killed".into()), Some("OOMKilled".into()));
        assert_eq!(status(&pool, &live).await, want);
        assert_eq!(status(&pool, &lost).await, want, "a daemon_lost end is upgraded to the cause");
        forget_user(&pool, uid).await;
    }

    /// An OOM kill of a child leaves the entrypoint exiting 0: the Job
    /// completes, and only the container's end names the cause.
    #[tokio::test]
    async fn a_finished_job_whose_worker_was_oom_killed_ends_oom_killed() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_liveness_oom_complete")
        else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, machine) = crate::events::tests::seed_user_machine(&pool, "dl-oom-ok").await;
        let name = format!("k8s-{uid}");
        let dispatcher = seed_dispatcher(&pool, uid, &name).await;
        let stuck = crate::events::tests::seed_session(&pool, uid, machine, Some("stuck")).await;
        let done = crate::events::tests::seed_session(&pool, uid, machine, Some("done")).await;
        let guard = cctui_proto::guard_step::GuardStepState { step: -1, ..Default::default() };
        sqlx::query("INSERT INTO guard_steps (session_id, state) VALUES ($1, $2)")
            .bind(&done)
            .bind(sqlx::types::Json(&guard))
            .execute(&pool)
            .await
            .unwrap();
        for sid in [&stuck, &done] {
            seed_handle(&pool, sid, &name, &format!("jobs/cctui-worker-{sid}")).await;
        }
        let oom = "OOMKilled (memory limit 8Gi)";
        let fake = fake_dispatcher(&state, dispatcher, (Some("complete"), Some(oom)));
        sweep(&state).await;
        fake.abort();

        assert_eq!(
            status(&pool, &stuck).await,
            ("ended".into(), Some("oom_killed".into()), Some(oom.into())),
            "a session left working by its dead worker learns why"
        );
        assert_eq!(
            status(&pool, &done).await,
            ("ended".into(), Some("completed".into()), Some(EXITED_CLEANLY.into())),
            "work already done stays completed"
        );
        forget_user(&pool, uid).await;
    }

    #[tokio::test]
    async fn a_worker_that_dies_before_registering_records_the_cause() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_liveness_unregistered")
        else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, _machine) = crate::events::tests::seed_user_machine(&pool, "dl-unreg").await;
        let name = format!("k8s-{uid}");
        let dispatcher = seed_dispatcher(&pool, uid, &name).await;
        let sid = Uuid::new_v4().to_string();
        let req: cctui_proto::api::DispatchRequest = serde_json::from_value(
            serde_json::json!({"dispatcher": name, "payload": {"prompt": "go"}}),
        )
        .unwrap();
        let record = dispatch_records::NewRecord::from_request(&sid, Some(uid), &req, None, None);
        dispatch_records::insert(&pool, &record).await.unwrap();
        dispatch_records::set_outcome(&pool, &sid, "forwarded", None).await.unwrap();
        seed_handle(&pool, &sid, &name, "jobs/cctui-worker-unreg").await;

        let fake = fake_dispatcher(&state, dispatcher, (Some("failed"), Some("OOMKilled")));
        sweep(&state).await;
        fake.abort();

        let listed = dispatch_records::list_failed(&pool, Some(uid)).await.unwrap();
        assert_eq!(listed.len(), 1, "listed at once, without waiting out the timeout");
        assert_eq!(listed[0].session_id, sid);
        assert_eq!(listed[0].reason, cctui_proto::api::FailedDispatchReason::OomKilled);
        assert_eq!(listed[0].detail.as_deref(), Some("OOMKilled"));
        sqlx::query("DELETE FROM dispatch_handles WHERE session_id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .ok();
    }

    const PREFLIGHT: &str = "preflight: k8s API reachable (kubernetes.default.svc:443)";

    /// An admin dispatch resolves a dispatcher another user owns, while its
    /// record keeps the admin's own user.
    #[tokio::test]
    async fn a_preflight_refusal_on_another_owners_dispatcher_fails_the_record() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_liveness_preflight") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (owner, _) = crate::events::tests::seed_user_machine(&pool, "dl-pf-owner").await;
        let (admin, _) = crate::events::tests::seed_user_machine(&pool, "dl-pf-admin").await;
        let name = format!("k8s-{owner}");
        let dispatcher = seed_dispatcher(&pool, owner, &name).await;
        let sid = Uuid::new_v4().to_string();
        let req: cctui_proto::api::DispatchRequest = serde_json::from_value(
            serde_json::json!({"dispatcher": name, "payload": {"prompt": "go"}}),
        )
        .unwrap();
        let record = dispatch_records::NewRecord::from_request(&sid, Some(admin), &req, None, None);
        dispatch_records::insert(&pool, &record).await.unwrap();
        dispatch_records::set_outcome(&pool, &sid, "forwarded", None).await.unwrap();
        seed_handle(&pool, &sid, &name, "jobs/cctui-worker-preflight").await;

        let exit = cctui_proto::api::WorkerExit {
            container: Some("worker".into()),
            exit_code: Some(1),
            reason: Some(PREFLIGHT.into()),
            ..Default::default()
        };
        let verdict = (Some("failed"), Some(PREFLIGHT));
        let fake = fake_dispatcher_exiting(&state, dispatcher, verdict, Some(exit));
        sweep(&state).await;
        fake.abort();

        let listed = dispatch_records::list_failed(&pool, Some(admin)).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, sid);
        assert_eq!(listed[0].reason, cctui_proto::api::FailedDispatchReason::SpawnFailed);
        assert_eq!(listed[0].detail.as_deref(), Some(PREFLIGHT));
        let exit = listed[0].worker_exit.as_ref().expect("the exit is on the card");
        assert_eq!(exit.exit_code, Some(1));
        sqlx::query("DELETE FROM dispatch_handles WHERE session_id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .ok();
        forget_user(&pool, admin).await;
        forget_user(&pool, owner).await;
    }

    const BACKOFF: &str = "Job has reached the specified backoff limit";

    /// Every machine left behind is swept by each later machine liveness test.
    async fn forget_user(pool: &sqlx::PgPool, uid: Uuid) {
        for sql in [
            "DELETE FROM dispatch_handles WHERE session_id IN \
             (SELECT id FROM sessions WHERE user_id = $1)",
            "DELETE FROM sessions WHERE user_id = $1",
            "DELETE FROM machines WHERE user_id = $1",
            "DELETE FROM dispatchers WHERE user_id = $1",
            "DELETE FROM users WHERE id = $1",
        ] {
            sqlx::query(sql).bind(uid).execute(pool).await.ok();
        }
    }

    /// A dispatched, registered session with a guard at `step` and a Job the
    /// dispatcher reports failed with exit code 1.
    async fn backoff_failure(tag: &str, step: i64) -> Option<(sqlx::PgPool, Uuid, String)> {
        let url = crate::routes::gateway::test_db_url(tag)?;
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, machine) = crate::events::tests::seed_user_machine(&pool, tag).await;
        let name = format!("k8s-{uid}");
        let dispatcher = seed_dispatcher(&pool, uid, &name).await;
        let sid = crate::events::tests::seed_session(&pool, uid, machine, Some(tag)).await;
        let req: cctui_proto::api::DispatchRequest = serde_json::from_value(
            serde_json::json!({"dispatcher": name, "payload": {"prompt": "go", "name": tag}}),
        )
        .unwrap();
        let record = dispatch_records::NewRecord::from_request(&sid, Some(uid), &req, None, None);
        dispatch_records::insert(&pool, &record).await.unwrap();
        dispatch_records::set_outcome(&pool, &sid, "forwarded", None).await.unwrap();
        seed_handle(&pool, &sid, &name, &format!("jobs/cctui-worker-{tag}")).await;
        let guard = cctui_proto::guard_step::GuardStepState { step, ..Default::default() };
        sqlx::query("INSERT INTO guard_steps (session_id, state) VALUES ($1, $2)")
            .bind(&sid)
            .bind(sqlx::types::Json(&guard))
            .execute(&pool)
            .await
            .unwrap();
        let exit = cctui_proto::api::WorkerExit {
            container: Some("worker".into()),
            exit_code: Some(1),
            log_tail: Some("result written\nGITHUB_TOKEN=ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123".into()),
            ..Default::default()
        };
        let fake = fake_dispatcher_exiting(
            &state,
            dispatcher,
            (Some("failed"), Some(BACKOFF)),
            Some(exit),
        );
        sweep(&state).await;
        fake.abort();
        Some((pool, uid, sid))
    }

    #[tokio::test]
    async fn a_failed_job_after_the_guard_exited_is_a_completed_session() {
        let Some((pool, uid, sid)) = backoff_failure("dispatch_liveness_completed", -1).await
        else {
            return;
        };
        assert_eq!(
            status(&pool, &sid).await,
            (
                "ended".into(),
                Some("completed".into()),
                Some(format!("completed (worker exited non-zero): {BACKOFF}")),
            )
        );
        assert!(
            dispatch_records::list_failed(&pool, Some(uid)).await.unwrap().is_empty(),
            "no failure card for work that was done"
        );
        forget_user(&pool, uid).await;
    }

    /// A finished Job after the guard exited is a clean completion, even when
    /// the disconnect already marked the session `daemon_lost`.
    #[tokio::test]
    async fn a_finished_job_after_the_guard_exited_is_a_completed_session() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_liveness_finished") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, machine) = crate::events::tests::seed_user_machine(&pool, "dl-finished").await;
        let name = format!("k8s-{uid}");
        let dispatcher = seed_dispatcher(&pool, uid, &name).await;
        let lost = crate::events::tests::seed_session(&pool, uid, machine, Some("lost")).await;
        let unguarded = crate::events::tests::seed_session(&pool, uid, machine, None).await;
        sqlx::query(
            "UPDATE sessions SET status = 'ended', end_reason = 'daemon_lost', \
             end_detail = 'daemon connection closed' WHERE id = $1",
        )
        .bind(&lost)
        .execute(&pool)
        .await
        .unwrap();
        let guard = cctui_proto::guard_step::GuardStepState { step: -1, ..Default::default() };
        sqlx::query("INSERT INTO guard_steps (session_id, state) VALUES ($1, $2)")
            .bind(&lost)
            .bind(sqlx::types::Json(&guard))
            .execute(&pool)
            .await
            .unwrap();
        for sid in [&lost, &unguarded] {
            seed_handle(&pool, sid, &name, &format!("jobs/cctui-worker-{sid}")).await;
        }

        let fake = fake_dispatcher(&state, dispatcher, (Some("complete"), None));
        sweep(&state).await;
        fake.abort();

        assert_eq!(
            status(&pool, &lost).await,
            ("ended".into(), Some("completed".into()), Some(EXITED_CLEANLY.into())),
        );
        assert_eq!(
            status(&pool, &unguarded).await,
            ("active".into(), None, None),
            "a finished Job alone is no proof the work was done"
        );
        forget_user(&pool, uid).await;
    }

    /// A guard-less dispatch has no guard step: its daemon's own `completed`
    /// end is the evidence, and it upgrades a `daemon_lost` mark.
    #[tokio::test]
    async fn a_finished_job_the_daemon_reported_completed_is_a_completed_session() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_liveness_reported") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, machine) = crate::events::tests::seed_user_machine(&pool, "dl-reported").await;
        let name = format!("k8s-{uid}");
        let dispatcher = seed_dispatcher(&pool, uid, &name).await;
        let reported = crate::events::tests::seed_session(&pool, uid, machine, None).await;
        let crashed = crate::events::tests::seed_session(&pool, uid, machine, None).await;
        for (sid, reason) in [
            (&reported, cctui_proto::adapter::EndReason::Completed),
            (&crashed, cctui_proto::adapter::EndReason::Crashed { detail: "boom".into() }),
        ] {
            sqlx::query(
                "INSERT INTO stream_events (session_id, event_type, payload) \
                 VALUES ($1, 'session_ended', $2)",
            )
            .bind(sid)
            .bind(serde_json::json!({ "reason": reason }))
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "UPDATE sessions SET status = 'ended', end_reason = 'daemon_lost', \
                 end_detail = 'daemon connection closed' WHERE id = $1",
            )
            .bind(sid)
            .execute(&pool)
            .await
            .unwrap();
            seed_handle(&pool, sid, &name, &format!("jobs/cctui-worker-{sid}")).await;
        }

        let fake = fake_dispatcher(&state, dispatcher, (Some("complete"), None));
        sweep(&state).await;
        fake.abort();

        assert_eq!(
            status(&pool, &reported).await,
            ("ended".into(), Some("completed".into()), Some(EXITED_CLEANLY.into())),
        );
        assert_eq!(
            status(&pool, &crashed).await,
            ("ended".into(), Some("daemon_lost".into()), Some("daemon connection closed".into())),
            "only a completed end is evidence"
        );
        forget_user(&pool, uid).await;
    }

    #[tokio::test]
    async fn a_failed_job_without_completion_evidence_stays_a_crash() {
        let Some((pool, uid, sid)) = backoff_failure("dispatch_liveness_crash", 2).await else {
            return;
        };
        assert_eq!(
            status(&pool, &sid).await,
            ("ended".into(), Some("crashed".into()), Some(BACKOFF.into()))
        );
        let listed = dispatch_records::list_failed(&pool, Some(uid)).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].reason, cctui_proto::api::FailedDispatchReason::Crashed);
        assert_eq!(listed[0].detail.as_deref(), Some(BACKOFF));
        let exit = listed[0].worker_exit.as_ref().expect("the exit is on the card");
        assert_eq!(exit.exit_code, Some(1));
        let log = exit.log_tail.as_deref().unwrap();
        assert!(log.starts_with("result written\n"), "{log}");
        assert!(
            !log.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123"),
            "a token reached the card: {log}"
        );
        forget_user(&pool, uid).await;
    }

    async fn queue_state(pool: &sqlx::PgPool, sid: &str) -> (bool, bool) {
        sqlx::query_as(
            "SELECT queued, started_at IS NOT NULL FROM dispatch_records WHERE session_id = $1",
        )
        .bind(sid)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// The sweep follows the dispatcher's queue: a suspended Job is queued,
    /// and its registration clock starts when the dispatcher runs it.
    #[tokio::test]
    async fn an_unregistered_worker_follows_the_dispatchers_queue() {
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_liveness_queued") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, _machine) = crate::events::tests::seed_user_machine(&pool, "dl-queued").await;
        let name = format!("k8s-{uid}");
        let dispatcher = seed_dispatcher(&pool, uid, &name).await;
        let sid = Uuid::new_v4().to_string();
        let req: cctui_proto::api::DispatchRequest = serde_json::from_value(
            serde_json::json!({"dispatcher": name, "payload": {"prompt": "go"}}),
        )
        .unwrap();
        let record = dispatch_records::NewRecord::from_request(&sid, Some(uid), &req, None, None);
        dispatch_records::insert(&pool, &record).await.unwrap();
        dispatch_records::set_outcome(&pool, &sid, "forwarded", None).await.unwrap();
        seed_handle(&pool, &sid, &name, "jobs/cctui-worker-queued").await;
        sqlx::query(
            "UPDATE dispatch_records SET created_at = now() - interval '1 hour' \
             WHERE session_id = $1",
        )
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();

        let fake = fake_dispatcher(&state, dispatcher, (Some("queued"), None));
        sweep(&state).await;
        fake.abort();
        assert_eq!(queue_state(&pool, &sid).await, (true, false));
        assert!(
            dispatch_records::list_failed(&pool, Some(uid)).await.unwrap().is_empty(),
            "an hour in the queue is not a worker that never started"
        );

        let fake = fake_dispatcher(&state, dispatcher, (Some("running"), None));
        sweep(&state).await;
        fake.abort();
        assert_eq!(queue_state(&pool, &sid).await, (false, true), "the clock starts on resume");
        assert!(dispatch_records::list_failed(&pool, Some(uid)).await.unwrap().is_empty());
        forget_user(&pool, uid).await;
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
