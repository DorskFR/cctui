//! [`Dispatcher`] backed by an enrolled dispatcher connected over the WS hub.
//! Resolving a dispatch target to an enrolled dispatcher yields one
//! of these; `dispatch`/`status`/`cancel` send the corresponding
//! [`DispatcherFrameDown`] over the dispatcher's live channel and await the
//! request-id-correlated [`DispatcherFrameUp`] reply with a timeout.
//!
//! The server forwards only a [`WireDispatchSpec`]; machine-key lifting and
//! payload semantics live in the executor binary.

use cctui_proto::ws::{DispatcherFrameDown, DispatcherFrameUp, WireDispatchSpec};
use uuid::Uuid;

use super::{DispatchError, DispatchHandle, DispatchSpec, Dispatcher, HandleStatus};
use crate::bus::{Bus, BusError};
use crate::state::AppState;

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
    bus.request_dispatcher(dispatcher_id, request_id, frame).await.map_err(|err| {
        DispatchError::Backend(match err {
            BusError::NoDispatcher(_) => format!("dispatcher '{name}' is offline"),
            BusError::Closed => format!("dispatcher '{name}' connection closed"),
            BusError::Disconnected => {
                format!("dispatcher '{name}' disconnected before replying")
            }
            BusError::Timeout => format!("dispatcher '{name}' did not reply within 30s"),
            other => format!("dispatcher '{name}' round-trip failed: {other}"),
        })
    })
}

#[async_trait::async_trait]
impl Dispatcher for EnrolledDispatcher {
    fn id(&self) -> &str {
        &self.name
    }

    async fn dispatch(&self, spec: &DispatchSpec<'_>) -> Result<DispatchHandle, DispatchError> {
        let request_id = Uuid::new_v4();
        let wire = WireDispatchSpec {
            session_id: spec.session_id.to_owned(),
            timeout_minutes: spec.timeout_minutes,
            reply_url: spec.reply_url.map(ToOwned::to_owned),
            dedup_key: spec.dedup_key.map(ToOwned::to_owned),
            profile: None,
            payload: spec.payload.clone(),
        };
        let reply = self
            .round_trip(request_id, DispatcherFrameDown::Dispatch { request_id, spec: wire })
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
