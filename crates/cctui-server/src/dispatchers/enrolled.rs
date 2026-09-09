//! [`Dispatcher`] backed by an enrolled dispatcher connected over the WS hub.
//! Resolving a dispatch target to an enrolled dispatcher yields one
//! of these; `dispatch`/`status`/`cancel` send the corresponding
//! [`DispatcherFrameDown`] over the dispatcher's live channel and await the
//! request-id-correlated [`DispatcherFrameUp`] reply with a timeout.
//!
//! The server forwards only a [`WireDispatchSpec`]; machine-key lifting and
//! payload semantics live in the executor binary.

use std::sync::LazyLock;
use std::time::Duration;

use cctui_proto::ws::{DispatcherFrameDown, DispatcherFrameUp, WireDispatchSpec};
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::{DispatchError, DispatchHandle, DispatchSpec, Dispatcher, HandleStatus};
use crate::bus::{Bus, BusError};
use crate::state::AppState;

/// How long a dispatch is held while the dispatcher is not connected. A release
/// rolling-restarts the dispatcher and the observed re-enrol gap is ~30s; 45s
/// covers it with margin yet stays far below the caller's HTTP timeout, so a
/// dead dispatcher still fails the request rather than hanging it.
const DISPATCH_HOLD: Duration = Duration::from_secs(45);

/// Backoff between hold attempts, capped at the last entry.
const HOLD_BACKOFF_SECS: &[u64] = &[1, 2, 4, 8];

/// How many dispatches may be held concurrently while the dispatcher is away;
/// the rest fail fast on the existing loud path (error log, 502, ntfy) so the
/// caller can reconcile its claim. A dispatcher restart is ~30s and dispatches
/// arrive in bursts of at most a handful (n8n retries, a wave of tagged
/// tickets), so 32 covers any realistic burst several times over while
/// bounding held connections and tasks to a few dozen per replica.
const MAX_HELD_DISPATCHES: usize = 32;

/// Process-wide permits for [`dispatch_held`]; one dispatcher hub per process,
/// so one cap.
static HELD: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(MAX_HELD_DISPATCHES));

pub struct EnrolledDispatcher {
    /// The dispatcher's display name (the `dispatcher` field of the caller's
    /// request) — surfaced as the dispatcher id to the caller.
    name: String,
    dispatcher_id: Uuid,
    state: AppState,
}

impl EnrolledDispatcher {
    pub fn new(name: impl Into<String>, dispatcher_id: Uuid, state: AppState) -> Self {
        Self { name: name.into(), dispatcher_id, state }
    }

    /// Send a frame over the dispatcher's live channel and await the
    /// request-id-correlated reply. Offline / closed / timeout all surface as
    /// [`DispatchError::Backend`].
    async fn round_trip(
        &self,
        request_id: Uuid,
        frame: DispatcherFrameDown,
    ) -> Result<DispatcherFrameUp, DispatchError> {
        round_trip(&self.state.bus, self.dispatcher_id, &self.name, request_id, frame).await
    }
}

/// Hub-level round-trip, factored out of [`EnrolledDispatcher`] so it depends
/// only on the [`Bus`] (not the full `AppState`) and is unit-testable against a
/// fake dispatcher. The correlated send/await (parked oneshot, timeout) lives
/// in [`Bus::request_dispatcher`]; this maps its errors onto the dispatcher's
/// human-readable [`DispatchError::Backend`] messages.
async fn round_trip(
    bus: &Bus,
    dispatcher_id: Uuid,
    name: &str,
    request_id: Uuid,
    frame: DispatcherFrameDown,
) -> Result<DispatcherFrameUp, DispatchError> {
    bus.request_dispatcher(dispatcher_id, request_id, frame)
        .await
        .map_err(|err| backend_error(name, &err))
}

fn backend_error(name: &str, err: &BusError) -> DispatchError {
    DispatchError::Backend(match err {
        BusError::NoDispatcher(_) => format!("dispatcher '{name}' is offline"),
        BusError::Closed => format!("dispatcher '{name}' connection closed"),
        BusError::Disconnected => {
            format!("dispatcher '{name}' disconnected before replying")
        }
        BusError::Timeout => format!("dispatcher '{name}' did not reply within 30s"),
        other => format!("dispatcher '{name}' round-trip failed: {other}"),
    })
}

/// True when the dispatch frame provably never reached a dispatcher, so
/// re-sending it cannot duplicate work.
const fn never_delivered(err: &BusError) -> bool {
    matches!(err, BusError::NoDispatcher(_) | BusError::Closed)
}

/// Send a `Dispatch` frame, holding it across a dispatcher restart rather than
/// failing the caller. At most [`MAX_HELD_DISPATCHES`] are held at once; beyond
/// that a dispatch fails immediately with the same "nothing was dispatched"
/// contract as an expired hold.
///
/// Retrying is safe only because a frame that never reached a dispatcher
/// registered nothing — no dedup key, no Job — so nothing is duplicated and
/// nothing is left claimed when the hold expires unplaced.
async fn dispatch_held(
    bus: &Bus,
    dispatcher_id: Uuid,
    name: &str,
    spec: &WireDispatchSpec,
    hold: Duration,
) -> Result<DispatcherFrameUp, DispatchError> {
    dispatch_held_with(&HELD, bus, dispatcher_id, name, spec, hold).await
}

#[allow(clippy::significant_drop_tightening)]
async fn dispatch_held_with(
    permits: &Semaphore,
    bus: &Bus,
    dispatcher_id: Uuid,
    name: &str,
    spec: &WireDispatchSpec,
    hold: Duration,
) -> Result<DispatcherFrameUp, DispatchError> {
    let deadline = tokio::time::Instant::now() + hold;
    let mut attempt = 0usize;
    let mut permit = None;
    loop {
        let request_id = Uuid::new_v4();
        let frame = DispatcherFrameDown::Dispatch { request_id, spec: spec.clone() };
        let err = match bus.request_dispatcher(dispatcher_id, request_id, frame).await {
            Ok(reply) => return Ok(reply),
            Err(err) => err,
        };

        let now = tokio::time::Instant::now();
        let mut expired = now >= deadline;
        if never_delivered(&err) && !expired && permit.is_none() {
            if let Ok(p) = permits.try_acquire() {
                permit = Some(p);
            } else {
                tracing::warn!(
                    dispatcher = name,
                    session = %spec.session_id,
                    cap = MAX_HELD_DISPATCHES,
                    "dispatcher unreachable ({err}) and the held-dispatch cap is reached; \
                     failing fast instead of holding"
                );
                expired = true;
            }
        }
        if !never_delivered(&err) || expired {
            if never_delivered(&err) {
                tracing::error!(
                    dispatcher = name,
                    session = %spec.session_id,
                    attempts = attempt + 1,
                    held_secs = hold.as_secs(),
                    "dispatch could not be placed before the hold expired — nothing was \
                     dispatched, the caller must retry or release the claim"
                );
            }
            return Err(backend_error(name, &err));
        }

        let backoff =
            Duration::from_secs(HOLD_BACKOFF_SECS[attempt.min(HOLD_BACKOFF_SECS.len() - 1)]);
        tracing::warn!(
            dispatcher = name,
            session = %spec.session_id,
            attempt = attempt + 1,
            "dispatcher unreachable ({err}); holding dispatch and retrying in {}s",
            backoff.as_secs()
        );
        tokio::time::sleep_until((now + backoff).min(deadline)).await;
        attempt = attempt.saturating_add(1);
    }
}

#[async_trait::async_trait]
impl Dispatcher for EnrolledDispatcher {
    fn id(&self) -> &str {
        &self.name
    }

    async fn dispatch(&self, spec: &DispatchSpec<'_>) -> Result<DispatchHandle, DispatchError> {
        let wire = WireDispatchSpec {
            session_id: spec.session_id.to_owned(),
            timeout_minutes: spec.timeout_minutes,
            reply_url: spec.reply_url.map(ToOwned::to_owned),
            dedup_key: spec.dedup_key.map(ToOwned::to_owned),
            profile: None,
            payload: spec.payload.clone(),
        };
        let reply =
            dispatch_held(&self.state.bus, self.dispatcher_id, &self.name, &wire, DISPATCH_HOLD)
                .await?;
        match reply {
            DispatcherFrameUp::DispatchResult { handle, namespace, status, error, .. } => {
                if let Some(err) = error {
                    return Err(DispatchError::Backend(err));
                }
                Ok(DispatchHandle { handle, namespace, status })
            }
            other => Err(DispatchError::Backend(format!("unexpected dispatcher reply: {other:?}"))),
        }
    }

    async fn status(&self, handle: &str) -> Result<HandleStatus, DispatchError> {
        let request_id = Uuid::new_v4();
        let reply = self
            .round_trip(
                request_id,
                DispatcherFrameDown::Status { request_id, handle: handle.to_owned() },
            )
            .await?;
        match reply {
            DispatcherFrameUp::StatusResult { state, error, .. } => {
                // `error` alongside a `failed` state is the failure *reason*
                // (CrashLoopBackOff / OOMKilled / non-zero exit), not a
                // transport error — only treat it as a hard error when the
                // dispatcher reported no state at all (couldn't introspect).
                match state.as_deref() {
                    Some("complete") => Ok(HandleStatus::Complete),
                    Some("failed") => Ok(HandleStatus::Failed(error)),
                    Some("gone") => Ok(HandleStatus::Gone),
                    Some(_) => Ok(HandleStatus::Running),
                    None => Err(DispatchError::Backend(
                        error.unwrap_or_else(|| "dispatcher returned no status".into()),
                    )),
                }
            }
            other => Err(DispatchError::Backend(format!("unexpected dispatcher reply: {other:?}"))),
        }
    }

    async fn cancel(&self, handle: &str) -> Result<(), DispatchError> {
        let request_id = Uuid::new_v4();
        let reply = self
            .round_trip(
                request_id,
                DispatcherFrameDown::Cancel { request_id, handle: handle.to_owned() },
            )
            .await?;
        match reply {
            DispatcherFrameUp::CancelResult { ok, error, .. } => {
                if ok {
                    Ok(())
                } else {
                    Err(DispatchError::Backend(
                        error.unwrap_or_else(|| "dispatcher reported cancel failure".into()),
                    ))
                }
            }
            other => Err(DispatchError::Backend(format!("unexpected dispatcher reply: {other:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;
    use crate::bus::NoopTransport;

    fn bus() -> Bus {
        Bus::new(Box::new(NoopTransport))
    }

    /// A fake dispatcher: register a channel, read the next frame the server
    /// sends, and reply by firing the parked oneshot — exactly what the real WS
    /// read loop does in `routes::dispatcher::process_frame`.
    #[tokio::test]
    async fn dispatch_round_trip_succeeds() {
        let bus = bus();
        let id = Uuid::new_v4();
        let (tx, mut rx) = mpsc::channel::<DispatcherFrameDown>(8);
        bus.register_dispatcher(id, tx);

        let bus2 = bus.clone();
        let fake = tokio::spawn(async move {
            let frame = rx.recv().await.unwrap();
            let DispatcherFrameDown::Dispatch { request_id, spec } = frame else {
                panic!("expected Dispatch");
            };
            assert!(bus2.resolve_dispatcher_reply(
                request_id,
                DispatcherFrameUp::DispatchResult {
                    request_id,
                    session_id: spec.session_id,
                    handle: "container/worker-1".into(),
                    namespace: None,
                    status: Some("dispatched".into()),
                    error: None,
                },
            ));
        });

        let request_id = Uuid::new_v4();
        let spec = WireDispatchSpec {
            session_id: "sess-1".into(),
            timeout_minutes: None,
            reply_url: None,
            dedup_key: None,
            profile: None,
            payload: serde_json::json!({}),
        };
        let reply = round_trip(
            &bus,
            id,
            "k8s",
            request_id,
            DispatcherFrameDown::Dispatch { request_id, spec },
        )
        .await
        .unwrap();
        match reply {
            DispatcherFrameUp::DispatchResult { handle, status, .. } => {
                assert_eq!(handle, "container/worker-1");
                assert_eq!(status.as_deref(), Some("dispatched"));
            }
            other => panic!("unexpected reply: {other:?}"),
        }
        fake.await.unwrap();
    }

    fn spec(session: &str) -> WireDispatchSpec {
        WireDispatchSpec {
            session_id: session.into(),
            timeout_minutes: None,
            reply_url: None,
            dedup_key: Some("dedup-1".into()),
            profile: None,
            payload: serde_json::json!({}),
        }
    }

    /// A rolling restart: the dispatcher is absent when the dispatch arrives and
    /// re-enrols inside the hold. The caller must see a success, not an error.
    #[tokio::test]
    async fn dispatch_survives_a_dispatcher_restart_within_the_hold() {
        let bus = bus();
        let id = Uuid::new_v4();

        let late = bus.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1200)).await;
            let (tx, mut rx) = mpsc::channel::<DispatcherFrameDown>(8);
            late.register_dispatcher(id, tx);
            let frame = rx.recv().await.unwrap();
            let DispatcherFrameDown::Dispatch { request_id, spec } = frame else {
                panic!("expected Dispatch");
            };
            assert!(late.resolve_dispatcher_reply(
                request_id,
                DispatcherFrameUp::DispatchResult {
                    request_id,
                    session_id: spec.session_id,
                    handle: "job/worker-1".into(),
                    namespace: Some("cctui".into()),
                    status: Some("dispatched".into()),
                    error: None,
                },
            ));
        });

        let reply =
            dispatch_held(&bus, id, "k8s", &spec("sess-restart"), Duration::from_secs(10)).await;
        match reply.unwrap() {
            DispatcherFrameUp::DispatchResult { handle, error, .. } => {
                assert_eq!(handle, "job/worker-1");
                assert!(error.is_none());
            }
            other => panic!("unexpected reply: {other:?}"),
        }
    }

    /// Past the bound the dispatch fails loudly. Nothing was ever sent, so no
    /// dedup key / Job exists for it — the caller is free to retry.
    #[tokio::test]
    async fn dispatch_beyond_the_hold_errors_and_sends_nothing() {
        let bus = bus();
        let id = Uuid::new_v4();
        let (tx, mut rx) = mpsc::channel::<DispatcherFrameDown>(8);

        let err = dispatch_held(&bus, id, "k8s", &spec("sess-lost"), Duration::from_millis(1500))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, DispatchError::Backend(m) if m.contains("k8s") && m.contains("offline"))
        );

        bus.register_dispatcher(id, tx);
        assert!(rx.try_recv().is_err(), "no frame may reach a dispatcher that never held one");
    }

    /// With every permit taken, a dispatch that would need to hold fails at
    /// once instead of joining the hold. Once a held dispatch ends its permit is
    /// free again, and a dispatcher that is present never needs one.
    #[tokio::test]
    async fn held_dispatches_are_capped() {
        let held = std::sync::Arc::new(Semaphore::new(1));
        let bus = bus();
        let id = Uuid::new_v4();

        let holder = {
            let (held, bus) = (held.clone(), bus.clone());
            tokio::spawn(async move {
                dispatch_held_with(&held, &bus, id, "k8s", &spec("sess-a"), Duration::from_secs(2))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(held.available_permits(), 0);

        let started = tokio::time::Instant::now();
        let err =
            dispatch_held_with(&held, &bus, id, "k8s", &spec("sess-b"), Duration::from_secs(10))
                .await
                .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1), "over the cap must fail fast");
        assert!(matches!(&err, DispatchError::Backend(m) if m.contains("offline")));

        assert!(holder.await.unwrap().is_err());
        assert_eq!(held.available_permits(), 1, "permit returns when the hold ends");

        let (tx, mut rx) = mpsc::channel::<DispatcherFrameDown>(8);
        bus.register_dispatcher(id, tx);
        let bus2 = bus.clone();
        let fake = tokio::spawn(async move {
            let DispatcherFrameDown::Dispatch { request_id, spec } = rx.recv().await.unwrap()
            else {
                panic!("expected Dispatch");
            };
            assert!(bus2.resolve_dispatcher_reply(
                request_id,
                DispatcherFrameUp::DispatchResult {
                    request_id,
                    session_id: spec.session_id,
                    handle: "job/worker-2".into(),
                    namespace: None,
                    status: Some("dispatched".into()),
                    error: None,
                },
            ));
        });
        let exhausted = std::sync::Arc::new(Semaphore::new(0));
        dispatch_held_with(&exhausted, &bus, id, "k8s", &spec("sess-c"), Duration::from_secs(2))
            .await
            .expect("a present dispatcher needs no hold permit");
        fake.await.unwrap();
    }

    #[tokio::test]
    async fn offline_dispatcher_errors_fast() {
        let bus = bus();
        let request_id = Uuid::new_v4();
        let err = round_trip(
            &bus,
            Uuid::new_v4(),
            "k8s",
            request_id,
            DispatcherFrameDown::Status { request_id, handle: "h".into() },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DispatchError::Backend(m) if m.contains("offline")));
    }

    #[tokio::test]
    async fn closed_connection_errors_fast() {
        // Register a live channel whose receiver is gone: send fails →
        // "connection closed" (the parked request is cleaned up inside the bus).
        let bus = bus();
        let id = Uuid::new_v4();
        let (tx, rx) = mpsc::channel::<DispatcherFrameDown>(1);
        bus.register_dispatcher(id, tx);
        drop(rx);

        let request_id = Uuid::new_v4();
        let err = round_trip(
            &bus,
            id,
            "k8s",
            request_id,
            DispatcherFrameDown::Status { request_id, handle: "h".into() },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DispatchError::Backend(m) if m.contains("closed")));
    }
}
