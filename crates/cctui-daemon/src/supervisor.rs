//! Daemon supervisor.
//!
//! Owns the WS connection, the per-adapter channels, and the reconnect
//! loop. Reconnect backoff: 5s → 10s → 20s → 60s (capped). On every
//! successful (re)connect we honour the freshest `Reconcile` from the
//! server — adapters not in the new manifest are shut down; new adapters
//! are spawned.

use std::collections::HashMap;
use std::time::Duration;

use cctui_crypto::redact::{self, CompiledPatterns};
use cctui_proto::adapter::AdapterEvent;
use cctui_proto::api::DaemonAdapterConfig;
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp, SecretScrubConfig};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::adapter_runtime::AdapterFactory;
use crate::bus::build_ctx;
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
    /// A chunked transfer interrupted by a disconnect, kept across reconnects so
    /// the next connection resumes from the last acked chunk rather than byte
    /// zero (CCT-738).
    pending_transfer: std::sync::Mutex<Option<PendingTransfer>>,
}

impl Supervisor {
    #[must_use]
    pub fn new(
        client: ServerClient,
        machine_key: String,
        factories: Vec<Box<dyn AdapterFactory>>,
    ) -> Self {
        Self { client, machine_key, factories, pending_transfer: std::sync::Mutex::new(None) }
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

        let mut scrub = CompiledPatterns::disabled();

        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Discard the immediate first tick — we just connected.
        ping.tick().await;

        // Last time we heard *anything* from the server (text frame or the
        // auto-Pong to our Ping). Drives half-open detection on ping ticks.
        let mut last_rx = tokio::time::Instant::now();

        // Resume an interrupted transfer from its last acked chunk (CCT-738).
        let mut active: Option<PendingTransfer> = self.pending_transfer.lock().unwrap().take();
        if let Some(t) = active.as_mut() {
            t.rewind_to_ack();
        }

        let outcome: anyhow::Result<()> = async {
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
                            if let DaemonFrameDown::ChunkAck { transfer_id, highest_contiguous_chunk } = &frame {
                                if let Some(t) = active.as_mut()
                                    && t.id == *transfer_id {
                                        t.record_ack(*highest_contiguous_chunk);
                                        if t.is_complete() {
                                            active = None;
                                        }
                                    }
                            } else {
                                self.handle_frame(frame, &mut running, &event_tx, &frame_up_tx, &mut scrub, &shutdown).await;
                            }
                        }
                    }
                    () = std::future::ready(()), if active.as_ref().is_some_and(PendingTransfer::has_unsent) => {
                        if let Some(t) = active.as_mut() {
                            let payload = serde_json::to_string(&t.next_frame())?;
                            sink.send(Message::Text(payload.into())).await?;
                        }
                    }
                    Some(frame) = frame_up_rx.recv() => {
                        let payload = serde_json::to_string(&frame)?;
                        sink.send(Message::Text(payload.into())).await?;
                    }
                    // Pause new events while a chunked transfer is in flight so a
                    // single WS carries one large transfer at a time.
                    Some((adapter_id, event)) = event_rx.recv(), if active.is_none() => {
                        tracing::debug!(
                            %adapter_id,
                            kind = event_kind(&event),
                            local_id = event_local_id(&event),
                            "sending event",
                        );
                        // Redact secrets before the event reaches the wire / DB.
                        let event = scrub_event(event, &scrub);
                        let up = DaemonFrameUp::Event { adapter_id, event };
                        let payload = serde_json::to_string(&up)?;
                        if payload.len() > cctui_proto::chunk::CHUNK_THRESHOLD {
                            active = PendingTransfer::new(payload.into_bytes());
                        } else {
                            sink.send(Message::Text(payload.into())).await?;
                        }
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
        .await;

        // Keep an unfinished transfer for the next connection to resume.
        if let Some(t) = active
            && !t.is_complete()
        {
            *self.pending_transfer.lock().unwrap() = Some(t);
        }
        outcome
    }

    // Dispatch over every `DaemonFrameDown` variant (reconcile / spawn / command /
    // …); complexity is the breadth of the match arms, not nesting. Per-arm helpers
    // would be churn and obscure the frame-handling overview.
    #[allow(clippy::cognitive_complexity)]
    async fn handle_frame(
        &self,
        frame: DaemonFrameDown,
        running: &mut HashMap<String, AdapterRunning>,
        event_tx: &mpsc::Sender<(String, AdapterEvent)>,
        frame_up_tx: &mpsc::Sender<DaemonFrameUp>,
        scrub: &mut CompiledPatterns,
        shutdown: &CancellationToken,
    ) {
        match frame {
            DaemonFrameDown::Reconcile { adapters, secret_scrub } => {
                *scrub = compile_scrub(&secret_scrub);
                self.reconcile(adapters, running, event_tx, shutdown);
            }
            DaemonFrameDown::Command { adapter_id, command } => {
                if let Some(running) = running.get(&adapter_id) {
                    let _ = running.commands_tx.send(*command).await;
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
            DaemonFrameDown::ListDirs { request_id, path } => {
                let up = match crate::listdirs::list_dirs(&path) {
                    Ok(dirs) => {
                        DaemonFrameUp::ListDirsResult { request_id, ok: true, dirs, error: None }
                    }
                    Err(err) => DaemonFrameUp::ListDirsResult {
                        request_id,
                        ok: false,
                        dirs: Vec::new(),
                        error: Some(err.to_string()),
                    },
                };
                if frame_up_tx.send(up).await.is_err() {
                    tracing::warn!("frame_up channel closed; dropping ListDirsResult");
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

        // Allow-list roots for agent-posted image markers (CCT-566), resolved
        // once per reconcile and shared by every adapter's event pump below.
        let image_roots = crate::imagepost::default_allowed_roots();

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

        // Start adapters that should be running but aren't, and rebuild
        // adapters whose config changed since they were last (re)built —
        // without this a live mode switch (bg→sdk) is silently ignored until
        // the adapter is stopped or the daemon restarts (CCT-496).
        for (id, cfg) in want.drain() {
            if !cfg.enabled {
                continue;
            }
            if let Some(existing) = running.get(&id) {
                if existing.config == cfg.config {
                    // Unchanged — leave it running, don't churn it.
                    continue;
                }
                // Config changed: tear the old instance down and rebuild it
                // with the new config below. Cancel its token so the old
                // adapter task + event pump exit cleanly before the new
                // instance is spawned.
                if let Some(old) = running.remove(&id) {
                    old.shutdown.cancel();
                    // The old command sink is dropped here. Any command frame
                    // still queued in it (server → daemon → old adapter) but
                    // not yet consumed by the now-cancelled adapter is lost;
                    // surface that the way the unknown-adapter path does so a
                    // dropped command isn't silent.
                    let queued = old.commands_tx.max_capacity() - old.commands_tx.capacity();
                    if queued > 0 {
                        tracing::warn!(
                            adapter_id = %id,
                            queued,
                            "dropping in-flight commands for rebuilt adapter",
                        );
                    }
                    tracing::info!(adapter_id = %id, "config changed; rebuilding adapter");
                }
            }
            let Some(factory) = self.factories.iter().find(|f| f.id() == id) else {
                tracing::warn!(adapter_id = %id, "no factory compiled in for adapter");
                continue;
            };
            let token = shutdown.child_token();
            // Hand the adapter an authenticated server client + machine key so
            // its launch chokepoint can pull per-session gateway env (CCT-460).
            let (ctx, channels) = build_ctx(
                cfg.config.clone(),
                token.clone(),
                Some(self.client.clone()),
                Some(self.machine_key.clone()),
            );
            let adapter = factory.build(cfg.config.clone());
            let adapter_id_for_pump = id.clone();
            let event_tx = event_tx.clone();
            let mut events_rx = channels.events_rx;
            // Pump per-adapter events into the shared event_tx with the adapter
            // id attached. Assistant messages pass through the image-marker
            // rewrite (CCT-566) here — per-adapter task, so an upload can't stall
            // the WS loop; a non-marker message returns unchanged.
            let img_client = self.client.clone();
            let img_key = self.machine_key.clone();
            let img_roots = image_roots.clone();
            tokio::spawn(async move {
                while let Some(evt) = events_rx.recv().await {
                    let evt =
                        crate::imagepost::process_event(&img_client, &img_key, evt, &img_roots)
                            .await;
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
                    // Remember the config this instance was built from so the
                    // next reconcile can detect a change (CCT-496).
                    config: cfg.config,
                    commands_tx: channels.commands_tx,
                },
            );
            tracing::info!(adapter_id = %id, "started adapter");
        }
    }
}

/// A large serialized up-frame being sent as ordered chunks (CCT-738), with
/// enough state to resume after a disconnect: the content-hash id, the highest
/// chunk the server has acked, and the next chunk to hand this connection.
struct PendingTransfer {
    id: String,
    payload: Vec<u8>,
    total: u32,
    highest_acked: Option<u32>,
    cursor: u32,
}

impl PendingTransfer {
    /// Build a transfer for `payload`, or `None` when it fits the single-message
    /// fast path.
    fn new(payload: Vec<u8>) -> Option<Self> {
        if payload.len() <= cctui_proto::chunk::CHUNK_THRESHOLD {
            return None;
        }
        let id = cctui_proto::chunk::transfer_id(&payload);
        let total = cctui_proto::chunk::chunk_count(payload.len());
        Some(Self { id, payload, total, highest_acked: None, cursor: 0 })
    }

    fn resume_index(&self) -> u32 {
        self.highest_acked.map_or(0, |h| h.saturating_add(1))
    }

    fn rewind_to_ack(&mut self) {
        self.cursor = self.resume_index();
    }

    const fn has_unsent(&self) -> bool {
        self.cursor < self.total
    }

    fn next_frame(&mut self) -> DaemonFrameUp {
        let frame =
            cctui_proto::chunk::chunk_frame(&self.id, &self.payload, self.cursor, self.total);
        self.cursor = self.cursor.saturating_add(1);
        frame
    }

    fn record_ack(&mut self, highest_contiguous: Option<u32>) {
        if let Some(h) = highest_contiguous
            && self.highest_acked.is_none_or(|cur| h > cur)
        {
            self.highest_acked = Some(h);
        }
    }

    fn is_complete(&self) -> bool {
        self.highest_acked == self.total.checked_sub(1)
    }
}

struct AdapterRunning {
    shutdown: CancellationToken,
    /// The adapter config this instance was built from. A reconcile compares
    /// the new manifest's config against this to decide rebuild-vs-leave-alone
    /// (CCT-496).
    config: serde_json::Value,
    /// Command sink the supervisor routes server `Command` frames into.
    commands_tx: mpsc::Sender<cctui_proto::adapter::AdapterCommand>,
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
    // Staging is filesystem-only (writes to /tmp/cctui-uploads/<id>/ and returns
    // absolute paths the message text references), so it's adapter-agnostic —
    // codex reads staged file paths just like claude does (CCT-300).
    let result = if adapter_id == "claude-code" || adapter_id == "codex" {
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

/// Compile the effective scrub set from a synced [`SecretScrubConfig`]. The
/// correlation-suffix key is the daemon's `CCTUI_VAULT_KEY` if set (empty
/// otherwise, dropping the suffix — the daemon needn't hold the server key).
fn compile_scrub(cfg: &SecretScrubConfig) -> CompiledPatterns {
    let user: Vec<(String, String)> =
        cfg.patterns.iter().map(|p| (p.name.clone(), p.regex.clone())).collect();
    redact::compile(cfg.enabled, &user, &cctui_crypto::vault_key())
}

/// Redact secrets out of an event by round-tripping it through JSON and running
/// [`redact::redact_json`] over every string leaf. A no-op when scrubbing is
/// disabled; on the (structurally impossible) round-trip failure the original
/// event is passed through unchanged rather than dropped.
fn scrub_event(event: AdapterEvent, scrub: &CompiledPatterns) -> AdapterEvent {
    if scrub.is_empty() {
        return event;
    }
    let Ok(mut value) = serde_json::to_value(&event) else { return event };
    if redact::redact_json(&mut value, scrub) == 0 {
        return event;
    }
    serde_json::from_value(value).unwrap_or(event)
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
        | AdapterEvent::AskResolved { local_id }
        | AdapterEvent::PlanRequest { local_id, .. }
        | AdapterEvent::PlanResolved { local_id } => local_id,
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
        AdapterEvent::PlanRequest { .. } => "plan_request",
        AdapterEvent::PlanResolved { .. } => "plan_resolved",
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
    use std::sync::{Arc, Mutex};

    use cctui_proto::api::DaemonAdapterConfig;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use cctui_proto::chunk::{Accept, Reassembler};
    use cctui_proto::ws::DaemonFrameUp;

    use super::{
        AdapterRunning, LIVENESS_TIMEOUT, PING_INTERVAL, PendingTransfer, Supervisor,
        compile_scrub, scrub_event,
    };
    use crate::adapter_runtime::{Adapter, AdapterCtx, AdapterFactory};
    use crate::client::ServerClient;

    #[test]
    fn scrub_event_masks_tool_use_secret_before_send() {
        let cfg = cctui_proto::ws::SecretScrubConfig { enabled: true, patterns: vec![] };
        let scrub = compile_scrub(&cfg);
        let event = cctui_proto::adapter::AdapterEvent::ToolUse {
            local_id: "sess-1".to_owned(),
            payload: serde_json::json!({
                "command": "export GH_TOKEN=ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123",
            }),
        };
        let out = scrub_event(event, &scrub);
        let json = serde_json::to_string(&out).unwrap();
        assert!(!json.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123"), "secret leaked: {json}");
        assert!(json.contains("[REDACTED:github_token"), "no placeholder: {json}");
    }

    #[test]
    fn scrub_event_is_noop_when_disabled() {
        let scrub = compile_scrub(&cctui_proto::ws::SecretScrubConfig::default());
        let event = cctui_proto::adapter::AdapterEvent::Message {
            local_id: "s".to_owned(),
            payload: serde_json::json!({ "text": "ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123" }),
        };
        let out = scrub_event(event, &scrub);
        assert!(serde_json::to_string(&out).unwrap().contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123"));
    }

    /// Records the config each `build` call was handed, so a test can assert
    /// whether (and with what config) an adapter was (re)built.
    #[derive(Clone, Default)]
    struct BuildRecorder {
        builds: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    struct StubAdapter;

    #[async_trait::async_trait]
    impl Adapter for StubAdapter {
        fn id(&self) -> &'static str {
            "stub"
        }
        async fn start(&self, ctx: AdapterCtx) -> anyhow::Result<()> {
            // Idle until cancelled so the spawned task doesn't error out.
            ctx.shutdown.cancelled().await;
            Ok(())
        }
    }

    struct StubFactory {
        recorder: BuildRecorder,
    }

    impl AdapterFactory for StubFactory {
        fn id(&self) -> &'static str {
            "stub"
        }
        fn build(&self, config: serde_json::Value) -> Box<dyn Adapter> {
            self.recorder.builds.lock().unwrap().push(config);
            Box::new(StubAdapter)
        }
    }

    fn cfg(mode: &str) -> DaemonAdapterConfig {
        DaemonAdapterConfig {
            adapter_id: "stub".into(),
            config: serde_json::json!({ "mode": mode }),
            enabled: true,
        }
    }

    #[tokio::test]
    async fn config_change_rebuilds_adapter_identical_does_not_churn() {
        let recorder = BuildRecorder::default();
        let supervisor = Supervisor::new(
            ServerClient::new("http://localhost"),
            "machine-key".to_string(),
            vec![Box::new(StubFactory { recorder: recorder.clone() })],
        );
        let shutdown = CancellationToken::new();
        let (event_tx, _event_rx) = mpsc::channel(8);
        let mut running: std::collections::HashMap<String, AdapterRunning> =
            std::collections::HashMap::new();

        // First reconcile: adapter starts → one build with mode=bg.
        supervisor.reconcile(vec![cfg("bg")], &mut running, &event_tx, &shutdown);
        let first_token = running.get("stub").expect("adapter running").shutdown.clone();
        assert_eq!(recorder.builds.lock().unwrap().len(), 1);

        // Identical reconcile: no churn — still one build, same instance.
        supervisor.reconcile(vec![cfg("bg")], &mut running, &event_tx, &shutdown);
        assert_eq!(recorder.builds.lock().unwrap().len(), 1, "identical config must not rebuild");
        assert!(!first_token.is_cancelled(), "identical config must not cancel the adapter");

        // Changed reconcile (mode bg→sdk): rebuild — second build with the new
        // config, old instance cancelled, fresh token in the map.
        supervisor.reconcile(vec![cfg("sdk")], &mut running, &event_tx, &shutdown);
        let builds = recorder.builds.lock().unwrap().clone();
        assert_eq!(builds.len(), 2, "config change must rebuild the adapter");
        assert_eq!(builds[1], serde_json::json!({ "mode": "sdk" }));
        assert!(first_token.is_cancelled(), "old adapter instance must be cancelled on rebuild");
        let new_token = &running.get("stub").expect("adapter still running").shutdown;
        assert!(!new_token.is_cancelled(), "rebuilt adapter must be live");
    }

    fn chunk_parts(frame: DaemonFrameUp) -> (String, u32, u32, String) {
        match frame {
            DaemonFrameUp::Chunk { transfer_id, chunk_index, total_chunks, data } => {
                (transfer_id, chunk_index, total_chunks, data)
            }
            _ => panic!("expected a chunk frame"),
        }
    }

    #[test]
    fn small_frames_take_the_single_message_fast_path() {
        assert!(PendingTransfer::new(vec![0u8; 1024]).is_none());
        assert!(PendingTransfer::new(vec![0u8; cctui_proto::chunk::CHUNK_THRESHOLD]).is_none());
        assert!(PendingTransfer::new(vec![0u8; cctui_proto::chunk::CHUNK_THRESHOLD + 1]).is_some());
    }

    #[test]
    fn resume_completes_20mb_transfer_across_repeated_disconnects() {
        // The ticket's acceptance test: a 20MB event over a link killed every
        // few chunks must still complete by resuming from the last acked chunk.
        let payload: Vec<u8> =
            (0..20 * 1024 * 1024).map(|i| u8::try_from(i % 251).unwrap()).collect();
        let mut sender = PendingTransfer::new(payload.clone()).expect("20MB must chunk");
        let total = sender.total;
        let mut server = Reassembler::new(64 * 1024 * 1024);
        let mut completed: Option<Vec<u8>> = None;
        let mut connections = 0u32;
        let mut sends = 0u32;
        let kill_after = 5u32;
        let mut guard = 0u32;
        while completed.is_none() {
            guard += 1;
            assert!(guard < 100_000, "resume loop failed to converge");
            connections += 1;
            sender.rewind_to_ack();
            let mut sent_this_conn = 0u32;
            while sender.has_unsent() {
                let (id, idx, tot, data) = chunk_parts(sender.next_frame());
                sends += 1;
                match server.accept(&id, idx, tot, &data) {
                    Accept::Pending(highest) => sender.record_ack(highest),
                    Accept::Complete(bytes) => {
                        completed = Some(bytes);
                        break;
                    }
                    Accept::Restart => sender.record_ack(None),
                }
                sent_this_conn += 1;
                if sent_this_conn >= kill_after {
                    break;
                }
            }
        }
        assert_eq!(completed.unwrap(), payload, "resumed transfer must be byte-exact");
        assert!(connections > 1, "completing must have spanned multiple connections");
        // Resuming past the acked prefix means no chunk is ever re-sent.
        assert_eq!(sends, total, "resume must not re-upload already-acked chunks");
    }

    #[test]
    fn record_ack_is_monotonic_and_marks_completion() {
        let mut t =
            PendingTransfer::new(vec![7u8; cctui_proto::chunk::CHUNK_SIZE * 3 + 1]).unwrap();
        assert_eq!(t.total, 4);
        t.record_ack(Some(2));
        // A stale lower ack must not rewind progress.
        t.record_ack(Some(1));
        assert_eq!(t.resume_index(), 3);
        assert!(!t.is_complete());
        t.record_ack(Some(3));
        assert!(t.is_complete());
    }

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
