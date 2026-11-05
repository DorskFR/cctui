//! Dial-out WS run loop.
//!
//! Connect out to `/api/v1/dispatcher/ws`, send `Hello` + periodic `Heartbeat`,
//! and handle `Dispatch`/`Status`/`Cancel` frames by driving a [`Dispatcher`].
//! Reconnect backoff + half-open detection follow the daemon's pattern so a
//! NAT'd dispatcher recovers the same way.

use std::sync::Arc;
use std::time::Duration;

use cctui_proto::backoff::Backoff;
use cctui_proto::ws::{DispatcherFrameDown, DispatcherFrameUp};
use futures_util::{Sink, SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_util::sync::CancellationToken;

use crate::client::ServerClient;
use crate::dispatcher::{Dispatcher, HandleState};

const PING_INTERVAL: Duration = Duration::from_secs(20);
const LIVENESS_TIMEOUT: Duration = Duration::from_mins(1);

pub struct Runner<D: Dispatcher> {
    client: ServerClient,
    /// Read on every connection attempt, so a rotated key is used on the next
    /// reconnect.
    key: cctui_secrets::Secret,
    spawner: Arc<D>,
}

impl<D: Dispatcher + 'static> Runner<D> {
    #[must_use]
    pub fn new(client: ServerClient, key: cctui_secrets::Secret, spawner: D) -> Self {
        Self { client, key, spawner: Arc::new(spawner) }
    }

    /// Connect/reconnect until `shutdown` fires.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut backoff = Backoff::reconnect();
        loop {
            if shutdown.is_cancelled() {
                return;
            }
            match self.run_once(shutdown.clone()).await {
                Ok(()) => {
                    tracing::info!("dispatcher WS closed cleanly, reconnecting");
                    backoff.reset();
                }
                Err(err) => {
                    let attempt = backoff.attempt();
                    tracing::warn!(%err, attempt, "dispatcher connection failed; retry in ~{:?}", backoff.peek());
                    if !backoff.sleep(&shutdown).await {
                        return;
                    }
                }
            }
        }
    }

    /// Each frame runs on its own task, so a slow dispatch never holds back
    /// the frames behind it; a task outlives a dropped connection, and its
    /// reply is lost with it.
    async fn run_once(&self, shutdown: CancellationToken) -> anyhow::Result<()> {
        let request = self.client.dispatcher_ws_request(&self.key.get())?;
        tracing::info!("connecting to dispatcher WS");
        let (ws, _) = tokio_tungstenite::connect_async(request).await?;
        let (mut sink, mut stream) = ws.split();

        let hello = DispatcherFrameUp::Hello {
            kind: self.spawner.kind().to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        send_frame(&mut sink, &hello).await?;

        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping.tick().await;
        let mut last_rx = tokio::time::Instant::now();
        let (replies_tx, mut replies) = mpsc::unbounded_channel::<DispatcherFrameUp>();

        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => {
                    drop(replies_tx);
                    while let Some(up) = replies.recv().await {
                        send_frame(&mut sink, &up).await?;
                    }
                    let _ = sink.send(Message::Close(None)).await;
                    return Ok(());
                }
                Some(up) = replies.recv() => send_frame(&mut sink, &up).await?,
                msg = stream.next() => {
                    let Some(msg) = msg else { return Ok(()); };
                    let msg = msg?;
                    last_rx = tokio::time::Instant::now();
                    if let Some(frame) = parse_frame(msg)? {
                        let spawner = Arc::clone(&self.spawner);
                        let tx = replies_tx.clone();
                        tokio::spawn(async move {
                            let _ = tx.send(handle_frame(spawner.as_ref(), frame).await);
                        });
                    }
                }
                _ = ping.tick() => {
                    if last_rx.elapsed() > LIVENESS_TIMEOUT {
                        anyhow::bail!(
                            "no server traffic for {}s — WS half-open, reconnecting",
                            last_rx.elapsed().as_secs()
                        );
                    }
                    sink.send(Message::Ping(Vec::new().into())).await?;
                    let hb = DispatcherFrameUp::Heartbeat { sent_at: chrono::Utc::now() };
                    send_frame(&mut sink, &hb).await?;
                }
            }
        }
    }
}

async fn send_frame<S>(sink: &mut S, up: &DispatcherFrameUp) -> anyhow::Result<()>
where
    S: Sink<Message, Error = tungstenite::Error> + Unpin,
{
    sink.send(Message::Text(serde_json::to_string(up)?.into())).await?;
    Ok(())
}

async fn status_frame<D: Dispatcher>(
    spawner: &D,
    request_id: uuid::Uuid,
    handle: String,
) -> DispatcherFrameUp {
    match spawner.status(&handle).await {
        Ok((state, reason)) => {
            let worker_exit = if state == HandleState::Failed {
                spawner.worker_exit(&handle).await
            } else {
                None
            };
            DispatcherFrameUp::StatusResult {
                request_id,
                handle,
                state: Some(state.as_str().to_owned()),
                error: reason,
                worker_exit,
            }
        }
        Err(err) => DispatcherFrameUp::StatusResult {
            request_id,
            handle,
            state: None,
            error: Some(err.to_string()),
            worker_exit: None,
        },
    }
}

#[allow(clippy::cognitive_complexity)]
async fn dispatch_frame<D: Dispatcher>(
    spawner: &D,
    request_id: uuid::Uuid,
    spec: &cctui_proto::ws::WireDispatchSpec,
) -> DispatcherFrameUp {
    let session_id = spec.session_id.clone();
    match spawner.dispatch(spec).await {
        Ok(out) => {
            tracing::info!(%session_id, handle = %out.handle, status = %out.status, "dispatched worker");
            let queue_position = match out.status.as_str() {
                "queued" | "deduplicated" => spawner.queue_position(&out.handle).await,
                _ => None,
            };
            DispatcherFrameUp::DispatchResult {
                request_id,
                session_id,
                handle: out.handle,
                namespace: out.namespace,
                status: Some(out.status),
                queue_position,
                exposure: out.exposure,
                error: None,
            }
        }
        Err(err) => {
            tracing::warn!(%session_id, %err, "dispatch failed");
            DispatcherFrameUp::DispatchResult {
                request_id,
                session_id,
                handle: String::new(),
                namespace: None,
                status: None,
                queue_position: None,
                exposure: None,
                error: Some(err.to_string()),
            }
        }
    }
}

/// Drive a server-sent frame against the dispatcher, producing the reply
/// frame. Errors are reported back in-band (never panic the loop) so the
/// server can surface a dispatch failure instead of hanging.
async fn handle_frame<D: Dispatcher>(spawner: &D, frame: DispatcherFrameDown) -> DispatcherFrameUp {
    match frame {
        DispatcherFrameDown::Dispatch { request_id, spec } => {
            dispatch_frame(spawner, request_id, &spec).await
        }
        DispatcherFrameDown::Status { request_id, handle } => {
            status_frame(spawner, request_id, handle).await
        }
        DispatcherFrameDown::Cancel { request_id, handle } => match spawner.cancel(&handle).await {
            Ok(()) => DispatcherFrameUp::CancelResult { request_id, handle, ok: true, error: None },
            Err(err) => DispatcherFrameUp::CancelResult {
                request_id,
                handle,
                ok: false,
                error: Some(err.to_string()),
            },
        },
        _ => {
            tracing::warn!("unknown dispatcher frame; ignoring");
            DispatcherFrameUp::Heartbeat { sent_at: chrono::Utc::now() }
        }
    }
}

fn parse_frame(msg: Message) -> anyhow::Result<Option<DispatcherFrameDown>> {
    let txt = match msg {
        Message::Text(t) => t.to_string(),
        Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
        _ => return Ok(None),
    };
    Ok(Some(serde_json::from_str(&txt)?))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cctui_proto::ws::{DispatcherFrameDown, DispatcherFrameUp, WireDispatchSpec};
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::Notify;
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_util::sync::CancellationToken;

    use super::{LIVENESS_TIMEOUT, PING_INTERVAL, Runner};
    use crate::client::ServerClient;
    use crate::dispatcher::{Dispatcher, HandleState, SpawnOutcome};

    #[test]
    fn liveness_timeout_allows_at_least_two_missed_pings() {
        assert!(LIVENESS_TIMEOUT >= PING_INTERVAL * 2);
    }

    /// Holds every dispatch until `release` fires.
    struct HeldDispatch {
        release: Arc<Notify>,
    }

    impl Dispatcher for HeldDispatch {
        fn kind(&self) -> &'static str {
            "test"
        }

        async fn dispatch(&self, spec: &WireDispatchSpec) -> anyhow::Result<SpawnOutcome> {
            self.release.notified().await;
            Ok(SpawnOutcome {
                handle: format!("jobs/{}", spec.session_id),
                status: "dispatched".into(),
                namespace: None,
                exposure: None,
            })
        }

        async fn status(&self, _handle: &str) -> anyhow::Result<(HandleState, Option<String>)> {
            Ok((HandleState::Running, None))
        }

        async fn cancel(&self, _handle: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    async fn send_down(ws: &mut WebSocketStream<TcpStream>, frame: &DispatcherFrameDown) {
        ws.send(Message::Text(serde_json::to_string(frame).unwrap().into())).await.unwrap();
    }

    async fn next_up(ws: &mut WebSocketStream<TcpStream>) -> DispatcherFrameUp {
        let read = async {
            loop {
                let Message::Text(text) = ws.next().await.unwrap().unwrap() else { continue };
                let up: DispatcherFrameUp = serde_json::from_str(text.as_str()).unwrap();
                if !matches!(up, DispatcherFrameUp::Heartbeat { .. }) {
                    return up;
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), read)
            .await
            .expect("the dispatcher answers within 5s")
    }

    #[tokio::test]
    async fn a_status_frame_is_answered_while_a_dispatch_is_in_flight() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let release = Arc::new(Notify::new());
        let runner = Runner::new(
            ServerClient::new(format!("http://{addr}"), "test"),
            cctui_secrets::Secret::fixed("dispatcher-key", "dk-test"),
            HeldDispatch { release: release.clone() },
        );
        let shutdown = CancellationToken::new();
        let run = tokio::spawn(runner.run(shutdown.clone()));
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        assert!(matches!(next_up(&mut ws).await, DispatcherFrameUp::Hello { .. }));

        let dispatch_id = uuid::Uuid::new_v4();
        let spec = WireDispatchSpec {
            session_id: "held".into(),
            timeout_minutes: None,
            reply_url: None,
            dedup_key: None,
            profile: None,
            payload: serde_json::json!({}),
            traceparent: None,
        };
        send_down(&mut ws, &DispatcherFrameDown::Dispatch { request_id: dispatch_id, spec }).await;
        let status_id = uuid::Uuid::new_v4();
        let probe = DispatcherFrameDown::Status { request_id: status_id, handle: "jobs/x".into() };
        send_down(&mut ws, &probe).await;

        let first = next_up(&mut ws).await;
        let DispatcherFrameUp::StatusResult { request_id, .. } = &first else {
            panic!("the probe is answered behind a held dispatch: {first:?}");
        };
        assert_eq!(*request_id, status_id);
        release.notify_one();
        let second = next_up(&mut ws).await;
        let DispatcherFrameUp::DispatchResult { request_id, error, .. } = &second else {
            panic!("the held dispatch is answered once released: {second:?}");
        };
        assert_eq!((*request_id, error.as_deref()), (dispatch_id, None));

        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), run).await.unwrap().unwrap();
    }
}
