//! Oneshot stream-json driver for the claude-code adapter.
//!
//! Every turn is a fresh `claude -p <prompt> --output-format stream-json
//! --verbose` child: Spawn pre-mints `--session-id`, Reply/Resume use
//! `--resume <id>`, Fork adds `--fork-session`. The terminal `result` frame
//! emits an idle `Status`, not `SessionEnded`, so the conversation stays
//! resumable. Kill and Interrupt both terminate the in-flight child.
//! Permissions, Ask and Plan flow through the shared `--settings` hook
//! listener; `SetModel` is unsupported in place.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::Context as _;
use cctui_proto::adapter::{AdapterCommand, AdapterEvent, EndReason, SessionMeta, SessionSpec};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::control::DriverConfig;
use super::launch::LaunchArgs;
use super::{PendingAsks, PendingPermHooks, SessionMap, streamjson};
use crate::adapter_runtime::{
    AdapterCtx, CommandOutcome, Handled, SessionDriver, run_command_loop,
};

/// A single-shot `claude -p` driver.
///
/// Unlike the `bg` driver (which talks to a long-lived `claude daemon` control
/// socket), this driver owns each child process directly: one transient `claude
/// -p` per turn. It keeps no roster poll — session liveness is "is a child
/// currently running for this id".
pub(super) struct OneshotDriver {
    cfg: DriverConfig,
    events: mpsc::Sender<AdapterEvent>,
    shutdown: CancellationToken,
    server: Option<crate::client::ServerClient>,
    machine_key: Option<String>,
    /// `session_id → live child` for sessions with a turn in flight, so
    /// `Kill`/`Interrupt` can terminate the running `-p` process.
    running: HashMap<String, Child>,
    /// Daemon-side names (oneshot has no PTY/state.json to persist into).
    names: HashMap<String, String>,
    /// Working dir per session, captured at spawn so a later `Resume`/`Reply`
    /// re-invokes in the right place even without an on-disk job state.
    cwds: HashMap<String, String>,
    /// Hook posture captured at spawn: every turn regenerates its
    /// `--settings` file from it.
    hook_postures: HashMap<String, super::control::HookPosture>,
    /// Shared maps for the ask/permission hook listener. The listener resolves
    /// the live `session_id` a hook reports through [`SessionMap`]; oneshot keys
    /// on the full session id so we register an identity mapping per session.
    session_map: SessionMap,
    pending_asks: PendingAsks,
    pending_perm_hooks: PendingPermHooks,
}

fn short_of(session_id: &str) -> &str {
    &session_id[..8.min(session_id.len())]
}

impl OneshotDriver {
    pub(super) fn new(ctx: AdapterCtx) -> (Self, mpsc::Receiver<AdapterCommand>) {
        let cfg = DriverConfig::from_value(&ctx.config);
        let driver = Self {
            cfg,
            events: ctx.events,
            shutdown: ctx.shutdown,
            server: ctx.server,
            machine_key: ctx.machine_key,
            running: HashMap::new(),
            names: HashMap::new(),
            cwds: HashMap::new(),
            hook_postures: HashMap::new(),
            session_map: Arc::default(),
            pending_asks: Arc::default(),
            pending_perm_hooks: Arc::default(),
        };
        (driver, ctx.commands)
    }

    pub(super) async fn run(
        mut self,
        mut commands: mpsc::Receiver<AdapterCommand>,
    ) -> anyhow::Result<()> {
        tracing::info!("claude-code adapter starting in oneshot mode");
        // The same ask/permission hook listener the bg driver uses: headless
        // `-p` runs fire PreToolUse/AskUserQuestion hooks, so
        // bind the local socket the injected `--settings` file targets and route
        // deliveries through the shared maps. Spawned as a sibling task; it
        // exits on the shared shutdown token.
        self.spawn_hook_listener()?;

        let (events, shutdown) = (self.events.clone(), self.shutdown.clone());
        run_command_loop(&mut self, &mut commands, &events, &shutdown).await;
        shutdown.cancelled().await;
        self.kill_all().await;
        Ok(())
    }

    /// Bind the shared ask/permission hook socket and route deliveries through
    /// the same handler the bg driver uses.
    fn spawn_hook_listener(&self) -> anyhow::Result<()> {
        let sock = &self.cfg.hook_socket_path;
        let listener = crate::runtime::bind_private_socket(sock).inspect_err(
            |err| tracing::error!(%err, "claude-code oneshot ask-hook socket unavailable"),
        )?;
        let events = self.events.clone();
        let shutdown = self.shutdown.clone();
        let session_map = self.session_map.clone();
        let pending_asks = self.pending_asks.clone();
        let pending_perm_hooks = self.pending_perm_hooks.clone();
        tokio::spawn(async move {
            if let Err(err) = super::run_hook_listener(
                listener,
                events,
                shutdown,
                session_map,
                pending_asks,
                pending_perm_hooks,
                // These drivers don't serve the diagnose aggregation
                // (is bg-only); a fresh log satisfies the listener.
                super::HookLog::default(),
            )
            .await
            {
                tracing::warn!(%err, "claude-code oneshot ask-hook listener exited");
            }
        });
        Ok(())
    }

    /// Spawn a brand-new (or forked) conversation as a first `-p` turn.
    async fn spawn(
        &mut self,
        spec: &SessionSpec,
        session_id: String,
        fork_parent: Option<String>,
    ) -> anyhow::Result<()> {
        let cwd = spec
            .working_dir
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("spawn: working_dir required"))?
            .to_owned();
        if !std::path::Path::new(&cwd).is_dir() {
            anyhow::bail!("spawn: working_dir does not exist or is not a directory: {cwd}");
        }

        // Resolve gateway env + per-account settings (539/540) BEFORE
        // writing the hook-settings file, so the account settings can be
        // deep-merged under the managed hooks. Fail-closed inside
        // `resolve_launch_env`.
        let launch_env = self.resolve_launch_env(&session_id, &spec.env).await?;

        let posture = super::control::HookPosture::of(spec);
        let settings = posture.write(
            &self.cfg.hook_socket_path,
            short_of(&session_id),
            &launch_env,
            self.machine_key.as_deref(),
        )?;
        self.hook_postures.insert(session_id.clone(), posture);

        let mut launch = LaunchArgs::from_spec(spec, Some(settings));
        launch.session_id = Some(session_id.clone());
        launch.plugin_dirs.clone_from(&launch_env.plugin_dirs);
        if let Some(parent) = &fork_parent {
            launch.resume_from = Some(parent.clone());
            launch.fork = true;
        }

        // Register the session id in the hook map (identity) so a hook reporting
        // this live session id resolves to the same local_id we key on, and pin
        // the cwd for later resume/reply turns.
        self.register(&session_id, &cwd);

        // Announce the session before its first events so the server has a row.
        self.emit(AdapterEvent::SessionStarted {
            local_id: session_id.clone(),
            meta: started_meta(spec, &session_id, &cwd, fork_parent),
        })
        .await;

        let env = launch_env.env;
        let prompt = spec.prompt.clone().unwrap_or_default();
        self.launch_turn(&session_id, &cwd, launch.to_argv(), &prompt, &env).await
    }

    /// Continue a conversation with another `-p` turn (`--resume <id>`).
    async fn reply(
        &mut self,
        local_id: &str,
        text: &str,
        env_hint: std::collections::BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        let cwd = self
            .cwds
            .get(local_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("reply: unknown session {local_id}"))?;
        let launch_env = self.resolve_launch_env(local_id, &env_hint).await?;
        let launch = LaunchArgs {
            resume_from: Some(local_id.to_owned()),
            settings_path: self.turn_settings(local_id, &launch_env)?,
            plugin_dirs: launch_env.plugin_dirs,
            ..LaunchArgs::default()
        };
        self.launch_turn(local_id, &cwd, launch.to_argv(), text, &launch_env.env).await
    }

    /// Revive an exited-but-resumable conversation without a reply: a no-op
    /// `--resume` turn with an empty prompt.
    async fn resume(&mut self, local_id: &str, working_dir: Option<String>) -> anyhow::Result<()> {
        let cwd = working_dir
            .or_else(|| self.cwds.get(local_id).cloned())
            .ok_or_else(|| anyhow::anyhow!("resume: no working_dir for {local_id}"))?;
        self.register(local_id, &cwd);
        let launch_env =
            self.resolve_launch_env(local_id, &std::collections::BTreeMap::new()).await?;
        let launch = LaunchArgs {
            resume_from: Some(local_id.to_owned()),
            settings_path: self.turn_settings(local_id, &launch_env)?,
            plugin_dirs: launch_env.plugin_dirs,
            ..LaunchArgs::default()
        };
        self.launch_turn(local_id, &cwd, launch.to_argv(), "", &launch_env.env).await
    }

    /// Regenerate `local_id`'s `--settings` file for its next turn. A session
    /// launched before a daemon restart gets its posture back from the kept,
    /// sealed policy; a tampered one refuses the turn.
    fn turn_settings(
        &mut self,
        local_id: &str,
        launch_env: &super::control::LaunchEnv,
    ) -> anyhow::Result<Option<String>> {
        let short = short_of(local_id);
        let seal_key = self.machine_key.as_deref();
        let posture = match self.hook_postures.entry(local_id.to_owned()) {
            Entry::Occupied(known) => known.into_mut(),
            Entry::Vacant(slot) => {
                slot.insert(super::control::HookPosture::recover(short, seal_key).with_context(
                    || format!("refusing a turn for {local_id} without its hook policy"),
                )?)
            }
        };
        Ok(Some(posture.write(&self.cfg.hook_socket_path, short, launch_env, seal_key)?))
    }

    /// Spawn one `claude -p` child, stream its stdout through the shared codec,
    /// forward events, and on the terminal frame emit an idle `Status` so the
    /// session stays resumable. Blocks until the turn completes (or shutdown).
    // Linear spawn → stream → forward → terminal-frame pipeline with a select over
    // shutdown / child output; complexity is the per-line/per-event handling, not
    // nesting. Splitting would fragment the single-turn lifecycle.
    #[allow(clippy::cognitive_complexity)]
    async fn launch_turn(
        &mut self,
        local_id: &str,
        cwd: &str,
        shared_args: Vec<String>,
        prompt: &str,
        env: &std::collections::BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        let mut command = Command::new(&self.cfg.claude_bin);
        command
            .current_dir(cwd)
            .arg("--print")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--include-partial-messages")
            .args(&shared_args)
            .arg("--")
            .arg(prompt)
            .envs(env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        crate::childenv::ScrubChildEnv::scrub_child_env(&mut command);

        let mut child = command
            .spawn()
            .with_context(|| format!("spawning `{} -p` in {cwd}", self.cfg.claude_bin))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr_ring =
            child.stderr.take().map(|stderr| streamjson::spawn_stderr_ring(stderr, "oneshot"));
        // Register so Kill/Interrupt can terminate this turn.
        self.running.insert(local_id.to_owned(), child);

        let mut reader = BufReader::new(stdout).lines();
        let mut cursor = streamjson::StreamCursor::default();
        let mut crashed: Option<EndReason> = None;
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => {
                    self.kill(local_id).await;
                    return Ok(());
                }
                line = reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            let mut out = Vec::new();
                            let outcome = streamjson::parse_stream_line(
                                local_id,
                                &line,
                                &mut cursor,
                                &mut out,
                            );
                            for evt in out {
                                self.emit(evt).await;
                            }
                            if let Some(end) = outcome.end {
                                if let EndReason::Crashed { .. } = end {
                                    crashed = Some(end);
                                }
                                break;
                            }
                        }
                        // EOF: child closed stdout — turn done.
                        Ok(None) => break,
                        Err(err) => {
                            tracing::warn!(%err, %local_id, "oneshot stdout read error");
                            break;
                        }
                    }
                }
            }
        }

        // Reap the child and clear the running slot.
        if let Some(mut child) = self.running.remove(local_id) {
            match child.wait().await {
                Ok(status) if !status.success() && crashed.is_none() => {
                    let tail = match &stderr_ring {
                        Some(ring) => streamjson::stderr_tail(ring).await,
                        None => String::new(),
                    };
                    crashed = Some(EndReason::Crashed {
                        detail: streamjson::exit_detail("claude -p", status, &tail),
                    });
                }
                Ok(_) => {}
                Err(err) => tracing::warn!(%err, %local_id, "oneshot child wait failed"),
            }
        }

        if let Some(reason) = crashed {
            // A failed turn ends the session; the transcript still resumes, but
            // surface the failure rather than masking it as idle.
            self.emit(AdapterEvent::SessionEnded { local_id: local_id.to_owned(), reason }).await;
        } else {
            // Successful turn: keep the session resumable (do NOT SessionEnded).
            // Mirror how --bg idles awaiting input.
            self.emit(idle_status(local_id)).await;
        }
        Ok(())
    }

    /// Pull the session's gateway-routing env from the server,
    /// merging the carried `hint`. Fail-closed when account-bound but
    /// unmintable; best-effort hint when no server is configured.
    async fn resolve_launch_env(
        &self,
        local_id: &str,
        hint: &std::collections::BTreeMap<String, String>,
    ) -> anyhow::Result<super::control::LaunchEnv> {
        super::control::resolve_launch_env_for(
            self.server.as_ref(),
            self.machine_key.as_ref(),
            local_id,
            hint,
        )
        .await
    }

    /// Terminate the in-flight child for `local_id`, if any.
    async fn kill(&mut self, local_id: &str) {
        if let Some(mut child) = self.running.remove(local_id) {
            let _ = child.start_kill();
            let _ = child.wait().await;
            tracing::info!(%local_id, "oneshot killed in-flight turn");
        }
    }

    async fn kill_all(&mut self) {
        let ids: Vec<String> = self.running.keys().cloned().collect();
        for id in ids {
            self.kill(&id).await;
        }
    }

    /// Register a session id in the hook map (identity mapping) and pin its cwd.
    fn register(&mut self, session_id: &str, cwd: &str) {
        self.cwds.insert(session_id.to_owned(), cwd.to_owned());
        if let Ok(mut m) = self.session_map.lock() {
            m.insert(session_id.to_owned(), session_id.to_owned());
        }
    }

    fn forget(&self, session_id: &str) {
        if let Ok(mut m) = self.session_map.lock() {
            m.remove(session_id);
        }
    }

    async fn emit(&self, evt: AdapterEvent) {
        let _ = self.events.send(evt).await;
    }
}

#[async_trait::async_trait]
impl SessionDriver for OneshotDriver {
    fn adapter_id(&self) -> &'static str {
        "claude-code"
    }

    async fn spawn(
        &mut self,
        spec: SessionSpec,
        _command_id: Option<uuid::Uuid>,
        session_id: Option<uuid::Uuid>,
    ) -> CommandOutcome {
        let session_id =
            session_id.map_or_else(|| uuid::Uuid::new_v4().to_string(), |id| id.to_string());
        self.spawn(&spec, session_id, None).await?;
        Ok(Handled::Done)
    }

    async fn fork(
        &mut self,
        parent_local_id: String,
        spec: SessionSpec,
        _command_id: Option<uuid::Uuid>,
        session_id: Option<String>,
        _extract: Option<cctui_proto::adapter::ForkExtract>,
    ) -> CommandOutcome {
        let child_id = session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        self.spawn(&spec, child_id, Some(parent_local_id)).await?;
        Ok(Handled::Done)
    }

    async fn reply(
        &mut self,
        local_id: String,
        text: String,
        _ask_picks: Option<Vec<Vec<usize>>>,
        env: std::collections::BTreeMap<String, String>,
        _command_id: Option<uuid::Uuid>,
        _turn_id: Option<uuid::Uuid>,
    ) -> CommandOutcome {
        self.reply(&local_id, &text, env).await?;
        Ok(Handled::Done)
    }

    async fn send_message(&mut self, local_id: String, text: String) -> CommandOutcome {
        self.reply(&local_id, &text, std::collections::BTreeMap::new()).await?;
        Ok(Handled::Done)
    }

    async fn resume(
        &mut self,
        local_id: String,
        working_dir: Option<String>,
        _env: std::collections::BTreeMap<String, String>,
    ) -> CommandOutcome {
        self.resume(&local_id, working_dir).await?;
        Ok(Handled::Done)
    }

    async fn kill(&mut self, local_id: String, _signal: Option<i32>) -> CommandOutcome {
        self.kill(&local_id).await;
        Ok(Handled::Done)
    }

    async fn interrupt(
        &mut self,
        local_id: String,
        _command_id: Option<uuid::Uuid>,
    ) -> CommandOutcome {
        self.kill(&local_id).await;
        Ok(Handled::Done)
    }

    async fn remove(
        &mut self,
        local_id: String,
        _command_id: Option<uuid::Uuid>,
        _initiator: cctui_proto::adapter::RemoveInitiator,
    ) -> CommandOutcome {
        self.kill(&local_id).await;
        self.names.remove(&local_id);
        self.cwds.remove(&local_id);
        self.hook_postures.remove(&local_id);
        self.forget(&local_id);
        self.emit(AdapterEvent::SessionEnded { local_id, reason: EndReason::Killed }).await;
        Ok(Handled::Done)
    }

    async fn permission_response(
        &mut self,
        local_id: String,
        request_id: String,
        allow: bool,
    ) -> CommandOutcome {
        // Headless: the only carrier is the bidirectional PreToolUse hook
        // long-polling in the listener; there is no PTY keystroke fallback.
        let key = (local_id.clone(), request_id.clone());
        let hook = self.pending_perm_hooks.lock().ok().and_then(|mut m| m.remove(&key));
        if let Some(tx) = hook {
            if tx.send(allow).is_ok() {
                tracing::info!(%local_id, %request_id, allow, "oneshot answered permission via hook");
                return Ok(Handled::Done);
            }
            tracing::debug!(%local_id, %request_id, "oneshot perm hook receiver gone");
        } else {
            tracing::warn!(%local_id, %request_id, "oneshot: no pending permission hook to answer");
        }
        Ok(Handled::Done)
    }

    async fn rename(&mut self, local_id: String, name: String) -> CommandOutcome {
        self.names.insert(local_id, name);
        Ok(Handled::Done)
    }

    async fn set_model(
        &mut self,
        local_id: String,
        _model: Option<String>,
        _effort: Option<String>,
        _command_id: Option<uuid::Uuid>,
    ) -> CommandOutcome {
        tracing::warn!(%local_id, "claude oneshot: in-place model/effort switch not supported; fork to change model");
        anyhow::bail!(
            "in-place model/effort switch is not supported for claude sessions — fork to change model"
        );
    }
}

fn started_meta(
    spec: &SessionSpec,
    session_id: &str,
    cwd: &str,
    fork_parent: Option<String>,
) -> SessionMeta {
    let (parent, relation) = fork_parent.map_or_else(
        || (spec.parent_local_id.clone(), spec.parent_local_id.as_ref().map(|_| "subagent")),
        |parent| (Some(parent), Some("fork")),
    );
    SessionMeta {
        working_dir: Some(cwd.to_owned()),
        parent_local_id: parent,
        extra: relation.map_or(
            serde_json::Value::Null,
            |relation| serde_json::json!({"relation":relation, "spawn_key":session_id}),
        ),
    }
}

/// The idle [`AdapterEvent::Status`] emitted after a successful oneshot turn:
/// the conversation is done with this turn but stays resumable (mirrors how the
/// bg worker idles awaiting input).
fn idle_status(local_id: &str) -> AdapterEvent {
    AdapterEvent::Status {
        local_id: local_id.to_owned(),
        tempo: Some("idle".to_owned()),
        state: Some("done".to_owned()),
        detail: None,
        activity: None,
        name: None,
        intent: None,
        model: None,
        effort: None,
        permission_mode: None,
        children: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::control::{ConfigRootOverride, HookPosture, HookSettingsError};
    use super::*;

    fn refusing_driver(tmp: &std::path::Path) -> (OneshotDriver, mpsc::Receiver<AdapterEvent>) {
        let (events, rx) = mpsc::channel(8);
        let (_commands_tx, commands) = mpsc::channel(8);
        let (_connected_tx, connected) = tokio::sync::broadcast::channel(1);
        let bin = tmp.join("claude-must-not-run");
        let ctx = AdapterCtx {
            events,
            commands,
            pty_watch: None,
            interrupts: None,
            shutdown: CancellationToken::new(),
            config: serde_json::json!({ "claude_bin": bin }),
            server: None,
            machine_key: None,
            connected,
        };
        (OneshotDriver::new(ctx).0, rx)
    }

    fn policy_spec(cwd: &std::path::Path) -> SessionSpec {
        serde_json::from_value(serde_json::json!({
            "adapter_id": "claude-code",
            "working_dir": cwd,
            "prompt": "go",
            "hook_policy": { "rules": [{ "event": "Stop", "command": "p-stop" }] },
        }))
        .unwrap()
    }

    fn refused_for_hook_settings(launched: anyhow::Result<()>) {
        let err = launched.expect_err("the launch was not refused");
        assert!(err.downcast_ref::<HookSettingsError>().is_some(), "{err:#}");
    }

    #[tokio::test]
    async fn a_spawn_is_refused_without_starting_claude_when_its_settings_cannot_be_written() {
        let tmp = tempfile::tempdir().unwrap();
        let _root = ConfigRootOverride::unwritable(tmp.path());
        let (mut d, mut events) = refusing_driver(tmp.path());
        let id = "5e550001-0000-4000-8000-000000000000".to_owned();
        refused_for_hook_settings(d.spawn(&policy_spec(tmp.path()), id, None).await);
        assert!(d.running.is_empty(), "no `claude -p` child was started");
        assert!(d.cwds.is_empty() && d.hook_postures.is_empty());
        assert!(events.try_recv().is_err(), "no SessionStarted for a refused spawn");
    }

    #[tokio::test]
    async fn a_turn_is_refused_without_starting_claude_when_its_settings_cannot_be_written() {
        let tmp = tempfile::tempdir().unwrap();
        let _root = ConfigRootOverride::unwritable(tmp.path());
        let (mut d, mut events) = refusing_driver(tmp.path());
        let id = "5e550002-0000-4000-8000-000000000000";
        d.cwds.insert(id.to_owned(), tmp.path().to_string_lossy().into_owned());
        d.hook_postures.insert(id.to_owned(), HookPosture::default());
        refused_for_hook_settings(d.reply(id, "next", std::collections::BTreeMap::new()).await);
        assert!(d.running.is_empty(), "no `claude -p` child was started");
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn child_registration_keeps_the_parent_and_launch_key_for_policy_enforcement() {
        let spec: SessionSpec = serde_json::from_value(serde_json::json!({
            "adapter_id":"claude-code", "working_dir":"/work/acme", "parent_local_id":"parent"
        }))
        .unwrap();
        let meta = started_meta(&spec, "child-key", "/work/acme", None);
        assert_eq!(meta.parent_local_id.as_deref(), Some("parent"));
        assert_eq!(meta.extra["relation"], "subagent");
        assert_eq!(meta.extra["spawn_key"], "child-key");
        let fork = started_meta(&spec, "fork-key", "/work/acme", Some("fork-source".into()));
        assert_eq!(fork.parent_local_id.as_deref(), Some("fork-source"));
        assert_eq!(fork.extra["relation"], "fork");
    }

    #[test]
    fn idle_status_is_resumable_not_ended() {
        // A successful oneshot turn must keep the session resumable: an idle
        // Status, never a SessionEnded.
        match idle_status("sid-1") {
            AdapterEvent::Status { local_id, tempo, state, .. } => {
                assert_eq!(local_id, "sid-1");
                assert_eq!(tempo.as_deref(), Some("idle"));
                assert_eq!(state.as_deref(), Some("done"));
            }
            other => panic!("expected idle Status, got {other:?}"),
        }
    }
}
