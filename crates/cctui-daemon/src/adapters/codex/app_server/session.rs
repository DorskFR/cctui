use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use cctui_proto::adapter::{AdapterEvent, EndReason, SessionMeta};
use cctui_proto::codex_catalog::CodexModelCatalog;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::config::{AppServerConfig, shared_eligible, shared_overlay};
use super::diagnose::{DiagnoseRings, stderr_tail};
use super::registry::{LiveSessionRegistry, SessionCommand, SessionRecord, SessionRegistry};
use super::requests::{ThreadConfig, thread_name_set_req};
use super::rpc::{ID_THREAD_START, PendingRpcs, RPC_TIMEOUT, RpcStdin};

/// Holds the spawn/fork `command_id` until the launch outcome is known: the
/// success ack is deferred to `thread/start`/`thread/resume`/`thread/fork`
/// succeeding, and every failure path (JSON-RPC error, timeout, process exit,
/// spawn error) resolves it as a failure instead. One-shot: the
/// first resolution wins, later calls are no-ops.
pub(super) struct SpawnAck {
    command_id: Option<Uuid>,
    events: mpsc::Sender<AdapterEvent>,
}

impl SpawnAck {
    pub(super) async fn ok(&mut self) {
        if let Some(command_id) = self.command_id.take() {
            let _ = self
                .events
                .send(AdapterEvent::CommandResult { command_id, ok: true, error: None })
                .await;
        }
    }

    pub(super) async fn fail(&mut self, error: &str) {
        if let Some(command_id) = self.command_id.take() {
            let _ = self
                .events
                .send(AdapterEvent::CommandResult {
                    command_id,
                    ok: false,
                    error: Some(error.to_owned()),
                })
                .await;
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum SessionLaunch {
    Fresh {
        prompt: Option<String>,
        name: Option<String>,
        /// Staged spawn-attachment paths, fed into the first turn.
        attachments: Vec<String>,
    },
    Resume {
        thread_id: String,
        initial_commands: Vec<SessionCommand>,
    },
    /// Fork a parent thread into a new one seeded from its history.
    /// Post-fork it behaves like `Fresh` (optional name + first turn), but the
    /// start handshake sends `thread/fork { threadId }` and the resulting
    /// `SessionStarted` carries `parent_local_id` for discoverability.
    Fork {
        parent_thread_id: String,
        prompt: Option<String>,
        name: Option<String>,
        attachments: Vec<String>,
    },
}

/// One spawned Codex session: owns a `codex app-server` subprocess and a
/// single thread within it.
pub struct CodexSession {
    pub(super) cfg: AppServerConfig,
    pub(super) cwd: String,
    /// Launch-time env merged onto the `codex app-server` child process.
    /// Holds the gateway-routing credential resolved at spawn /
    /// fork / resume; see [`SessionRecord::env`].
    pub(super) env: std::collections::BTreeMap<String, String>,
    pub(super) launch: SessionLaunch,
    /// Spawn/fork correlation id: resolved as an
    /// [`AdapterEvent::CommandResult`] only once the launch outcome is known.
    pub(super) command_id: Option<Uuid>,
    /// Server-pre-minted session id, echoed on `SessionStarted` so childwatch
    /// can bind the thread codex mints to the `CctuiAgent` waiter.
    pub(super) spawn_key: Option<String>,
    /// The spawning parent session for a `CctuiAgent` child, carried onto
    /// `SessionStarted` so the server nests it under its caller.
    pub(super) parent_local_id: Option<String>,
    /// `CctuiAgent` relay to declare to this app-server, when the server granted
    /// the session spawn rights. `None` means the tool is absent — a session
    /// without a capability must not be able to see it.
    pub(super) agent_mcp: Option<crate::adapters::agent_mcp::AgentMcp>,
    pub(super) events: mpsc::Sender<AdapterEvent>,
    pub(super) live: LiveSessionRegistry,
    pub(super) registry: SessionRegistry,
    pub(super) shutdown: CancellationToken,
}

impl CodexSession {
    #[must_use]
    pub fn with_agent_mcp(
        mut self,
        agent_mcp: Option<crate::adapters::agent_mcp::AgentMcp>,
    ) -> Self {
        self.agent_mcp = agent_mcp;
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub const fn new_fresh(
        cfg: AppServerConfig,
        cwd: String,
        env: std::collections::BTreeMap<String, String>,
        prompt: Option<String>,
        name: Option<String>,
        attachments: Vec<String>,
        command_id: Option<Uuid>,
        spawn_key: Option<String>,
        parent_local_id: Option<String>,
        events: mpsc::Sender<AdapterEvent>,
        live: LiveSessionRegistry,
        registry: SessionRegistry,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            cfg,
            cwd,
            env,
            launch: SessionLaunch::Fresh { prompt, name, attachments },
            command_id,
            spawn_key,
            parent_local_id,
            agent_mcp: None,
            events,
            live,
            registry,
            shutdown,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub const fn new_fork(
        cfg: AppServerConfig,
        cwd: String,
        env: std::collections::BTreeMap<String, String>,
        parent_thread_id: String,
        prompt: Option<String>,
        name: Option<String>,
        attachments: Vec<String>,
        command_id: Option<Uuid>,
        events: mpsc::Sender<AdapterEvent>,
        live: LiveSessionRegistry,
        registry: SessionRegistry,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            cfg,
            cwd,
            env,
            launch: SessionLaunch::Fork { parent_thread_id, prompt, name, attachments },
            command_id,
            spawn_key: None,
            parent_local_id: None,
            agent_mcp: None,
            events,
            live,
            registry,
            shutdown,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub const fn new_resume(
        cfg: AppServerConfig,
        cwd: String,
        env: std::collections::BTreeMap<String, String>,
        thread_id: String,
        initial_commands: Vec<SessionCommand>,
        events: mpsc::Sender<AdapterEvent>,
        live: LiveSessionRegistry,
        registry: SessionRegistry,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            cfg,
            cwd,
            env,
            launch: SessionLaunch::Resume { thread_id, initial_commands },
            command_id: None,
            spawn_key: None,
            parent_local_id: None,
            agent_mcp: None,
            events,
            live,
            registry,
            shutdown,
        }
    }

    /// Spawn the subprocess, complete the handshake, then pump IO until the
    /// process exits, the session is killed, or the daemon shuts down.
    /// The spawn/fork `command_id` (when present) is resolved exactly once:
    /// `ok` after the thread request succeeds, failure on any other outcome.
    pub async fn run(mut self) -> Result<()> {
        let mut ack = SpawnAck { command_id: self.command_id.take(), events: self.events.clone() };
        let rings = Arc::new(DiagnoseRings::default());
        let res = self.run_inner(&mut ack, &rings).await;
        match &res {
            Err(err) => {
                let detail = format!("{err}{}", stderr_tail(&rings));
                self.fail_handshake(&mut ack, &detail).await;
            }
            Ok(()) => ack.fail("codex app-server exited before the thread was started").await,
        }
        res
    }

    /// Resolve the spawn ack as failed. A resume has no `command_id`, so its
    /// failure is reported on the thread instead: a failed `Status` plus a
    /// `SessionEnded` the server persists as `resume_failed`.
    pub(super) async fn fail_handshake(&self, ack: &mut SpawnAck, detail: &str) {
        ack.fail(detail).await;
        let SessionLaunch::Resume { thread_id, .. } = &self.launch else { return };
        self.events
            .send(AdapterEvent::Status {
                local_id: thread_id.clone(),
                tempo: None,
                state: Some("failed".to_owned()),
                detail: Some(detail.to_owned()),
                activity: Some("failure".to_owned()),
                name: None,
                intent: None,
                model: None,
                effort: None,
                permission_mode: None,
                children: Vec::new(),
            })
            .await
            .ok();
        self.events
            .send(AdapterEvent::SessionEnded {
                local_id: thread_id.clone(),
                reason: EndReason::ResumeFailed { detail: detail.to_owned() },
            })
            .await
            .ok();
    }

    pub(super) fn thread_config(&self, shared: bool) -> ThreadConfig {
        let config = ThreadConfig::new(&self.env, self.cfg.service_tier.as_deref());
        if shared { config.with_overlay(shared_overlay(&self.cfg, &self.env)) } else { config }
    }

    pub(super) fn thread_request(&self, config: &ThreadConfig) -> (Value, &'static str) {
        let (method, params) = match &self.launch {
            SessionLaunch::Fresh { .. } => ("thread/start", config.start_params(&self.cwd)),
            SessionLaunch::Resume { thread_id, .. } => {
                ("thread/resume", config.resume_params(thread_id, &self.cwd))
            }
            SessionLaunch::Fork { parent_thread_id, .. } => {
                ("thread/fork", config.fork_params(parent_thread_id, &self.cwd))
            }
        };
        (
            json!({"jsonrpc": "2.0", "id": ID_THREAD_START, "method": method, "params": params}),
            method,
        )
    }

    #[cfg(test)]
    fn stdio_thread_request(&self) -> (Value, &'static str) {
        self.thread_request(&self.thread_config(false))
    }

    /// A route on the shared app-server when this machine opted in and the
    /// session qualifies; `None` keeps the private stdio child.
    pub(super) async fn open_shared(
        &self,
    ) -> Option<(crate::adapters::codex::daemon::ThreadWire, ThreadConfig)> {
        if !shared_eligible(&self.env, self.agent_mcp.is_some()) {
            return None;
        }
        let handle = crate::adapters::codex::daemon::turn_transport().await?;
        let config = self.thread_config(true);
        match handle.open_thread(config.clone(), self.cwd.clone()).await {
            Ok(wire) => Some((wire, config)),
            Err(err) => {
                tracing::info!(%err, "codex: shared turn transport unavailable; using stdio");
                None
            }
        }
    }

    /// Fail a command that was sitting in the dead session's channel buffer,
    /// loudly: `CommandResult` when it carries an id, a failed `Status` for a
    /// user message so the drop is visible in the webui.
    pub(super) async fn fail_dropped_command(&self, local_id: &str, cmd: &SessionCommand) {
        if let Some(command_id) = cmd.command_id() {
            self.events
                .send(AdapterEvent::CommandResult {
                    command_id,
                    ok: false,
                    error: Some("codex app-server exited before the command was delivered".into()),
                })
                .await
                .ok();
        }
        if matches!(cmd, SessionCommand::Send { .. }) {
            self.events
                .send(AdapterEvent::Status {
                    local_id: local_id.to_owned(),
                    tempo: None,
                    state: Some("failed".to_owned()),
                    detail: Some(
                        "message dropped: codex app-server exited before it was delivered"
                            .to_owned(),
                    ),
                    activity: Some("failure".to_owned()),
                    name: None,
                    intent: None,
                    model: None,
                    effort: None,
                    permission_mode: None,
                    children: Vec::new(),
                })
                .await
                .ok();
        }
        tracing::warn!(%local_id, ?cmd, "codex: dropping buffered command — app-server gone");
    }
}

/// Only an explicit kill removes the durable record; a crash keeps it
/// resumable.
pub(super) const fn removes_record(reason: &EndReason) -> bool {
    matches!(reason, EndReason::Killed)
}

/// Split commands drained from a dead session's channel: resumable ones to
/// retry on the revived thread, the rest to fail visibly. A buffered `Kill`
/// wins over hibernation.
pub(super) fn partition_drained(
    drained: Vec<SessionCommand>,
) -> (Vec<SessionCommand>, Vec<SessionCommand>, bool) {
    let mut retry = Vec::new();
    let mut dropped = Vec::new();
    let mut killed = false;
    for cmd in drained {
        if matches!(cmd, SessionCommand::Kill { .. }) {
            killed = true;
        } else if cmd.is_resumable() {
            retry.push(cmd);
        } else {
            dropped.push(cmd);
        }
    }
    (retry, dropped, killed)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn set_thread_name(
    stdin: &mut RpcStdin,
    next_id: &mut i64,
    pending_rpcs: &mut PendingRpcs,
    thread_id: &str,
    name: &str,
    events: &mpsc::Sender<AdapterEvent>,
    registry: &SessionRegistry,
) -> Result<()> {
    pending_rpcs.insert(*next_id, "thread/name/set", None, Instant::now() + RPC_TIMEOUT);
    stdin.send(&thread_name_set_req(*next_id, thread_id, name)).await?;
    *next_id += 1;
    if let Some(record) = registry.lock().await.get_mut(thread_id) {
        record.name = Some(name.to_owned());
    }
    crate::adapters::codex::persist::save(registry).await;
    events
        .send(AdapterEvent::Status {
            local_id: thread_id.to_owned(),
            tempo: None,
            state: None,
            detail: None,
            activity: None,
            name: Some(name.to_owned()),
            intent: None,
            model: None,
            effort: None,
            permission_mode: None,
            children: Vec::new(),
        })
        .await
        .ok();
    Ok(())
}

/// Record an in-place model/effort change. Stashes the
/// override in `override_model`/`override_effort` (carried on the next
/// `turn/start`, which codex promotes to the later default — the stable path,
/// vs the `experimentalApi`-gated `thread/settings/update` codex 0.144.1
/// rejects) and folds it into the durable `SessionRecord` cfg so a resume
/// relaunches with matching `-c model=`/`-c model_reasoning_effort=` flags.
/// No app-server round-trip can reject it, so the chip (`Status`) and the
/// `command_id` ack (`CommandResult`) are truthful the moment they fire here.
#[allow(clippy::too_many_arguments)]
pub(super) async fn record_model_override(
    override_model: &mut Option<String>,
    override_effort: &mut Option<String>,
    model: Option<&str>,
    effort: Option<&str>,
    thread_id: &str,
    events: &mpsc::Sender<AdapterEvent>,
    registry: &SessionRegistry,
    command_id: Option<Uuid>,
) {
    if let Some(model) = model {
        *override_model = Some(model.to_owned());
    }
    if let Some(effort) = effort {
        *override_effort = Some(effort.to_owned());
    }
    if let Some(record) = registry.lock().await.get_mut(thread_id) {
        if let Some(model) = model {
            record.cfg.model = Some(model.to_owned());
        }
        if let Some(effort) = effort {
            record.cfg.reasoning_effort = Some(effort.to_owned());
        }
    }
    crate::adapters::codex::persist::save(registry).await;
    events
        .send(AdapterEvent::Status {
            local_id: thread_id.to_owned(),
            tempo: None,
            state: None,
            detail: None,
            activity: None,
            name: None,
            intent: None,
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
            permission_mode: None,
            children: Vec::new(),
        })
        .await
        .ok();
    if let Some(command_id) = command_id {
        events.send(AdapterEvent::CommandResult { command_id, ok: true, error: None }).await.ok();
    }
}

/// A subagent the session's own thread spawned on the shared app-server
/// (`/agents`, `spawn_agent`), announced as a real child session nested under
/// it, the way a `CctuiAgent` child is.
pub(super) fn subagent_started_event(local_id: &str, value: &Value) -> Option<AdapterEvent> {
    if local_id.is_empty() || value.get("method").and_then(Value::as_str) != Some("thread/started")
    {
        return None;
    }
    let params = value.get("params")?;
    let (child, parent) = crate::adapters::codex::daemon::subagent_parent(params)?;
    if parent != local_id {
        return None;
    }
    let thread = &params["thread"];
    Some(AdapterEvent::SessionStarted {
        local_id: child,
        meta: SessionMeta {
            working_dir: thread.get("cwd").and_then(Value::as_str).map(str::to_owned),
            parent_local_id: Some(parent),
            extra: json!({
                "source": "codex-app-server",
                "rollout_path": thread.get("path"),
                "relation": "subagent",
                "agent_nickname": thread.get("agentNickname"),
                "agent_role": thread.get("agentRole"),
            }),
        },
    })
}

/// The `(parent_local_id, relation)` a freshly started thread reports, so the
/// server resolves `parent_id`. Relation `"subagent"` — not `"fork"` — is what
/// the webui nests, so a `CctuiAgent` child must not be labelled a fork.
#[must_use]
pub(super) fn child_linkage(
    launch: &SessionLaunch,
    parent_local_id: Option<&str>,
) -> (Option<String>, Option<&'static str>) {
    match launch {
        SessionLaunch::Fork { parent_thread_id, .. } => {
            (Some(parent_thread_id.clone()), Some("fork"))
        }
        _ => (parent_local_id.map(str::to_owned), parent_local_id.map(|_| "subagent")),
    }
}

/// The `CctuiAgent` relay a resume re-declares: this daemon's remembered launch
/// decision, else the persisted one, which is all a rediscovered thread has.
#[must_use]
pub fn resume_relay(
    thread_id: &str,
    record: &SessionRecord,
) -> Option<crate::adapters::agent_mcp::AgentMcp> {
    crate::adapters::agent_mcp::recall(thread_id).or_else(|| {
        record
            .spawn_relay
            .then(|| crate::adapters::agent_mcp::AgentMcp::for_session(thread_id))
            .flatten()
    })
}

pub fn spawn_resumed_session(
    record: SessionRecord,
    thread_id: &str,
    commands: Vec<SessionCommand>,
    events: mpsc::Sender<AdapterEvent>,
    live: LiveSessionRegistry,
    registry: SessionRegistry,
    shutdown: CancellationToken,
) {
    let commands: Vec<SessionCommand> = commands
        .into_iter()
        .filter(|command| {
            let ok = command.is_resumable();
            if !ok {
                tracing::warn!(%thread_id, ?command, "codex: command is not resumable");
            }
            ok
        })
        .collect();
    if commands.is_empty() {
        return;
    }
    let relay = resume_relay(thread_id, &record);
    let session = CodexSession::new_resume(
        record.cfg,
        record.cwd,
        record.env,
        thread_id.to_owned(),
        commands,
        events,
        live,
        registry,
        shutdown,
    )
    .with_agent_mcp(relay);
    tokio::spawn(async move {
        if let Err(err) = session.run().await {
            tracing::error!(%err, "codex resumed app-server session ended in error");
        }
    });
}

/// `Some(warning)` when `model` is set and absent from the catalog (by id or
/// underlying slug). Advisory only: a catalog can be stale or a machine-local
/// fallback, and there is no way to un-stick it from the UI, so it must never
/// block a spawn — codex rejects a genuinely bad model itself. An empty catalog
/// cannot vouch for anything and passes.
pub(super) fn unknown_model(model: Option<&str>, catalog: &CodexModelCatalog) -> Option<String> {
    let model = model?;
    if catalog.models.is_empty() || catalog.models.iter().any(|m| m.id == model || m.model == model)
    {
        return None;
    }
    let mut available: Vec<&str> =
        catalog.models.iter().filter(|m| !m.hidden).map(|m| m.id.as_str()).collect();
    if available.is_empty() {
        available = catalog.models.iter().map(|m| m.id.as_str()).collect();
    }
    Some(format!("unknown model {model}; available: {}", available.join(", ")))
}

pub(super) fn kill_child(child: &mut Option<tokio::process::Child>) {
    if let Some(child) = child.as_mut() {
        let _ = child.start_kill();
    }
}

/// SIGTERM, per POSIX. The control-plane `Kill { signal }` uses raw signal
/// numbers; 15 is the one graceful case we special-case.
pub(super) const SIGTERM: i32 = 15;

/// Terminate the child with the requested signal. `Some(15)` (SIGTERM)
/// gives codex a chance to flush its rollout file; anything else (incl.
/// `None`) is an immediate SIGKILL via tokio's `start_kill`.
pub(super) fn terminate_child(child: &mut tokio::process::Child, signal: Option<i32>) {
    if signal == Some(SIGTERM)
        && let Some(pid) =
            child.id().and_then(|p| i32::try_from(p).ok()).and_then(rustix::process::Pid::from_raw)
    {
        // A reaped pid just yields ESRCH, which we ignore.
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        return;
    }
    let _ = child.start_kill();
}

#[cfg(test)]
mod tests {
    use super::super::registry::{RouteAction, route_or_prepare_resume};
    use super::*;
    use crate::adapters::codex::model_list;

    fn fresh_launch() -> SessionLaunch {
        SessionLaunch::Fresh { prompt: None, name: None, attachments: Vec::new() }
    }

    #[test]
    fn a_cctui_agent_child_reports_its_spawning_parent_as_a_subagent() {
        let (parent, relation) = child_linkage(&fresh_launch(), Some("parent-thread-1"));
        assert_eq!(parent.as_deref(), Some("parent-thread-1"));
        assert_eq!(
            relation,
            Some("subagent"),
            "the webui nests on \"subagent\"; anything else orphans the child"
        );
    }

    #[test]
    fn a_parentless_thread_reports_no_linkage() {
        assert_eq!(child_linkage(&fresh_launch(), None), (None, None));
    }

    #[test]
    fn a_fork_links_to_its_parent_thread_as_a_fork() {
        let launch = SessionLaunch::Fork {
            parent_thread_id: "thread-7".to_owned(),
            prompt: None,
            name: None,
            attachments: Vec::new(),
        };
        assert_eq!(
            child_linkage(&launch, Some("ignored")),
            (Some("thread-7".to_owned()), Some("fork"))
        );
    }

    #[test]
    fn a_codex_session_with_a_capability_launches_the_agent_relay() {
        let cap = cctui_proto::api::SpawnCapability {
            adapters: vec!["codex".to_owned()],
            ..Default::default()
        };
        let with = crate::adapters::agent_mcp::AgentMcp::for_capability("key-1", Some(&cap));
        assert!(with.is_some(), "a granted capability must register the MCP tool");
        let keys: Vec<String> =
            with.unwrap().codex_config_overrides().into_iter().map(|(k, _)| k).collect();
        assert!(
            keys.iter().any(|k| k.starts_with("mcp_servers.")),
            "the relay must ride codex `-c mcp_servers.…`; got {keys:?}"
        );
        assert!(
            crate::adapters::agent_mcp::AgentMcp::for_capability("key-1", None).is_none(),
            "a session with no capability must not see the tool at all"
        );
    }

    fn relay_record(spawn_relay: bool) -> SessionRecord {
        SessionRecord {
            cfg: AppServerConfig::default(),
            cwd: "/repo".to_owned(),
            name: Some("worker".to_owned()),
            env: std::iter::once(("OPENAI_API_KEY".to_owned(), "sk-live".to_owned())).collect(),
            spawn_relay,
        }
    }

    /// The restart path: a relay session's record goes through the real on-disk
    /// snapshot, a fresh daemon merges it into an empty registry with nothing
    /// remembered in-process, and the resume must still declare the tool.
    #[tokio::test]
    async fn a_rediscovered_thread_keeps_the_spawn_tool_across_a_daemon_restart() {
        let thread_id = "thread_0199restartrelay";
        let cap = cctui_proto::api::SpawnCapability {
            adapters: vec!["codex".to_owned()],
            ..Default::default()
        };
        let launched =
            crate::adapters::agent_mcp::AgentMcp::for_capability("launch-key-r", Some(&cap));
        assert!(
            launched.is_some(),
            "the launch must have had the relay for this test to mean anything"
        );

        let mut before = std::collections::HashMap::new();
        before.insert(thread_id.to_owned(), relay_record(launched.is_some()));
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("codex-sessions.json");
        crate::adapters::codex::persist::save_to(&path, &before).expect("snapshot written");

        let restarted = SessionRegistry::default();
        let restored = crate::adapters::codex::persist::merge(
            &restarted,
            crate::adapters::codex::persist::load_from(&path),
        )
        .await;
        assert_eq!(restored, 1, "the thread is rediscovered from the snapshot");
        assert!(
            crate::adapters::agent_mcp::recall(thread_id).is_none(),
            "a restart must leave nothing remembered in-process, or this proves nothing"
        );

        let record = restarted.lock().await.get(thread_id).cloned().expect("record restored");
        let relay = resume_relay(thread_id, &record)
            .expect("a rediscovered thread that had the relay must still get it");
        assert_eq!(relay.session_key(), thread_id, "the relay keys onto the real thread id");
        let keys: Vec<String> =
            relay.codex_config_overrides().into_iter().map(|(k, _)| k).collect();
        assert!(
            keys.iter().any(|k| k.starts_with("mcp_servers.")),
            "the resumed thread must re-declare the relay; got {keys:?}"
        );
        assert!(
            !record.env.contains_key("OPENAI_API_KEY"),
            "the credential still must not survive the restart"
        );
    }

    #[tokio::test]
    async fn a_thread_that_never_had_the_relay_does_not_gain_it_on_resume() {
        let mut before = std::collections::HashMap::new();
        before.insert("thread_0199norelay".to_owned(), relay_record(false));
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("codex-sessions.json");
        crate::adapters::codex::persist::save_to(&path, &before).expect("snapshot written");
        let record = crate::adapters::codex::persist::load_from(&path)
            .remove("thread_0199norelay")
            .expect("record restored");
        assert!(
            resume_relay("thread_0199norelay", &record).is_none(),
            "fail-closed: a session the server never granted spawn rights must not see the tool"
        );
    }

    #[tokio::test]
    async fn record_model_override_updates_state_and_acks() {
        let registry = SessionRegistry::default();
        registry.lock().await.insert(
            "tid".to_owned(),
            SessionRecord {
                cfg: AppServerConfig::default(),
                cwd: "/tmp".to_owned(),
                name: None,
                env: std::collections::BTreeMap::new(),
                spawn_relay: false,
            },
        );
        let (tx, mut rx) = mpsc::channel(8);
        let mut model = None;
        let mut effort = None;
        let command_id = Uuid::new_v4();
        record_model_override(
            &mut model,
            &mut effort,
            Some("gpt-5-codex"),
            Some("high"),
            "tid",
            &tx,
            &registry,
            Some(command_id),
        )
        .await;
        // Override recorded for the next turn/start.
        assert_eq!(model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(effort.as_deref(), Some("high"));
        // Durable cfg folded in for a later resume's `-c` flags.
        let rec = registry.lock().await.get("tid").cloned().unwrap();
        assert_eq!(rec.cfg.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(rec.cfg.reasoning_effort.as_deref(), Some("high"));
        // Chip Status then a truthful ok CommandResult.
        let status = rx.recv().await.unwrap();
        assert!(
            matches!(status, AdapterEvent::Status { model: Some(m), .. } if m == "gpt-5-codex")
        );
        let ack = rx.recv().await.unwrap();
        assert!(
            matches!(ack, AdapterEvent::CommandResult { ok: true, command_id: c, .. } if c == command_id)
        );
    }

    #[test]
    fn unknown_model_lists_visible_ids() {
        let catalog = CodexModelCatalog {
            models: model_list::parse_model_list(&json!({"data": [
                {"id": "a", "hidden": false}, {"id": "b", "hidden": true}
            ]})),
            client_version: None,
        };
        assert_eq!(unknown_model(Some("a"), &catalog), None);
        assert_eq!(unknown_model(Some("b"), &catalog), None);
        assert_eq!(unknown_model(None, &catalog), None);
        assert_eq!(
            unknown_model(Some("x"), &catalog).as_deref(),
            Some("unknown model x; available: a")
        );
        assert_eq!(
            unknown_model(Some("x"), &CodexModelCatalog { models: vec![], client_version: None }),
            None
        );
    }

    #[test]
    fn crash_keeps_record_kill_removes_it() {
        assert!(removes_record(&EndReason::Killed));
        assert!(!removes_record(&EndReason::Crashed { detail: "boom".to_owned() }));
    }

    #[test]
    fn partition_drained_splits_retry_dropped_and_kill() {
        let cid = Uuid::new_v4();
        let drained = vec![
            SessionCommand::Send { text: "hi".to_owned(), command_id: None },
            SessionCommand::Permission { request_id: "r".to_owned(), allow: true },
            SessionCommand::Kill { signal: None },
            SessionCommand::Rename { name: "n".to_owned() },
            SessionCommand::Interrupt { command_id: Some(cid) },
        ];
        let (retry, dropped, killed) = partition_drained(drained);
        assert!(killed);
        assert_eq!(retry.len(), 2);
        assert!(matches!(&retry[0], SessionCommand::Send { text, .. } if text == "hi"));
        assert!(matches!(&retry[1], SessionCommand::Rename { name } if name == "n"));
        assert_eq!(dropped.len(), 2);
        assert_eq!(dropped[1].command_id(), Some(cid));

        let (retry, dropped, killed) = partition_drained(Vec::new());
        assert!(retry.is_empty() && dropped.is_empty() && !killed);
    }

    #[tokio::test]
    async fn terminate_child_sigterm_stops_the_process() {
        let mut child = tokio::process::Command::new("sleep").arg("300").spawn().unwrap();
        terminate_child(&mut child, Some(SIGTERM));
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .expect("child survived SIGTERM")
            .unwrap();
        assert!(!status.success());
    }

    fn session_with_tier(launch: SessionLaunch, tier: Option<&str>) -> CodexSession {
        let (events, _rx) = mpsc::channel(8);
        CodexSession {
            cfg: AppServerConfig {
                service_tier: tier.map(str::to_owned),
                ..AppServerConfig::default()
            },
            cwd: "/repo".to_owned(),
            env: std::collections::BTreeMap::default(),
            launch,
            command_id: None,
            spawn_key: None,
            parent_local_id: None,
            agent_mcp: None,
            events,
            live: LiveSessionRegistry::default(),
            registry: SessionRegistry::default(),
            shutdown: CancellationToken::new(),
        }
    }

    /// The resume trap: a `thread/resume` does not re-pull the gateway env, and
    /// codex persists no tier in the rollout, so the tier cached on the record
    /// must ride every resume or Fast lapses silently after the first one.
    #[test]
    fn a_resumed_session_re_supplies_the_cached_tier() {
        let (req, method) = session_with_tier(
            SessionLaunch::Resume { thread_id: "tid".to_owned(), initial_commands: Vec::new() },
            Some("fast"),
        )
        .stdio_thread_request();
        assert_eq!(method, "thread/resume");
        assert_eq!(req["params"]["config"]["service_tier"], "fast");
        assert_eq!(req["params"]["serviceTier"], "fast");
    }

    #[test]
    fn a_resumed_session_without_a_cached_tier_supplies_none() {
        let (req, _) = session_with_tier(
            SessionLaunch::Resume { thread_id: "tid".to_owned(), initial_commands: Vec::new() },
            None,
        )
        .stdio_thread_request();
        assert!(req["params"].get("serviceTier").is_none());
    }

    #[test]
    fn a_forked_session_re_supplies_the_cached_tier() {
        let (req, method) = session_with_tier(
            SessionLaunch::Fork {
                parent_thread_id: "parent".to_owned(),
                prompt: None,
                name: None,
                attachments: Vec::new(),
            },
            Some("fast"),
        )
        .stdio_thread_request();
        assert_eq!(method, "thread/fork");
        assert_eq!(req["params"]["config"]["service_tier"], "fast");
    }

    #[test]
    fn a_fresh_session_carries_the_tier_on_thread_start() {
        let (req, method) = session_with_tier(
            SessionLaunch::Fresh { prompt: None, name: None, attachments: Vec::new() },
            Some("default"),
        )
        .stdio_thread_request();
        assert_eq!(method, "thread/start");
        assert_eq!(req["params"]["config"]["service_tier"], "default");
        assert_eq!(req["params"]["serviceTier"], "default");
    }

    #[test]
    fn a_shared_resume_carries_the_process_knobs_a_stdio_child_took_as_flags() {
        let mut session = session_with_tier(
            SessionLaunch::Resume { thread_id: "tid".to_owned(), initial_commands: Vec::new() },
            Some("fast"),
        );
        session.cfg.model = Some("gpt-5.5".to_owned());
        session.cfg.approval_policy = "never".to_owned();
        let (req, method) = session.thread_request(&session.thread_config(true));
        assert_eq!(method, "thread/resume");
        let config = &req["params"]["config"];
        assert_eq!(config["model"], "gpt-5.5");
        assert_eq!(config["approval_policy"], "never");
        assert_eq!(config["sandbox_mode"], "workspace-write");
        assert_eq!(config["service_tier"], "fast");
        let (stdio, _) = session.stdio_thread_request();
        assert!(stdio["params"]["config"].get("model").is_none(), "stdio keeps them on -c");
    }

    #[test]
    fn a_subagent_of_this_thread_nests_as_a_child_session() {
        let started = json!({"method": "thread/started", "params": {"thread": {
            "id": "kid", "parentThreadId": "me", "cwd": "/repo", "agentNickname": "Ohm",
        }}});
        let Some(AdapterEvent::SessionStarted { local_id, meta }) =
            subagent_started_event("me", &started)
        else {
            panic!("expected a child SessionStarted");
        };
        assert_eq!(local_id, "kid");
        assert_eq!(meta.parent_local_id.as_deref(), Some("me"));
        assert_eq!(meta.extra["relation"], "subagent");
        assert_eq!(meta.extra["agent_nickname"], "Ohm");
        assert!(subagent_started_event("other", &started).is_none());
        assert!(subagent_started_event("", &started).is_none());
        let own = json!({"method": "thread/started", "params": {"thread": {"id": "me"}}});
        assert!(subagent_started_event("me", &own).is_none());
    }

    /// The record the registry stores after a launch is what a later resume
    /// relaunches from, so the tier must survive that round-trip.
    #[tokio::test]
    async fn a_hibernated_record_hands_its_tier_to_the_resume() {
        let registry = SessionRegistry::default();
        registry.lock().await.insert(
            "tid".to_owned(),
            SessionRecord {
                cfg: AppServerConfig {
                    service_tier: Some("fast".to_owned()),
                    ..AppServerConfig::default()
                },
                cwd: "/repo".to_owned(),
                name: None,
                env: std::collections::BTreeMap::default(),
                spawn_relay: false,
            },
        );
        let action = route_or_prepare_resume(
            &LiveSessionRegistry::default(),
            &registry,
            "tid",
            SessionCommand::Send { text: "hi".to_owned(), command_id: None },
        )
        .await;
        let RouteAction::Resume { record, .. } = action else { panic!("expected a resume") };
        let (req, _) = session_with_tier(
            SessionLaunch::Resume { thread_id: "tid".to_owned(), initial_commands: Vec::new() },
            record.cfg.service_tier.as_deref(),
        )
        .stdio_thread_request();
        assert_eq!(req["params"]["config"]["service_tier"], "fast");
    }
}
