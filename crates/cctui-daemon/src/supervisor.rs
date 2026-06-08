//! Daemon supervisor.
//!
//! Owns the WS connection, the per-adapter channels, and the reconnect
//! loop. Reconnect backoff: 5s → 10s → 20s → 60s (capped). On every
//! successful (re)connect we honour the freshest `Reconcile` from the
//! server — adapters not in the new manifest are shut down; new adapters
//! are spawned.

use std::collections::HashMap;
use std::time::Duration;

use cctui_proto::adapter::AdapterEvent;
use cctui_proto::api::DaemonAdapterConfig;
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::adapter_runtime::AdapterFactory;
use crate::bus::{AdapterChannels, build_ctx};
use crate::client::ServerClient;

/// Backoff schedule. Capped at the last entry on subsequent failures.
const BACKOFF_SECS: &[u64] = &[5, 10, 20, 60];

/// WS keepalive ping interval. Must be shorter than any idle timeout on
/// the path (ingress, NAT, load balancer). 20s is comfortably below the
/// typical 60s defaults.
const PING_INTERVAL: Duration = Duration::from_secs(20);

/// If no frame (incl. the server's auto-Pong to our Ping) arrives within
/// this window, the connection is treated as half-open and torn down so the
/// reconnect loop re-establishes it. ~3 missed pings. Without this the
/// daemon can sit forever on a dead TCP socket (`sink.send` buffers into the
/// kernel without erroring, `stream.next` blocks) and the web UI reports
/// "daemon offline" until a manual restart (CCT-140).
const LIVENESS_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Supervisor {
    client: ServerClient,
    machine_key: String,
    factories: Vec<Box<dyn AdapterFactory>>,
}

impl Supervisor {
    #[must_use]
    pub fn new(
        client: ServerClient,
        machine_key: String,
        factories: Vec<Box<dyn AdapterFactory>>,
    ) -> Self {
        Self { client, machine_key, factories }
    }

    /// Run the connect/reconnect loop until `shutdown` fires.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut attempt = 0usize;
        loop {
            if shutdown.is_cancelled() {
                return;
            }
            match self.run_once(shutdown.clone()).await {
                Ok(()) => {
                    tracing::info!("daemon WS closed cleanly, reconnecting");
                    attempt = 0;
                }
                Err(err) => {
                    let delay = BACKOFF_SECS[attempt.min(BACKOFF_SECS.len() - 1)];
                    tracing::warn!(%err, attempt, "daemon connection failed; retry in {delay}s");
                    attempt = attempt.saturating_add(1);
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_secs(delay)) => {}
                        () = shutdown.cancelled() => return,
                    }
                }
            }
        }
    }

    #[allow(clippy::cognitive_complexity)]
    async fn run_once(&self, shutdown: CancellationToken) -> anyhow::Result<()> {
        let url = self.client.daemon_ws_url(&self.machine_key);
        tracing::info!(%url, "connecting to daemon WS");
        let (ws, _) = tokio_tungstenite::connect_async(&url).await?;
        let (mut sink, mut stream) = ws.split();

        // Events from all running adapters fan into this single channel
        // and from there onto the WS.
        let (event_tx, mut event_rx) = mpsc::channel::<(String, AdapterEvent)>(256);

        // Out-of-band frames the supervisor itself produces (currently the
        // `StageFilesResult` reply to a mid-chat attachment request, CCT-236),
        // fanned onto the same WS sink as adapter events.
        let (frame_up_tx, mut frame_up_rx) = mpsc::channel::<DaemonFrameUp>(64);

        // Per-adapter command sinks (so `Command` frames from the server can
        // be routed to the right adapter by `adapter_id`).
        let mut running: HashMap<String, AdapterRunning> = HashMap::new();

        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Discard the immediate first tick — we just connected.
        ping.tick().await;

        // Last time we heard *anything* from the server (text frame or the
        // auto-Pong to our Ping). Drives half-open detection on ping ticks.
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
                        self.handle_frame(frame, &mut running, &event_tx, &frame_up_tx, &shutdown).await;
                    }
                }
                Some(frame) = frame_up_rx.recv() => {
                    let payload = serde_json::to_string(&frame)?;
                    sink.send(Message::Text(payload.into())).await?;
                }
                Some((adapter_id, event)) = event_rx.recv() => {
                    tracing::debug!(
                        %adapter_id,
                        kind = event_kind(&event),
                        local_id = event_local_id(&event),
                        "sending event",
                    );
                    let up = DaemonFrameUp::Event { adapter_id, event };
                    let payload = serde_json::to_string(&up)?;
                    sink.send(Message::Text(payload.into())).await?;
                }
                _ = ping.tick() => {
                    // Detect a half-open connection: if the server hasn't sent
                    // anything (not even a Pong) within LIVENESS_TIMEOUT, tear
                    // down so the reconnect loop takes over (CCT-140).
                    if last_rx.elapsed() > LIVENESS_TIMEOUT {
                        anyhow::bail!(
                            "no server traffic for {}s — WS half-open, reconnecting",
                            last_rx.elapsed().as_secs()
                        );
                    }
                    sink.send(Message::Ping(Vec::new().into())).await?;
                    // App-level liveness heartbeat (CCT-255). The WS Ping above
                    // keeps the socket warm, but the server only advances
                    // `machines.last_seen_at` on an application frame; this
                    // Heartbeat gives it a per-cadence signal to derive the
                    // machine online/stale/offline tier from.
                    let hb = DaemonFrameUp::Heartbeat { sent_at: chrono::Utc::now() };
                    let payload = serde_json::to_string(&hb)?;
                    sink.send(Message::Text(payload.into())).await?;
                }
            }
        }
    }

    async fn handle_frame(
        &self,
        frame: DaemonFrameDown,
        running: &mut HashMap<String, AdapterRunning>,
        event_tx: &mpsc::Sender<(String, AdapterEvent)>,
        frame_up_tx: &mpsc::Sender<DaemonFrameUp>,
        shutdown: &CancellationToken,
    ) {
        match frame {
            DaemonFrameDown::Reconcile { adapters } => {
                self.reconcile(adapters, running, event_tx, shutdown);
            }
            DaemonFrameDown::Command { adapter_id, command } => {
                if let Some(running) = running.get(&adapter_id) {
                    let _ = running.channels.commands_tx.send(*command).await;
                } else {
                    tracing::warn!(%adapter_id, "command for unknown adapter; dropping");
                }
            }
            DaemonFrameDown::StageFiles { request_id, adapter_id, local_id, uploads } => {
                let up = stage_files_result(request_id, &adapter_id, &local_id, &uploads);
                if frame_up_tx.send(up).await.is_err() {
                    tracing::warn!("frame_up channel closed; dropping StageFilesResult");
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::cognitive_complexity)]
    fn reconcile(
        &self,
        adapters: Vec<DaemonAdapterConfig>,
        running: &mut HashMap<String, AdapterRunning>,
        event_tx: &mpsc::Sender<(String, AdapterEvent)>,
        shutdown: &CancellationToken,
    ) {
        let mut want: HashMap<String, DaemonAdapterConfig> =
            adapters.into_iter().map(|a| (a.adapter_id.to_string(), a)).collect();

        // Stop adapters no longer in the manifest or disabled.
        let to_stop: Vec<String> = running
            .keys()
            .filter(|id| want.get(*id).is_none_or(|cfg| !cfg.enabled))
            .cloned()
            .collect();
        for id in to_stop {
            if let Some(r) = running.remove(&id) {
                r.shutdown.cancel();
                tracing::info!(adapter_id = %id, "stopped adapter");
            }
        }

        // Start adapters that should be running but aren't.
        for (id, cfg) in want.drain() {
            if !cfg.enabled || running.contains_key(&id) {
                continue;
            }
            let Some(factory) = self.factories.iter().find(|f| f.id() == id) else {
                tracing::warn!(adapter_id = %id, "no factory compiled in for adapter");
                continue;
            };
            let token = shutdown.child_token();
            let (ctx, channels) = build_ctx(cfg.config.clone(), token.clone());
            let adapter = factory.build(cfg.config);
            let adapter_id_for_pump = id.clone();
            let event_tx = event_tx.clone();
            let mut events_rx = channels.events_rx;
            // Pump per-adapter events into the shared event_tx with the
            // adapter id attached.
            tokio::spawn(async move {
                while let Some(evt) = events_rx.recv().await {
                    if event_tx.send((adapter_id_for_pump.clone(), evt)).await.is_err() {
                        break;
                    }
                }
            });
            let id_for_task = id.clone();
            tokio::spawn(async move {
                if let Err(err) = adapter.start(ctx).await {
                    tracing::error!(adapter_id = %id_for_task, %err, "adapter exited with error");
                }
            });
            running.insert(
                id.clone(),
                AdapterRunning {
                    shutdown: token,
                    channels: AdapterChannels {
                        // events_rx already moved into pump task above; we
                        // only need the commands_tx going forward.
                        events_rx: mpsc::channel(1).1,
                        commands_tx: channels.commands_tx,
                    },
                },
            );
            tracing::info!(adapter_id = %id, "started adapter");
        }
    }
}

struct AdapterRunning {
    shutdown: CancellationToken,
    channels: AdapterChannels,
}

/// Stage mid-chat attachments (CCT-236) and build the `StageFilesResult` reply.
/// Filesystem-only and reuses the spawn-time staging dir, so it doesn't need the
/// running adapter beyond confirming the adapter is supported.
fn stage_files_result(
    request_id: uuid::Uuid,
    adapter_id: &str,
    local_id: &str,
    uploads: &[cctui_proto::adapter::BootstrapFile],
) -> DaemonFrameUp {
    let result = if adapter_id == "claude-code" {
        crate::adapters::claude_code::stage_mid_chat_files(local_id, uploads)
    } else {
        Err(anyhow::anyhow!("adapter {adapter_id} does not support mid-chat file staging"))
    };
    match result {
        Ok(paths) => {
            tracing::info!(%local_id, count = paths.len(), "staged mid-chat files");
            DaemonFrameUp::StageFilesResult { request_id, ok: true, paths, error: None }
        }
        Err(err) => {
            tracing::warn!(%local_id, %err, "mid-chat file staging failed");
            DaemonFrameUp::StageFilesResult {
                request_id,
                ok: false,
                paths: Vec::new(),
                error: Some(err.to_string()),
            }
        }
    }
}

fn event_local_id(event: &AdapterEvent) -> &str {
    match event {
        AdapterEvent::SessionStarted { local_id, .. }
        | AdapterEvent::Message { local_id, .. }
        | AdapterEvent::ToolUse { local_id, .. }
        | AdapterEvent::SessionEnded { local_id, .. }
        | AdapterEvent::Status { local_id, .. }
        | AdapterEvent::PermissionRequest { local_id, .. }
        | AdapterEvent::AskQuestion { local_id, .. }
        | AdapterEvent::AskResolved { local_id } => local_id,
        _ => "",
    }
}

const fn event_kind(event: &AdapterEvent) -> &'static str {
    match event {
        AdapterEvent::SessionStarted { .. } => "session_started",
        AdapterEvent::Message { .. } => "message",
        AdapterEvent::ToolUse { .. } => "tool_use",
        AdapterEvent::SessionEnded { .. } => "session_ended",
        AdapterEvent::Status { .. } => "status",
        AdapterEvent::AskQuestion { .. } => "ask_question",
        AdapterEvent::AskResolved { .. } => "ask_resolved",
        _ => "other",
    }
}

fn parse_frame(msg: Message) -> anyhow::Result<Option<DaemonFrameDown>> {
    let txt = match msg {
        Message::Text(t) => t.to_string(),
        Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
        // Close + ping/pong/etc are not application frames.
        _ => return Ok(None),
    };
    Ok(Some(serde_json::from_str(&txt)?))
}

#[cfg(test)]
mod tests {
    use super::{LIVENESS_TIMEOUT, PING_INTERVAL};

    #[test]
    fn liveness_timeout_allows_at_least_two_missed_pings() {
        // The daemon pings every PING_INTERVAL and the server auto-pongs, so
        // last_rx refreshes each interval. The liveness window must span
        // several pings or a single dropped pong would trigger a needless
        // reconnect (CCT-140).
        assert!(
            LIVENESS_TIMEOUT >= PING_INTERVAL * 2,
            "LIVENESS_TIMEOUT must tolerate >=2 missed pings to avoid flapping"
        );
    }
}
