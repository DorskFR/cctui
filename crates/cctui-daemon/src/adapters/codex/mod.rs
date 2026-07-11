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
mod contract;
mod log_tail;
mod thread_list;

use std::path::PathBuf;

use cctui_proto::adapter::{AdapterCommand, AdapterEvent};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

use crate::adapter_runtime::{Adapter, AdapterCtx, AdapterFactory};
use crate::client::ServerClient;
use app_server::{
    AppServerConfig, CodexSession, LiveSessionRegistry, RouteAction, SessionCommand,
    SessionRegistry, route_or_prepare_resume, spawn_resumed_session,
};

/// Decide the codex launch env from a server `GatewayEnvResponse` (CCT-461),
/// mirroring the claude chokepoint's `launch_env_decision`
/// (`adapters/claude_code/control.rs`). Split out as a pure function so the
/// fail-closed contract is unit-testable without a live server.
///
/// FAIL-CLOSED INVARIANT (CCT-461 / CCT-460): when the session IS account-bound
/// but the resolved gateway env is empty, refuse the launch — a codex
/// app-server started without the gateway credential silently routes to the
/// default upstream and 401s, the exact bug this ticket fixes.
fn launch_env_decision(
    local_id: &str,
    resp: &cctui_proto::api::GatewayEnvResponse,
    hint: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    match resp {
        r if r.account_bound && r.env.is_empty() => anyhow::bail!(
            "refusing to launch codex {local_id}: session is account-bound but the \
             server returned no gateway env (account missing/unmintable) — launching \
             would route to the default upstream and 401 (CCT-461/CCT-460)"
        ),
        // Account-bound: gateway env must win for routing; merge it OVER the
        // pushed hint so user-supplied non-gateway env survives.
        r if r.account_bound => {
            let mut merged = hint.clone();
            merged.extend(r.env.iter().map(|(k, v)| (k.clone(), v.clone())));
            Ok(merged)
        }
        // Not account-bound: no gateway routing required; keep the hint.
        _ => Ok(hint.clone()),
    }
}

/// Pull the launch-time gateway env for `local_id` from the server's durable
/// `sessions.account_id` binding and merge it over the carried `hint`
/// (`spec.env`), mirroring the claude `Driver::resolve_launch_env` chokepoint.
///
/// Degrades to the pushed `hint` when no server is configured (tests / legacy)
/// or the pull is unavailable (older server / transient network) — a rollout or
/// blip falls back to the prior behavior rather than blocking the launch. The
/// fail-closed refusal (account-bound but empty env) only fires when the
/// authoritative pull SUCCEEDS and reports the binding.
async fn resolve_launch_env(
    server: Option<&ServerClient>,
    machine_key: Option<&String>,
    local_id: &str,
    hint: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    let (Some(server), Some(mk)) = (server, machine_key) else {
        return Ok(hint.clone());
    };
    match server.gateway_env(mk, local_id).await {
        Ok(resp) => launch_env_decision(local_id, &resp, hint),
        Err(e) => {
            tracing::warn!(%local_id, "codex gateway-env pull failed; falling back to pushed env: {e}");
            Ok(hint.clone())
        }
    }
}

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

    // CCT-263: poll `codex app-server`'s state-DB-backed `thread/list` for a
    // first-class inventory of EVERY machine session (cli/vscode/exec/
    // appServer) with real preview/name/cwd/status — the parity-with-claude
    // upgrade over the log-tail's heuristic JSONL scrape. Shares the
    // app-server `registry` so cctui-driven threads aren't double-emitted.
    // Falls back silently to log-tail-only when the poll can't run (codex
    // missing, sandbox/userns, auth). Disable with `inventory = false`.
    // CCT-339: before driving any commands, rediscover the codex threads cctui
    // itself owned before this daemon (re)started — a self-update / release
    // rollout restarts the daemon and drops the in-memory registry, leaving
    // in-flight `appServer`-source threads unrevivable. Seeding the durable
    // registry from `thread/list` lets the next reply/rename/set-model resume
    // them via `thread/resume`, mirroring the claude-code backfill/reconnect.
    if thread_list::ThreadListConfig::enabled(&ctx.config) {
        let cfg = thread_list::ThreadListConfig::from_value(&ctx.config);
        thread_list::rediscover_owned(&cfg, &registry).await;
    }

    let inventory_handle = if thread_list::ThreadListConfig::enabled(&ctx.config) {
        // CCT-276: the inventory's `seen` set is its own dedup state only — it
        // is no longer shared with the log-tail to suppress rollout files, so a
        // discovered CLI session still gets its real transcript backfilled.
        let seen = thread_list::SeenIds::default();
        let inv = thread_list::ThreadListInventory::new(
            thread_list::ThreadListConfig::from_value(&ctx.config),
            ctx.events.clone(),
            ctx.shutdown.clone(),
            registry.clone(),
            seen,
        );
        Some(tokio::spawn(inv.run()))
    } else {
        None
    };

    let log_handle = tokio::spawn(log.run());

    let pump = command_pump(
        ctx.commands,
        ctx.events.clone(),
        live,
        registry,
        app_cfg,
        ctx.shutdown,
        ctx.server,
        ctx.machine_key,
    );
    pump.await;
    log_handle.abort();
    if let Some(h) = inventory_handle {
        h.abort();
    }
    Ok(())
}

/// Route adapter commands. `Spawn` launches a new `codex app-server`-driven
/// session; the rest are forwarded to the owning session task by `local_id`
/// via the shared registry.
#[allow(clippy::cognitive_complexity, clippy::too_many_lines, clippy::too_many_arguments)]
async fn command_pump(
    mut commands: mpsc::Receiver<AdapterCommand>,
    events: mpsc::Sender<AdapterEvent>,
    live: LiveSessionRegistry,
    registry: SessionRegistry,
    app_cfg: AppServerConfig,
    shutdown: tokio_util::sync::CancellationToken,
    server: Option<ServerClient>,
    machine_key: Option<String>,
) {
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { return };
                match cmd {
                    // codex mints its own thread id, so the server-pre-minted
                    // `session_id` (CCT-446) is ignored here.
                    AdapterCommand::Spawn { spec, command_id, session_id } => {
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
                        // CCT-461/CCT-581: pull the launch-time gateway env from
                        // the server's durable binding, keyed by the id the
                        // server bound the gateway token to — the pre-minted
                        // session id when present, else `command_id` (codex mints
                        // its own thread id, so the server keys its token on
                        // command_id, spawn.rs). Never pull with an empty id (it
                        // would hit `/sessions//gateway-env` and never match).
                        // Merge over the carried `spec.env`. Fail-closed: an
                        // account-bound
                        // session with empty gateway env refuses to launch
                        // rather than starting env-less and 401ing (CCT-460).
                        let env = match resolve_launch_env(
                            server.as_ref(),
                            machine_key.as_ref(),
                            &session_id
                                .or(command_id)
                                .map_or_else(String::new, |id| id.to_string()),
                            &spec.env,
                        )
                        .await
                        {
                            Ok(env) => env,
                            Err(err) => {
                                tracing::error!(%err, "codex spawn: refusing env-less launch");
                                if let Some(command_id) = command_id {
                                    let _ = events
                                        .send(AdapterEvent::CommandResult {
                                            command_id,
                                            ok: false,
                                            error: Some(err.to_string()),
                                        })
                                        .await;
                                }
                                continue;
                            }
                        };
                        // The CommandResult for `command_id` is deferred to the
                        // session driver: it reports ok only after
                        // `thread/start` succeeds (CCT-631).
                        // Per-spawn permission posture (CCT-149): override the
                        // host default sandbox_mode + approval_policy. None →
                        // keep the daemon.toml defaults (which a no-userns host
                        // sets to full-access). `auto` keeps the workspace
                        // sandbox but disables approval prompts (approval=never).
                        let mut cfg = app_cfg.clone();
                        if let Some(mode) = spec.permission_mode {
                            let (sandbox, approval) = mode.codex_sandbox_approval();
                            sandbox.clone_into(&mut cfg.sandbox_mode);
                            approval.clone_into(&mut cfg.approval_policy);
                        }
                        // Per-spawn reasoning effort (codex: low/medium/high/xhigh).
                        if let Some(effort) =
                            spec.effort.as_deref().map(str::trim).filter(|e| !e.is_empty())
                        {
                            cfg.reasoning_effort = Some(effort.to_owned());
                        }
                        // Per-spawn model family (CCT-274).
                        if let Some(model) =
                            spec.model.as_deref().map(str::trim).filter(|m| !m.is_empty())
                        {
                            cfg.model = Some(model.to_owned());
                        }
                        // Stage spawn attachments (CCT-636). A staging failure is
                        // fatal to the spawn — silently dropping a file the user
                        // expects the session to read is the P0 bug this fixes.
                        // Keyed by the same id the gateway env used so the staging
                        // dir is stable across the session lifetime.
                        let stage_id = session_id
                            .or(command_id)
                            .map_or_else(String::new, |id| id.to_string());
                        let attachments = match crate::adapters::uploads::stage_bootstrap(
                            &stage_id,
                            &spec.bootstrap,
                        ) {
                            Ok(paths) => paths,
                            Err(err) => {
                                tracing::error!(%err, "codex spawn: attachment staging failed");
                                if let Some(command_id) = command_id {
                                    let _ = events
                                        .send(AdapterEvent::CommandResult {
                                            command_id,
                                            ok: false,
                                            error: Some(format!("attachment staging failed: {err}")),
                                        })
                                        .await;
                                }
                                continue;
                            }
                        };
                        let session = CodexSession::new_fresh(
                            cfg,
                            working_dir,
                            env,
                            spec.prompt.clone(),
                            spec.name.clone(),
                            attachments,
                            command_id,
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
                    AdapterCommand::Fork { parent_local_id, spec, command_id, session_id } => {
                        // Fork an existing thread into a new one seeded from its
                        // history (CCT-302). Mirrors Spawn for cfg overrides
                        // (permission/effort/model) but launches via thread/fork.
                        let working_dir = spec
                            .working_dir
                            .clone()
                            .unwrap_or_else(|| parent_local_id.clone());
                        // CCT-461: resolve gateway env keyed by the child
                        // session id the server pre-minted + bound the gateway
                        // token to (falling back to the parent thread id when
                        // absent), and fail closed on an account-bound fork with
                        // empty env — same contract as Spawn.
                        let env = match resolve_launch_env(
                            server.as_ref(),
                            machine_key.as_ref(),
                            &session_id.clone().unwrap_or_else(|| parent_local_id.clone()),
                            &spec.env,
                        )
                        .await
                        {
                            Ok(env) => env,
                            Err(err) => {
                                tracing::error!(%err, "codex fork: refusing env-less launch");
                                if let Some(command_id) = command_id {
                                    let _ = events
                                        .send(AdapterEvent::CommandResult {
                                            command_id,
                                            ok: false,
                                            error: Some(err.to_string()),
                                        })
                                        .await;
                                }
                                continue;
                            }
                        };
                        let mut cfg = app_cfg.clone();
                        if let Some(mode) = spec.permission_mode {
                            let (sandbox, approval) = mode.codex_sandbox_approval();
                            sandbox.clone_into(&mut cfg.sandbox_mode);
                            approval.clone_into(&mut cfg.approval_policy);
                        }
                        if let Some(effort) =
                            spec.effort.as_deref().map(str::trim).filter(|e| !e.is_empty())
                        {
                            cfg.reasoning_effort = Some(effort.to_owned());
                        }
                        if let Some(model) =
                            spec.model.as_deref().map(str::trim).filter(|m| !m.is_empty())
                        {
                            cfg.model = Some(model.to_owned());
                        }
                        // Stage fork attachments (CCT-636), fatal on failure — same
                        // contract as spawn.
                        let stage_id = session_id
                            .clone()
                            .unwrap_or_else(|| parent_local_id.clone());
                        let attachments = match crate::adapters::uploads::stage_bootstrap(
                            &stage_id,
                            &spec.bootstrap,
                        ) {
                            Ok(paths) => paths,
                            Err(err) => {
                                tracing::error!(%err, "codex fork: attachment staging failed");
                                if let Some(command_id) = command_id {
                                    let _ = events
                                        .send(AdapterEvent::CommandResult {
                                            command_id,
                                            ok: false,
                                            error: Some(format!("attachment staging failed: {err}")),
                                        })
                                        .await;
                                }
                                continue;
                            }
                        };
                        let session = CodexSession::new_fork(
                            cfg,
                            working_dir,
                            env,
                            parent_local_id,
                            spec.prompt.clone(),
                            spec.name.clone(),
                            attachments,
                            command_id,
                            events.clone(),
                            live.clone(),
                            registry.clone(),
                            shutdown.clone(),
                        );
                        tokio::spawn(async move {
                            if let Err(err) = session.run().await {
                                tracing::error!(%err, "codex app-server fork ended in error");
                            }
                        });
                    }
                    AdapterCommand::PermissionResponse { local_id, request_id, allow } => {
                        forward(
                            &live,
                            &registry,
                            &events,
                            &shutdown,
                            server.as_ref(),
                            machine_key.as_ref(),
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
                            server.as_ref(),
                            machine_key.as_ref(),
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
                            server.as_ref(),
                            machine_key.as_ref(),
                            &local_id,
                            SessionCommand::Kill { signal },
                        )
                        .await;
                    }
                    AdapterCommand::Interrupt { local_id, command_id } => {
                        // `turn/interrupt` only makes sense for a LIVE session
                        // (a hibernated thread has no in-flight turn to abort).
                        // When delivered, the session driver answers
                        // `command_id` from the correlated `turn/interrupt`
                        // JSON-RPC outcome (CCT-631); a non-delivery is
                        // reported as a failure here so the webui can say so.
                        let delivered = matches!(
                            route_or_prepare_resume(
                                &live,
                                &registry,
                                &local_id,
                                SessionCommand::Interrupt { command_id },
                            )
                            .await,
                            RouteAction::Delivered
                        );
                        if !delivered {
                            if let Some(command_id) = command_id {
                                let _ = events
                                    .send(AdapterEvent::CommandResult {
                                        command_id,
                                        ok: false,
                                        error: Some(
                                            "no live codex session to interrupt".to_owned(),
                                        ),
                                    })
                                    .await;
                            }
                            tracing::warn!(%local_id, "codex: interrupt for non-live session");
                        }
                    }
                    AdapterCommand::Rename { local_id, name } => {
                        forward(
                            &live,
                            &registry,
                            &events,
                            &shutdown,
                            server.as_ref(),
                            machine_key.as_ref(),
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
                            server.as_ref(),
                            machine_key.as_ref(),
                            &local_id,
                            SessionCommand::Kill { signal: None },
                        )
                        .await;
                        registry.lock().await.remove(&local_id);
                    }
                    AdapterCommand::SetModel { local_id, model, effort, command_id } => {
                        let handled = forward(
                            &live,
                            &registry,
                            &events,
                            &shutdown,
                            server.as_ref(),
                            machine_key.as_ref(),
                            &local_id,
                            SessionCommand::SetModel { model, effort, command_id },
                        )
                        .await;
                        // Delivered/resumed paths resolve `command_id` in the
                        // driver; an untracked session resolves it as failure
                        // here so the webui doesn't wait out the ack (CCT-635).
                        if !handled && let Some(command_id) = command_id {
                            let _ = events
                                .send(AdapterEvent::CommandResult {
                                    command_id,
                                    ok: false,
                                    error: Some("no codex session to change model on".to_owned()),
                                })
                                .await;
                        }
                    }
                    _ => tracing::warn!("codex: unhandled AdapterCommand variant"),
                }
            }
        }
    }
}

// Translates codex app-server frames into adapter events; complexity is the
// breadth of frame-type branches, not nesting. Splitting per frame-type would
// fragment the codex protocol mapping kept together here on purpose.
#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
async fn forward(
    live: &LiveSessionRegistry,
    registry: &SessionRegistry,
    events: &mpsc::Sender<AdapterEvent>,
    shutdown: &tokio_util::sync::CancellationToken,
    server: Option<&ServerClient>,
    machine_key: Option<&String>,
    local_id: &str,
    cmd: SessionCommand,
) -> bool {
    match route_or_prepare_resume(live, registry, local_id, cmd).await {
        RouteAction::Delivered => true,
        RouteAction::Resume { mut record, command } if command.is_resumable() => {
            tracing::info!(%local_id, ?command, "codex: resuming hibernated app-server session");
            // CCT-482: a thread REDISCOVERED from `thread/list` after a daemon
            // restart was seeded with an empty env (it was not spawned/forked in
            // this daemon lifetime), so its first resume would launch env-less
            // and 401 for an account-bound session. Re-pull the gateway env from
            // the server's durable `sessions.account_id` binding — same
            // fail-closed contract as spawn/fork — but ONLY when the stored env
            // is empty, so we don't double-pull (and regress) spawn/fork which
            // already resolved a fresh env at launch.
            if record.env.is_empty() {
                match resolve_launch_env(server, machine_key, local_id, &record.env).await {
                    Ok(env) => record.env = env,
                    Err(err) => {
                        tracing::error!(%local_id, %err, "codex resume: refusing env-less launch");
                        return false;
                    }
                }
            }
            spawn_resumed_session(
                record,
                local_id.to_owned(),
                command,
                events.clone(),
                live.clone(),
                registry.clone(),
                shutdown.clone(),
            );
            true
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
            false
        }
        RouteAction::Missing => {
            tracing::warn!(%local_id, "codex: no app-server session for command");
            false
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

#[cfg(test)]
mod gateway_env_tests {
    use cctui_proto::api::GatewayEnvResponse;

    use super::launch_env_decision;

    fn env_of(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn merges_server_env_over_hint_when_account_bound() {
        let hint = env_of(&[("KEEP", "1"), ("OPENAI_BASE_URL", "old")]);
        let resp = GatewayEnvResponse {
            account_bound: true,
            env: env_of(&[("OPENAI_BASE_URL", "gw"), ("OPENAI_API_KEY", "tok")]),
            ..Default::default()
        };
        let got = launch_env_decision("s1", &resp, &hint).unwrap();
        assert_eq!(got.get("KEEP").map(String::as_str), Some("1"));
        assert_eq!(got.get("OPENAI_BASE_URL").map(String::as_str), Some("gw"));
        assert_eq!(got.get("OPENAI_API_KEY").map(String::as_str), Some("tok"));
    }

    #[test]
    fn fails_closed_when_account_bound_but_env_empty() {
        let resp = GatewayEnvResponse {
            account_bound: true,
            env: std::collections::BTreeMap::default(),
            ..Default::default()
        };
        let err = launch_env_decision("s1", &resp, &env_of(&[("HINT", "1")])).unwrap_err();
        assert!(err.to_string().contains("account-bound"));
    }

    #[test]
    fn keeps_hint_when_not_account_bound() {
        let resp = GatewayEnvResponse {
            account_bound: false,
            env: std::collections::BTreeMap::default(),
            ..Default::default()
        };
        let hint = env_of(&[("HINT", "1")]);
        assert_eq!(launch_env_decision("s1", &resp, &hint).unwrap(), hint);
    }

    // CCT-482: a thread rediscovered from `thread/list` after a daemon restart
    // has an EMPTY stored env, so the resume path re-pulls with that empty env as
    // the hint. The decision must then seed the gateway env from the server
    // (account-bound) so the relaunch routes through the gateway instead of
    // launching env-less and 401ing.
    #[test]
    fn resume_repull_seeds_gateway_env_for_rediscovered_thread() {
        let stored_env = std::collections::BTreeMap::default();
        let resp = GatewayEnvResponse {
            account_bound: true,
            env: env_of(&[("OPENAI_BASE_URL", "gw"), ("OPENAI_API_KEY", "tok")]),
            ..Default::default()
        };
        let got = launch_env_decision("rediscovered", &resp, &stored_env).unwrap();
        assert_eq!(got.get("OPENAI_BASE_URL").map(String::as_str), Some("gw"));
        assert_eq!(got.get("OPENAI_API_KEY").map(String::as_str), Some("tok"));
    }

    // CCT-482: the resume re-pull is fail-closed too — a rediscovered
    // account-bound thread whose binding yields no env must refuse to relaunch
    // rather than cold-launch env-less.
    #[test]
    fn resume_repull_fails_closed_when_account_bound_but_env_empty() {
        let stored_env = std::collections::BTreeMap::default();
        let resp = GatewayEnvResponse {
            account_bound: true,
            env: std::collections::BTreeMap::default(),
            ..Default::default()
        };
        let err = launch_env_decision("rediscovered", &resp, &stored_env).unwrap_err();
        assert!(err.to_string().contains("account-bound"));
    }
}
