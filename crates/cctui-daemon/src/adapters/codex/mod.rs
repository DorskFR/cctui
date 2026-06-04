//! Codex adapter.
//!
//! Two modes, picked at start by config or env:
//!
//! - **Log-tail (default, CCT-89)** — watches `~/.codex/sessions/` for
//!   new log files, emits `SessionStarted`/`Message`/`ToolUse`/
//!   `SessionEnded` based on file activity and a configurable quiesce
//!   window. Sessions root and timing knobs are tunable via the
//!   `adapters_enabled.config` JSON.
//! - **UDS injection (legacy v0)** — listens on
//!   `$CCTUI_CODEX_SOCK` (or `$XDG_RUNTIME_DIR/cctui-codex.sock`) and
//!   forwards line-delimited `AdapterEvent` JSON. Kept for tests and
//!   for tools that want to push events directly. Enable with
//!   `config.mode = "uds"`.
//!
//! Same shape as the claude-code adapter: listens on a dedicated Unix
//! domain socket and forwards line-delimited [`AdapterEvent`] JSON to the
//! daemon. Proves the `Adapter` trait holds for a second harness.
//!
//! Full Codex CLI integration (log-tail of `~/.codex/sessions/` via the
//! `notify` crate, end-of-session quiescence detection, payload parsing)
//! is intentionally deferred to a follow-up PR.
//!
//! Defaults to **disabled** in `adapters_enabled` — users opt in by
//! flipping the row or via a future web toggle.
//!
//! Socket path: `$CCTUI_CODEX_SOCK`, defaulting to
//! `$XDG_RUNTIME_DIR/cctui-codex.sock`.

mod app_server;
mod log_tail;

use std::path::PathBuf;

use cctui_proto::adapter::{AdapterCommand, AdapterEvent};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

use crate::adapter_runtime::{Adapter, AdapterCtx, AdapterFactory};
use app_server::{
    AppServerConfig, CodexSession, LiveSessionRegistry, RouteAction, SessionCommand,
    SessionRegistry, route_or_prepare_resume, spawn_resumed_session,
};

fn uses_uds_mode(config: &serde_json::Value) -> bool {
    config.get("mode").and_then(|v| v.as_str()) == Some("uds")
}

pub struct CodexAdapter;

#[async_trait::async_trait]
impl Adapter for CodexAdapter {
    fn id(&self) -> &'static str {
        "codex"
    }

    async fn start(&self, ctx: AdapterCtx) -> anyhow::Result<()> {
        if !uses_uds_mode(&ctx.config) {
            return run_default(ctx).await;
        }
        let path = resolve_socket_path(&ctx.config);
        let _ = std::fs::remove_file(&path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let listener = UnixListener::bind(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&path, perms);
        }
        tracing::info!(socket = %path.display(), "codex adapter listening");

        loop {
            tokio::select! {
                () = ctx.shutdown.cancelled() => {
                    let _ = std::fs::remove_file(&path);
                    return Ok(());
                }
                accept = listener.accept() => {
                    let (stream, _) = accept?;
                    let events = ctx.events.clone();
                    tokio::spawn(async move {
                        if let Err(err) = handle_connection(stream, events).await {
                            tracing::warn!(%err, "codex uds connection error");
                        }
                    });
                }
            }
        }
    }
}

/// Default mode (CCT-89 + CCT-98): the passive log-tail observes sessions
/// started outside cctui, while the app-server command pump drives sessions
/// that cctui spawns. They share a [`SessionRegistry`] so the log-tail skips
/// rollout files an app-server session already owns (no double-ingest).
async fn run_default(ctx: AdapterCtx) -> anyhow::Result<()> {
    let app_cfg = AppServerConfig::from_value(&ctx.config);
    let registry: SessionRegistry = SessionRegistry::default();
    let live: LiveSessionRegistry = LiveSessionRegistry::default();

    let mut log = log_tail::LogTail::new(
        log_tail::LogTailConfig::from_value(&ctx.config),
        ctx.events.clone(),
        ctx.shutdown.clone(),
    );
    log.set_owned(registry.clone());
    let log_handle = tokio::spawn(log.run());

    let pump =
        command_pump(ctx.commands, ctx.events.clone(), live, registry, app_cfg, ctx.shutdown);
    pump.await;
    log_handle.abort();
    Ok(())
}

/// Route adapter commands. `Spawn` launches a new `codex app-server`-driven
/// session; the rest are forwarded to the owning session task by `local_id`
/// via the shared registry.
#[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
async fn command_pump(
    mut commands: mpsc::Receiver<AdapterCommand>,
    events: mpsc::Sender<AdapterEvent>,
    live: LiveSessionRegistry,
    registry: SessionRegistry,
    app_cfg: AppServerConfig,
    shutdown: tokio_util::sync::CancellationToken,
) {
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { return };
                match cmd {
                    AdapterCommand::Spawn { spec, command_id } => {
                        let Some(working_dir) = spec.working_dir.clone() else {
                            tracing::error!("codex spawn: working_dir required");
                            if let Some(command_id) = command_id {
                                let _ = events
                                    .send(AdapterEvent::CommandResult {
                                        command_id,
                                        ok: false,
                                        error: Some("working_dir required".to_owned()),
                                    })
                                    .await;
                            }
                            continue;
                        };
                        if let Some(command_id) = command_id {
                            // The codex session launches asynchronously; report
                            // that dispatch was accepted. Runtime failures
                            // surface as session events (CCT-131).
                            let _ = events
                                .send(AdapterEvent::CommandResult {
                                    command_id,
                                    ok: true,
                                    error: None,
                                })
                                .await;
                        }
                        // Per-spawn permission posture (CCT-149): override the
                        // host default sandbox_mode + approval_policy. None →
                        // keep the daemon.toml defaults (which a no-userns host
                        // sets to full-access). `auto` keeps the workspace
                        // sandbox but disables approval prompts (approval=never).
                        let mut cfg = app_cfg.clone();
                        if let Some(mode) = spec.permission_mode {
                            use cctui_proto::adapter::PermissionMode;
                            let (sandbox, approval) = match mode {
                                PermissionMode::Yolo => ("danger-full-access", "never"),
                                PermissionMode::Auto => ("workspace-write", "never"),
                                PermissionMode::Ask => ("workspace-write", "untrusted"),
                            };
                            sandbox.clone_into(&mut cfg.sandbox_mode);
                            approval.clone_into(&mut cfg.approval_policy);
                        }
                        // Per-spawn reasoning effort (codex: minimal/low/medium/high).
                        if let Some(effort) =
                            spec.effort.as_deref().map(str::trim).filter(|e| !e.is_empty())
                        {
                            cfg.reasoning_effort = Some(effort.to_owned());
                        }
                        let session = CodexSession::new_fresh(
                            cfg,
                            working_dir,
                            spec.prompt.clone(),
                            spec.name.clone(),
                            events.clone(),
                            live.clone(),
                            registry.clone(),
                            shutdown.clone(),
                        );
                        tokio::spawn(async move {
                            if let Err(err) = session.run().await {
                                tracing::error!(%err, "codex app-server session ended in error");
                            }
                        });
                    }
                    AdapterCommand::PermissionResponse { local_id, request_id, allow } => {
                        forward(
                            &live,
                            &registry,
                            &events,
                            &shutdown,
                            &local_id,
                            SessionCommand::Permission { request_id, allow },
                        )
                            .await;
                    }
                    AdapterCommand::SendMessage { local_id, text }
                    | AdapterCommand::Reply { local_id, text, .. } => {
                        forward(
                            &live,
                            &registry,
                            &events,
                            &shutdown,
                            &local_id,
                            SessionCommand::Send { text },
                        )
                        .await;
                    }
                    AdapterCommand::Kill { local_id, signal } => {
                        forward(
                            &live,
                            &registry,
                            &events,
                            &shutdown,
                            &local_id,
                            SessionCommand::Kill { signal },
                        )
                        .await;
                    }
                    AdapterCommand::Interrupt { local_id } => {
                        forward(
                            &live,
                            &registry,
                            &events,
                            &shutdown,
                            &local_id,
                            SessionCommand::Interrupt,
                        )
                        .await;
                    }
                    AdapterCommand::Rename { local_id, name } => {
                        forward(
                            &live,
                            &registry,
                            &events,
                            &shutdown,
                            &local_id,
                            SessionCommand::Rename { name },
                        )
                        .await;
                    }
                    AdapterCommand::Remove { local_id } => {
                        // Codex sessions have no external agent-view (no
                        // ~/.claude/jobs entry) to purge, so removal is just
                        // terminating the worker; cctui's own archived state
                        // hides it from the list. The app-server session
                        // already cleans up its temp transcript on exit.
                        forward(
                            &live,
                            &registry,
                            &events,
                            &shutdown,
                            &local_id,
                            SessionCommand::Kill { signal: None },
                        )
                        .await;
                        registry.lock().await.remove(&local_id);
                    }
                    _ => tracing::warn!("codex: unhandled AdapterCommand variant"),
                }
            }
        }
    }
}

async fn forward(
    live: &LiveSessionRegistry,
    registry: &SessionRegistry,
    events: &mpsc::Sender<AdapterEvent>,
    shutdown: &tokio_util::sync::CancellationToken,
    local_id: &str,
    cmd: SessionCommand,
) {
    match route_or_prepare_resume(live, registry, local_id, cmd).await {
        RouteAction::Delivered => {}
        RouteAction::Resume { record, command } if command.is_resumable() => {
            tracing::info!(%local_id, ?command, "codex: resuming hibernated app-server session");
            spawn_resumed_session(
                record,
                local_id.to_owned(),
                command,
                events.clone(),
                live.clone(),
                registry.clone(),
                shutdown.clone(),
            );
        }
        RouteAction::Resume { command, .. } => {
            tracing::warn!(%local_id, ?command, "codex: command cannot be applied to hibernated session");
            if matches!(command, SessionCommand::Kill { .. }) {
                registry.lock().await.remove(local_id);
                let _ = events
                    .send(AdapterEvent::SessionEnded {
                        local_id: local_id.to_owned(),
                        reason: cctui_proto::adapter::EndReason::Killed,
                    })
                    .await;
            }
        }
        RouteAction::Missing => {
            tracing::warn!(%local_id, "codex: no app-server session for command");
        }
    }
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    events: tokio::sync::mpsc::Sender<AdapterEvent>,
) -> anyhow::Result<()> {
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<AdapterEvent>(line) {
            Ok(evt) => {
                if events.send(evt).await.is_err() {
                    break;
                }
            }
            Err(err) => {
                tracing::warn!(%err, ?line, "ignoring non-AdapterEvent uds line");
            }
        }
    }
    Ok(())
}

fn resolve_socket_path(config: &serde_json::Value) -> PathBuf {
    if let Some(p) = config.get("socket_path").and_then(|v| v.as_str()) {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("CCTUI_CODEX_SOCK") {
        return PathBuf::from(p);
    }
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(base).join("cctui-codex.sock")
}

pub struct CodexFactory;

impl AdapterFactory for CodexFactory {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn build(&self, _config: serde_json::Value) -> Box<dyn Adapter> {
        Box::new(CodexAdapter)
    }
}
