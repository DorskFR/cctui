//! Dial-out WS run loop.
//!
//! Mirrors the daemon supervisor (CCT-248 transport spec): connect out to
//! `/api/v1/dispatcher/ws`, send `Hello` + periodic `Heartbeat`, and handle
//! `Dispatch`/`Status`/`Cancel` frames by driving the local `Spawner`. Reconnect
//! backoff + half-open detection follow the daemon's pattern verbatim so a NAT'd
//! dispatcher recovers the same way.

use std::time::Duration;

use cctui_proto::ws::{DispatcherFrameDown, DispatcherFrameUp};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::cli::ContainerCli;
use crate::client::ServerClient;
use crate::spawn::Spawner;

/// Backoff schedule, capped at the last entry (daemon parity).
const BACKOFF_SECS: &[u64] = &[5, 10, 20, 60];
const PING_INTERVAL: Duration = Duration::from_secs(20);
const LIVENESS_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Runner<C: ContainerCli> {
    client: ServerClient,
    key: String,
    spawner: Spawner<C>,
}

impl<C: ContainerCli> Runner<C> {
    #[must_use]
    pub const fn new(client: ServerClient, key: String, spawner: Spawner<C>) -> Self {
        Self { client, key, spawner }
    }

    /// Connect/reconnect until `shutdown` fires.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut attempt = 0usize;
        loop {
            if shutdown.is_cancelled() {
                return;
            }
            match self.run_once(shutdown.clone()).await {
                Ok(()) => {
                    tracing::info!("dispatcher WS closed cleanly, reconnecting");
                    attempt = 0;
                }
                Err(err) => {
                    let delay = BACKOFF_SECS[attempt.min(BACKOFF_SECS.len() - 1)];
                    tracing::warn!(%err, attempt, "dispatcher connection failed; retry in {delay}s");
                    attempt = attempt.saturating_add(1);
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_secs(delay)) => {}
                        () = shutdown.cancelled() => return,
                    }
                }
            }
        }
    }

    async fn run_once(&self, shutdown: CancellationToken) -> anyhow::Result<()> {
        let url = self.client.dispatcher_ws_url(&self.key);
        tracing::info!("connecting to dispatcher WS");
        let (ws, _) = tokio_tungstenite::connect_async(&url).await?;
        let (mut sink, mut stream) = ws.split();

        let hello = DispatcherFrameUp::Hello {
            kind: "apple".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        sink.send(Message::Text(serde_json::to_string(&hello)?.into())).await?;

        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping.tick().await;
        let mut last_rx = tokio::time::Instant::now();

        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => {
                    let _ = sink.send(Message::Close(None)).await;
                    return Ok(());
                }
                msg = stream.next() => {
                    let Some(msg) = msg else { return Ok(()); };
                    let msg = msg?;
                    last_rx = tokio::time::Instant::now();
                    if let Some(frame) = parse_frame(msg)? {
                        let up = self.handle_frame(frame).await;
                        sink.send(Message::Text(serde_json::to_string(&up)?.into())).await?;
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
                    sink.send(Message::Text(serde_json::to_string(&hb)?.into())).await?;
                }
            }
        }
    }

    /// Drive a server-sent frame against the local container host, producing the
    /// reply frame. Errors are reported back in-band (never panic the loop) so
    /// the server can surface a dispatch failure instead of hanging.
    #[allow(clippy::cognitive_complexity)]
    async fn handle_frame(&self, frame: DispatcherFrameDown) -> DispatcherFrameUp {
        match frame {
            DispatcherFrameDown::Dispatch { request_id, spec } => {
                let session_id = spec.session_id.clone();
                match self.spawner.dispatch(&spec).await {
                    Ok(out) => {
                        tracing::info!(%session_id, handle = %out.handle, status = %out.status, "dispatched worker");
                        DispatcherFrameUp::DispatchResult {
                            request_id,
                            session_id,
                            handle: out.handle,
                            namespace: None,
                            status: Some(out.status),
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
                            error: Some(err.to_string()),
                        }
                    }
                }
            }
            DispatcherFrameDown::Status { request_id, handle } => {
                match self.spawner.status(&handle).await {
                    Ok((state, reason)) => DispatcherFrameUp::StatusResult {
                        request_id,
                        handle,
                        state: Some(state.as_str().to_owned()),
                        error: reason,
                    },
                    Err(err) => DispatcherFrameUp::StatusResult {
                        request_id,
                        handle,
                        state: None,
                        error: Some(err.to_string()),
                    },
                }
            }
            DispatcherFrameDown::Cancel { request_id, handle } => {
                match self.spawner.cancel(&handle).await {
                    Ok(()) => DispatcherFrameUp::CancelResult {
                        request_id,
                        handle,
                        ok: true,
                        error: None,
                    },
                    Err(err) => DispatcherFrameUp::CancelResult {
                        request_id,
                        handle,
                        ok: false,
                        error: Some(err.to_string()),
                    },
                }
            }
            _ => {
                tracing::warn!("unknown dispatcher frame; ignoring");
                DispatcherFrameUp::Heartbeat { sent_at: chrono::Utc::now() }
            }
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
    use super::{LIVENESS_TIMEOUT, PING_INTERVAL};

    #[test]
    fn liveness_timeout_allows_at_least_two_missed_pings() {
        assert!(LIVENESS_TIMEOUT >= PING_INTERVAL * 2);
    }
}
