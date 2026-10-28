//! Ends dispatched sessions whose dispatcher reports their workload dead.
//!
//! For `OOMKilled`, crash loops, un-pullable images or stuck `Pending`, a worker killed
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

use crate::dispatch::DispatchContext;
use crate::dispatchers::{Dispatcher, HandleStatus};
use crate::events::{self, Actor, Event, kind};
use crate::store::dispatch_records;
use crate::store::sessions::SessionRowStatus;

/// Execution operations plus the host's durable completion evidence.
pub trait DispatchLivenessContext: DispatchContext {
    fn execution_finished(&self, session: &str) -> impl std::future::Future<Output = bool> + Send;
}

const SWEEP_LIMIT: i64 = 100;
const PROBE_CONCURRENCY: usize = 8;

/// A finished Job is garbage-collected within an hour, so an older handle only
/// ever answers `gone`.
const HANDLE_MAX_AGE_HOURS: i64 = 48;

/// Server guesses at a lost worker, overwritten by a reported cause.
const TRANSPORT_ENDS: &[&str] = &["daemon_lost", "machine_offline"];

const EXITED_AFTER_COMPLETION: &str = "completed (worker exited non-zero)";

/// A workload completed after an explicit execution completion signal.
pub const EXITED_CLEANLY: &str = "completed (worker exited)";

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

pub async fn sweep(state: &impl DispatchLivenessContext) {
    use futures_util::StreamExt;
    if let Err(err) = crate::store::dispatch_policies::prune_uses(state.dispatch_pool()).await {
        tracing::warn!(%err, "dispatch policy use prune failed");
    }
    if let Err(err) = dispatch_records::prune(state.dispatch_pool()).await {
        tracing::warn!(%err, "dispatch record prune failed");
    }
    if let Err(err) = dispatch_records::mark_registered(state.dispatch_pool()).await {
        tracing::warn!(%err, "dispatch record registration sweep failed");
    }
    match live_handles(state.dispatch_pool()).await {
        Ok(rows) => {
            futures_util::stream::iter(rows)
                .for_each_concurrent(PROBE_CONCURRENCY, |row| probe(state, row))
                .await;
        }
        Err(err) => tracing::warn!(%err, "dispatch liveness sweep query failed"),
    }
    match dispatch_records::unregistered_handles(
        state.dispatch_pool(),
        HANDLE_MAX_AGE_HOURS,
        SWEEP_LIMIT,
    )
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
    state: &impl DispatchLivenessContext,
    user_id: Option<Uuid>,
    dispatcher_name: &str,
    session_id: &str,
    handle: &str,
) -> Option<(HandleStatus, Option<WorkerExit>)> {
    let dispatcher = dispatcher_for(state, user_id, dispatcher_name).await?;
    let (status, exit) = dispatcher.status_detail(handle).await.ok()?;
    let exit = exit.map(|exit| dispatch_records::scrub_worker_exit(&exit));
    if let Some(exit) = &exit
        && let Err(err) =
            dispatch_records::set_worker_exit(state.dispatch_pool(), session_id, exit).await
    {
        tracing::warn!(%err, %session_id, "worker exit not stored");
    }
    Some((status, exit))
}

/// The dispatcher a handle was forwarded to. An admin dispatch resolves the
/// name across owners while its record keeps the caller's own user, so a name
/// that user does not own resolves the way the dispatch did.
async fn dispatcher_for(
    state: &impl DispatchLivenessContext,
    user_id: Option<Uuid>,
    name: &str,
) -> Option<Arc<dyn Dispatcher>> {
    use crate::dispatch::resolve_dispatcher;
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

/// How the session ends for a failed workload: `completed` when it already
/// finished its work, else the cause the failure names.
async fn verdict(
    state: &impl DispatchLivenessContext,
    session_id: &str,
    failure: String,
) -> (SessionEndReason, String) {
    if state.execution_finished(session_id).await {
        (SessionEndReason::Completed, format!("{EXITED_AFTER_COMPLETION}: {failure}"))
    } else {
        (end_reason_for(&failure), failure)
    }
}

async fn probe(state: &impl DispatchLivenessContext, row: LiveHandle) {
    let Some((status, exit)) =
        status_of(state, row.user_id, &row.dispatcher_name, &row.session_id, &row.handle).await
    else {
        return;
    };
    let (end_reason, detail) = match status {
        HandleStatus::Failed(reason) => {
            verdict(state, &row.session_id, failure_text(reason, exit.as_ref())).await
        }
        HandleStatus::Complete if state.execution_finished(&row.session_id).await => {
            (SessionEndReason::Completed, EXITED_CLEANLY.to_owned())
        }
        HandleStatus::Complete => match oom_kill(exit.as_ref()) {
            Some(oom) => (SessionEndReason::OomKilled, oom),
            None => return,
        },
        _ => return,
    };
    if !end_session(state.dispatch_pool(), &row.session_id, end_reason, &detail).await {
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
    events::record(state.dispatch_pool(), state.dispatch_bus(), event);
}

/// A worker that died before registering has no session to end: its
/// dispatch record carries the cause instead. One still in the dispatcher's
/// queue is not yet late.
async fn probe_unregistered(
    state: &impl DispatchLivenessContext,
    row: dispatch_records::UnregisteredHandle,
) {
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
    if let Err(err) = dispatch_records::set_queued(state.dispatch_pool(), &session_id, queued).await
    {
        tracing::warn!(%err, %session_id, "dispatch record queue state not stored");
    }
}

async fn mark_failed(
    state: &impl DispatchLivenessContext,
    session_id: &str,
    dispatcher_name: &str,
    handle: &str,
    detail: &str,
) {
    let end_reason = end_reason_for(detail);
    let marked = dispatch_records::mark_failed(
        state.dispatch_pool(),
        session_id,
        end_reason.as_str(),
        detail,
    )
    .await;
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
    use super::*;
    const BACKOFF: &str = "Job has reached the specified backoff limit";
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
}
