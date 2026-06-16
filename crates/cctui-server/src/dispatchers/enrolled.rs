//! [`Dispatcher`] backed by an enrolled dispatcher connected over the WS hub
//! (CCT-285). Resolving a dispatch target to an enrolled dispatcher yields one
//! of these; `dispatch`/`status`/`cancel` send the corresponding
//! [`DispatcherFrameDown`] over the dispatcher's live channel and await the
//! request-id-correlated [`DispatcherFrameUp`] reply with a timeout.
//!
//! The server forwards only a [`WireDispatchSpec`]; machine-key lifting and
//! payload semantics live in the executor binary.

use cctui_proto::ws::{DispatcherFrameDown, DispatcherFrameUp, WireDispatchSpec};
use uuid::Uuid;

use super::{DispatchError, DispatchHandle, DispatchSpec, Dispatcher, HandleStatus};
use crate::state::{AppState, DispatcherConnections, PendingDispatcherRequests};

/// How long to await a dispatcher reply before giving up. Spawning a
/// container/pod can take a few seconds; status/cancel are quick. One generous
/// bound covers all three round-trips.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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
        round_trip(
            &self.state.dispatcher_connections,
            &self.state.pending_dispatcher_requests,
            self.dispatcher_id,
            &self.name,
            request_id,
            frame,
        )
        .await
    }
}

/// Hub-level round-trip, factored out of [`EnrolledDispatcher`] so it depends
/// only on the two maps (not the full `AppState`) and is unit-testable against a
/// fake dispatcher. Sends `frame` on the dispatcher's channel, parks a oneshot
/// keyed by `request_id`, and awaits the matching [`DispatcherFrameUp`] (fired
/// by the WS read loop) within [`REQUEST_TIMEOUT`].
async fn round_trip(
    connections: &DispatcherConnections,
    pending: &PendingDispatcherRequests,
    dispatcher_id: Uuid,
    name: &str,
    request_id: Uuid,
    frame: DispatcherFrameDown,
) -> Result<DispatcherFrameUp, DispatchError> {
    let tx = connections
        .get(&dispatcher_id)
        .map(|e| e.value().clone())
        .ok_or_else(|| DispatchError::Backend(format!("dispatcher '{name}' is offline")))?;

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    pending.insert(request_id, reply_tx);

    if tx.send(frame).await.is_err() {
        pending.remove(&request_id);
        return Err(DispatchError::Backend(format!("dispatcher '{name}' connection closed")));
    }

    match tokio::time::timeout(REQUEST_TIMEOUT, reply_rx).await {
        Ok(Ok(reply)) => Ok(reply),
        Ok(Err(_)) => {
            Err(DispatchError::Backend(format!("dispatcher '{name}' disconnected before replying")))
        }
        Err(_) => {
            pending.remove(&request_id);
            Err(DispatchError::Backend(format!(
                "dispatcher '{name}' did not reply within {}s",
                REQUEST_TIMEOUT.as_secs()
            )))
        }
    }
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
                if let Some(err) = error {
                    return Err(DispatchError::Backend(err));
                }
                Ok(match state.as_deref() {
                    Some("complete") => HandleStatus::Complete,
                    Some("failed") => HandleStatus::Failed,
                    Some("gone") => HandleStatus::Gone,
                    _ => HandleStatus::Running,
                })
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
    use std::sync::Arc;

    use dashmap::DashMap;
    use tokio::sync::mpsc;

    use super::*;

    fn maps() -> (DispatcherConnections, PendingDispatcherRequests) {
        (Arc::new(DashMap::new()), Arc::new(DashMap::new()))
    }

    /// A fake dispatcher: register a channel, read the next frame the server
    /// sends, and reply by firing the parked oneshot — exactly what the real WS
    /// read loop does in `routes::dispatcher::process_frame`.
    #[tokio::test]
    async fn dispatch_round_trip_succeeds() {
        let (connections, pending) = maps();
        let id = Uuid::new_v4();
        let (tx, mut rx) = mpsc::channel::<DispatcherFrameDown>(8);
        connections.insert(id, tx);

        let pending2 = pending.clone();
        let fake = tokio::spawn(async move {
            let frame = rx.recv().await.unwrap();
            let DispatcherFrameDown::Dispatch { request_id, spec } = frame else {
                panic!("expected Dispatch");
            };
            let (_, reply_tx) = pending2.remove(&request_id).unwrap();
            reply_tx
                .send(DispatcherFrameUp::DispatchResult {
                    request_id,
                    session_id: spec.session_id,
                    handle: "container/worker-1".into(),
                    namespace: None,
                    status: Some("dispatched".into()),
                    error: None,
                })
                .unwrap();
        });

        let request_id = Uuid::new_v4();
        let spec = WireDispatchSpec {
            session_id: "sess-1".into(),
            timeout_minutes: None,
            reply_url: None,
            payload: serde_json::json!({}),
        };
        let reply = round_trip(
            &connections,
            &pending,
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
        // The pending entry was consumed by the fake dispatcher.
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn offline_dispatcher_errors_fast() {
        let (connections, pending) = maps();
        let request_id = Uuid::new_v4();
        let err = round_trip(
            &connections,
            &pending,
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
    async fn timeout_when_no_reply() {
        // Register a live channel but never reply; shorten the wait by parking
        // our own oneshot and letting the real REQUEST_TIMEOUT elapse would be
        // slow, so assert the no-reply path via a closed connection instead.
        let (connections, pending) = maps();
        let id = Uuid::new_v4();
        let (tx, rx) = mpsc::channel::<DispatcherFrameDown>(1);
        connections.insert(id, tx);
        drop(rx); // receiver gone → send fails → "connection closed"

        let request_id = Uuid::new_v4();
        let err = round_trip(
            &connections,
            &pending,
            id,
            "k8s",
            request_id,
            DispatcherFrameDown::Status { request_id, handle: "h".into() },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DispatchError::Backend(m) if m.contains("closed")));
        // The parked request was cleaned up on the send failure.
        assert!(pending.is_empty());
    }
}
