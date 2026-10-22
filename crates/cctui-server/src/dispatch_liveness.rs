//! Ends a dispatched session whose dispatcher reports its workload dead
//! (`OOMKilled`, crash loop, un-pullable image, stuck `Pending`): a worker killed
//! by the kernel never reaches its exit trap, so nothing else reports it.
//!
//! A session whose workflow guard already reached its exit, or that its daemon
//! reported completed, did its work: a failed Job after that is the
//! container's exit code, and the session ends `completed`, as does a Job that
//! finished. Without that evidence a failure is a crash.

use std::sync::Arc;

use cctui_proto::api::WorkerExit;
use cctui_proto::models::SessionEndReason;
use uuid::Uuid;

use crate::dispatchers::{Dispatcher, HandleStatus};
use crate::events::{self, Actor, Event, kind};
use crate::state::AppState;
use crate::store::dispatch_records;
use crate::store::sessions::SessionRowStatus;

const SWEEP_LIMIT: i64 = 100;
const PROBE_CONCURRENCY: usize = 8;

/// A finished Job is garbage-collected within an hour, so an older handle only
/// ever answers `gone`.
const HANDLE_MAX_AGE_HOURS: i64 = 48;

/// Server guesses at a lost worker, overwritten by a reported cause.
const TRANSPORT_ENDS: &[&str] = &["daemon_lost", "machine_offline"];

const EXITED_AFTER_COMPLETION: &str = "completed (worker exited non-zero)";

const EXITED_CLEANLY: &str = "completed (worker exited)";

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
        _ if reason.starts_with("pod pending") || reason.starts_with("preflight:") => {
            SessionEndReason::SpawnFailed
        }
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
    if let Err(err) = crate::store::dispatch_policies::prune_uses(&state.pool).await {
        tracing::warn!(%err, "dispatch policy use prune failed");
    }
    if let Err(err) = dispatch_records::prune(&state.pool).await {
        tracing::warn!(%err, "dispatch record prune failed");
    }
    if let Err(err) = dispatch_records::mark_registered(&state.pool).await {
        tracing::warn!(%err, "dispatch record registration sweep failed");
    }
    match live_handles(&state.pool).await {
        Ok(rows) => {
            futures_util::stream::iter(rows)
                .for_each_concurrent(PROBE_CONCURRENCY, |row| probe(state, row))
                .await;
        }
        Err(err) => tracing::warn!(%err, "dispatch liveness sweep query failed"),
    }
    match dispatch_records::unregistered_handles(&state.pool, HANDLE_MAX_AGE_HOURS, SWEEP_LIMIT)
        .await
    {
        Ok(rows) => {
            futures_util::stream::iter(rows)
                .for_each_concurrent(PROBE_CONCURRENCY, |row| probe_unregistered(state, row))
                .await;
        }
        Err(err) => tracing::warn!(%err, "unregistered dispatch sweep query failed"),
    }
}

/// The dispatcher's view of the workload and how its container ended, kept
/// on the dispatch record; `None` when the dispatcher cannot be asked.
async fn status_of(
    state: &AppState,
    user_id: Option<Uuid>,
    dispatcher_name: &str,
    session_id: &str,
    handle: &str,
) -> Option<(HandleStatus, Option<WorkerExit>)> {
    let dispatcher = dispatcher_for(state, user_id, dispatcher_name).await?;
    let (status, exit) = dispatcher.status_detail(handle).await.ok()?;
    let exit = exit.map(|exit| dispatch_records::scrub_worker_exit(&exit));
    if let Some(exit) = &exit
        && let Err(err) = dispatch_records::set_worker_exit(&state.pool, session_id, exit).await
    {
        tracing::warn!(%err, %session_id, "worker exit not stored");
    }
    Some((status, exit))
}

/// The dispatcher a handle was forwarded to. An admin dispatch resolves the
/// name across owners while its record keeps the caller's own user, so a name
/// that user does not own resolves the way the dispatch did.
async fn dispatcher_for(
    state: &AppState,
    user_id: Option<Uuid>,
    name: &str,
) -> Option<Arc<dyn Dispatcher>> {
    use crate::routes::dispatch::resolve_dispatcher;
    match resolve_dispatcher(state, user_id, name).await {
        Ok(Some(d)) => Some(d),
        _ if user_id.is_none() => None,
        _ => resolve_dispatcher(state, None, name).await.ok().flatten(),
    }
}

/// The kernel's OOM kill of the worker container, which a Job's failure
/// condition or a clean entrypoint exit can hide.
fn oom_kill(exit: Option<&WorkerExit>) -> Option<String> {
    let reason = exit?.reason.as_deref()?.trim();
    (end_reason_for(reason) == SessionEndReason::OomKilled).then(|| reason.to_owned())
}

/// Why the workload failed: the dispatcher's reason, unless it names no
/// specific cause and the worker container was OOM killed.
fn failure_text(reason: Option<String>, exit: Option<&WorkerExit>) -> String {
    match reason {
        Some(reason) if end_reason_for(&reason) != SessionEndReason::Crashed => reason,
        reason => oom_kill(exit).or(reason).unwrap_or_else(|| "workload failed".to_owned()),
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

/// How the session ends for a failed workload: `completed` when it already
/// finished its work, else the cause the failure names.
async fn verdict(
    pool: &sqlx::PgPool,
    session_id: &str,
    failure: String,
) -> (SessionEndReason, String) {
    if finished(pool, session_id).await {
        (SessionEndReason::Completed, format!("{EXITED_AFTER_COMPLETION}: {failure}"))
    } else {
        (end_reason_for(&failure), failure)
    }
}

async fn probe(state: &AppState, row: LiveHandle) {
    let Some((status, exit)) =
        status_of(state, row.user_id, &row.dispatcher_name, &row.session_id, &row.handle).await
    else {
        return;
    };
    let (end_reason, detail) = match status {
        HandleStatus::Failed(reason) => {
            verdict(&state.pool, &row.session_id, failure_text(reason, exit.as_ref())).await
        }
        HandleStatus::Complete if finished(&state.pool, &row.session_id).await => {
            (SessionEndReason::Completed, EXITED_CLEANLY.to_owned())
        }
        HandleStatus::Complete => match oom_kill(exit.as_ref()) {
            Some(oom) => (SessionEndReason::OomKilled, oom),
            None => return,
        },
        _ => return,
    };
    if !end_session(&state.pool, &row.session_id, end_reason, &detail).await {
        return;
    }
    let severity = if end_reason == SessionEndReason::Completed {
        tracing::info!(session_id = %row.session_id, handle = %row.handle, "{detail}");
        events::Severity::Info
    } else {
        tracing::warn!(
            session_id = %row.session_id,
            dispatcher = %row.dispatcher_name,
            handle = %row.handle,
            end_reason = end_reason.as_str(),
            "dispatched session ended: {detail}"
        );
        events::Severity::Warn
    };
    let mut event = Event::new(kind::SESSION_ENDED, Actor::Reaper)
        .severity(severity)
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

/// A worker that died before registering has no session to end: its
/// dispatch record carries the cause instead. One still in the dispatcher's
/// queue is not yet late.
async fn probe_unregistered(state: &AppState, row: dispatch_records::UnregisteredHandle) {
    let dispatch_records::UnregisteredHandle { session_id, user_id, dispatcher_name, handle } = row;
    let Some((status, exit)) =
        status_of(state, user_id, &dispatcher_name, &session_id, &handle).await
    else {
        return;
    };
    let queued = match status {
        HandleStatus::Failed(reason) => {
            let detail = failure_text(reason, exit.as_ref());
            mark_failed(state, &session_id, &dispatcher_name, &handle, &detail).await;
            return;
        }
        HandleStatus::Queued => true,
        HandleStatus::Running => false,
        HandleStatus::Complete | HandleStatus::Gone => return,
    };
    if let Err(err) = dispatch_records::set_queued(&state.pool, &session_id, queued).await {
        tracing::warn!(%err, %session_id, "dispatch record queue state not stored");
    }
}

async fn mark_failed(
    state: &AppState,
    session_id: &str,
    dispatcher_name: &str,
    handle: &str,
    detail: &str,
) {
    let end_reason = end_reason_for(detail);
    let marked =
        dispatch_records::mark_failed(&state.pool, session_id, end_reason.as_str(), detail).await;
    match marked {
        Ok(true) => tracing::warn!(
            %session_id,
            dispatcher = %dispatcher_name,
            %handle,
            end_reason = end_reason.as_str(),
            "dispatched worker died before registering: {detail}"
        ),
        Ok(false) => {}
        Err(err) => tracing::warn!(%err, %session_id, "dispatch record update failed"),
    }
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
            (
                "preflight: k8s API reachable (kubernetes.default.svc:443)",
                SessionEndReason::SpawnFailed,
            ),
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

    #[test]
    fn an_oom_killed_worker_names_the_cause_a_generic_failure_hides() {
        let oom = WorkerExit {
            exit_code: Some(137),
            reason: Some("OOMKilled".into()),
            ..Default::default()
        };
        assert_eq!(failure_text(Some(BACKOFF.into()), Some(&oom)), "OOMKilled");
        assert_eq!(failure_text(None, Some(&oom)), "OOMKilled");
        let limit = "OOMKilled (memory limit 8Gi)";
        assert_eq!(failure_text(Some(limit.into()), Some(&oom)), limit);
        assert_eq!(failure_text(Some("ImagePullBackOff".into()), Some(&oom)), "ImagePullBackOff");
        let error =
            WorkerExit { exit_code: Some(1), reason: Some("Error".into()), ..Default::default() };
        assert_eq!(failure_text(Some(BACKOFF.into()), Some(&error)), BACKOFF);
        assert_eq!(failure_text(Some(BACKOFF.into()), None), BACKOFF);
        assert_eq!(failure_text(None, None), "workload failed");
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
        let Some(url) = crate::routes::gateway::test_db_url("dispatch_liveness_oom_backoff")
        else {
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
