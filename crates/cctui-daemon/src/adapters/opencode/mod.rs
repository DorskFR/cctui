//! `OpenCode` adapter: drives `opencode serve` over its HTTP API + SSE bus.

pub mod client;
pub mod config;
pub mod events;
pub mod normalize;
mod pty_view;
pub mod session;

use cctui_proto::adapter::AdapterEvent;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::adapter_runtime::{
    Adapter, AdapterCtx, AdapterFactory, CommandOutcome, Handled, SessionDriver, dispatch_command,
};
use crate::client::ServerClient;
use session::{LiveRegistry, OpenCodeConfig, OpenCodeSession, SessionCommand, SpawnParams};

pub const ADAPTER_ID: &str = "opencode";

/// Dispatch-payload env key naming the opencode agent profile to run under.
pub const AGENT_ENV: &str = "CCTUI_OPENCODE_AGENT";

/// Pull + decide the opencode launch env, keeping the `CctuiAgent` capability
/// the same pull serves: fail-closed on a missing/partial gateway env for an
/// account-bound session (see [`crate::adapters::gateway_env`]).
async fn resolve_launch(
    server: Option<&ServerClient>,
    machine_key: Option<&String>,
    local_id: &str,
    hint: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<crate::adapters::gateway_env::LaunchEnv> {
    crate::adapters::gateway_env::resolve_launch(
        "opencode",
        server,
        machine_key,
        local_id,
        hint,
        crate::adapters::gateway_env::FIREWORKS_GATEWAY_KEYS,
    )
    .await
}

pub struct OpenCodeAdapter;

#[async_trait::async_trait]
impl Adapter for OpenCodeAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    async fn start(&self, ctx: AdapterCtx) -> anyhow::Result<()> {
        let cfg = OpenCodeConfig::from_value(&ctx.config);
        match session::probe_version(&cfg.bin).await {
            Ok(version) => {
                tracing::info!(%version, pinned = client::OPENCODE_PINNED_VERSION, "opencode adapter ready");
            }
            Err(err) => tracing::error!(
                %err,
                bin = %cfg.bin,
                "opencode binary unavailable — spawns will fail until it is installed"
            ),
        }
        command_pump(cfg, ctx).await;
        Ok(())
    }
}

async fn command_pump(cfg: OpenCodeConfig, ctx: AdapterCtx) {
    pump(cfg, ctx, LiveRegistry::default()).await;
}

async fn pump(cfg: OpenCodeConfig, ctx: AdapterCtx, live: LiveRegistry) {
    let AdapterCtx {
        events,
        mut commands,
        shutdown,
        server,
        machine_key,
        mut connected,
        pty_watch,
        ..
    } = ctx;
    let _watch_handle = pty_watch.map(|watches| {
        let pump = pty_view::PtyWatchPump::new(live.clone(), events.clone(), shutdown.clone());
        tokio::spawn(pump.run(watches))
    });
    let mut pump = Pump { cfg, events, shutdown, server, machine_key, live };
    let mut announced: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut connect_closed = false;

    loop {
        tokio::select! {
            () = pump.shutdown.cancelled() => return,
            edge = connected.recv(), if !connect_closed => {
                // Lagged is an edge like any other — the signal has no payload.
                // Closed only disables this arm: a closed receiver returns
                // ready forever, and the pump still owns live sessions.
                if matches!(edge, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                    connect_closed = true;
                    continue;
                }
                announced.clear();
                announce_live_sessions(&pump.live, &pump.events, &mut announced).await;
            }
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { return };
                let events = pump.events.clone();
                dispatch_command(&mut pump, &events, cmd).await;
            }
        }
    }
}

/// Everything the command loop needs besides the channels it selects on.
struct Pump {
    cfg: OpenCodeConfig,
    events: mpsc::Sender<AdapterEvent>,
    shutdown: tokio_util::sync::CancellationToken,
    server: Option<ServerClient>,
    machine_key: Option<String>,
    live: LiveRegistry,
}

#[async_trait::async_trait]
impl SessionDriver for Pump {
    fn adapter_id(&self) -> &'static str {
        ADAPTER_ID
    }

    async fn spawn(
        &mut self,
        spec: cctui_proto::adapter::SessionSpec,
        command_id: Option<Uuid>,
        session_id: Option<Uuid>,
    ) -> CommandOutcome {
        self.start_session(spec, command_id, session_id).await;
        Ok(Handled::Deferred)
    }

    async fn fork(
        &mut self,
        parent_local_id: String,
        spec: cctui_proto::adapter::SessionSpec,
        command_id: Option<Uuid>,
        _session_id: Option<String>,
        _extract: Option<cctui_proto::adapter::ForkExtract>,
    ) -> CommandOutcome {
        self.fork_session(&parent_local_id, &spec, command_id).await;
        Ok(Handled::Deferred)
    }

    async fn send_message(&mut self, local_id: String, text: String) -> CommandOutcome {
        self.prompt(local_id, text, None).await;
        Ok(Handled::Deferred)
    }

    async fn reply(
        &mut self,
        local_id: String,
        text: String,
        _ask_picks: Option<Vec<Vec<usize>>>,
        _env: std::collections::BTreeMap<String, String>,
        command_id: Option<Uuid>,
        _turn_id: Option<Uuid>,
    ) -> CommandOutcome {
        self.prompt(local_id, text, command_id).await;
        Ok(Handled::Deferred)
    }

    async fn kill(&mut self, local_id: String, _signal: Option<i32>) -> CommandOutcome {
        self.kill_session(local_id).await;
        Ok(Handled::Deferred)
    }

    async fn remove(
        &mut self,
        local_id: String,
        _command_id: Option<Uuid>,
        _initiator: cctui_proto::adapter::RemoveInitiator,
    ) -> CommandOutcome {
        self.kill_session(local_id).await;
        Ok(Handled::Done)
    }

    async fn interrupt(&mut self, local_id: String, command_id: Option<Uuid>) -> CommandOutcome {
        let delivered =
            route(&self.live, &local_id, SessionCommand::Kill { session_id: local_id.clone() })
                .await;
        if let Some(command_id) = command_id {
            crate::adapters::emit(
                &self.events,
                AdapterEvent::CommandResult {
                    command_id,
                    ok: delivered,
                    error: (!delivered).then(|| "no live opencode session".to_owned()),
                },
            )
            .await;
        }
        Ok(Handled::Deferred)
    }

    async fn permission_response(
        &mut self,
        local_id: String,
        request_id: String,
        allow: bool,
    ) -> CommandOutcome {
        route(
            &self.live,
            &local_id,
            SessionCommand::Permission { session_id: local_id.clone(), request_id, allow },
        )
        .await;
        Ok(Handled::Deferred)
    }

    async fn diagnose(&mut self, local_id: String, request_id: Uuid) -> CommandOutcome {
        let report =
            diagnose(&self.live, &local_id, self.server.as_ref(), self.machine_key.as_ref()).await;
        let _ = self
            .events
            .send(AdapterEvent::Diagnose { local_id, request_id, report: Box::new(report) })
            .await;
        Ok(Handled::Deferred)
    }
}

impl Pump {
    async fn prompt(&self, local_id: String, text: String, command_id: Option<Uuid>) {
        let delivered = route(
            &self.live,
            &local_id,
            SessionCommand::Prompt { session_id: local_id.clone(), text, command_id },
        )
        .await;
        if !delivered {
            fail(&self.events, command_id, "no live opencode session").await;
        }
    }

    async fn kill_session(&self, local_id: String) {
        if !route(&self.live, &local_id, SessionCommand::Kill { session_id: local_id.clone() })
            .await
        {
            crate::adapters::emit(
                &self.events,
                AdapterEvent::SessionEnded {
                    local_id,
                    reason: cctui_proto::adapter::EndReason::Killed,
                },
            )
            .await;
        }
    }

    async fn fork_session(
        &self,
        parent_local_id: &str,
        spec: &cctui_proto::adapter::SessionSpec,
        command_id: Option<Uuid>,
    ) {
        let delivered = route(
            &self.live,
            parent_local_id,
            SessionCommand::Fork {
                parent: parent_local_id.to_owned(),
                prompt: spec.prompt.clone(),
                name: spec.name.clone(),
                command_id,
            },
        )
        .await;
        if !delivered {
            fail(
                &self.events,
                command_id,
                "opencode fork requires the parent session to be live on this \
                 daemon",
            )
            .await;
        }
    }

    async fn start_session(
        &self,
        spec: cctui_proto::adapter::SessionSpec,
        command_id: Option<Uuid>,
        session_id: Option<Uuid>,
    ) {
        let Some(working_dir) = spec.working_dir.clone() else {
            fail(&self.events, command_id, "working_dir required").await;
            return;
        };
        let key = session_id.or(command_id).map_or_else(String::new, |id| id.to_string());
        let launch =
            match resolve_launch(self.server.as_ref(), self.machine_key.as_ref(), &key, &spec.env)
                .await
            {
                Ok(launch) => launch,
                Err(err) => {
                    fail(&self.events, command_id, &err.to_string()).await;
                    return;
                }
            };
        let skills =
            crate::plugins::resolve_session_skills(self.server.as_ref(), &key, &launch.plugins)
                .await;
        // The gateway env wins: a plugin must not be able to reroute the model.
        let mut env = launch.env;
        for (name, value) in &skills.env {
            env.entry(name.clone()).or_insert_with(|| value.clone());
        }
        let agent_mcp = crate::adapters::agent_mcp::AgentMcp::for_capability(
            &key,
            launch.spawn_capability.as_ref(),
        );
        let attachments = match crate::adapters::uploads::stage_bootstrap(&key, &spec.bootstrap) {
            Ok(paths) => paths,
            Err(err) => {
                fail(&self.events, command_id, &format!("attachment staging failed: {err}")).await;
                return;
            }
        };
        let preflight = crate::preflight::Preflight::new(self.events.clone(), spec.model.clone())
            .with_limits(self.server.as_ref(), self.machine_key.as_deref())
            .with_relay(agent_mcp.as_ref().map(|relay| relay.session_key().to_owned()));
        let params = SpawnParams {
            cfg: self.cfg.clone(),
            key,
            cwd: working_dir,
            env,
            prompt: spec.prompt.clone(),
            name: spec.name.clone(),
            model: spec.model.clone(),
            agent: agent_of(&spec, &self.cfg),
            attachments,
            command_id,
            parent_local_id: spec.parent_local_id.clone(),
            agent_mcp,
            skill_roots: skills.roots,
            preflight: Some(preflight),
            context: launch.context,
        };
        let session = OpenCodeSession::new(
            params,
            self.events.clone(),
            self.live.clone(),
            self.shutdown.clone(),
        );
        tokio::spawn(session.run());
    }
}

/// Which opencode agent profile the spawn runs under: named by the dispatch
/// payload (`CCTUI_OPENCODE_AGENT`), else the bounded builder when the spawn
/// asked for `yolo` or `whip`, else the adapter default, else the locked-down
/// reviewer — opencode's own default agent has edit rights, arbitrary bash and
/// no step bound, which no cctui spawn may fall back to.
///
/// The permission mode is a ceiling: below `yolo`/`whip` a named agent is only
/// honoured when it is one this config locks down, since any other name (a
/// repo's own `opencode.json` agent included) may grant edits and bash.
fn agent_of(spec: &cctui_proto::adapter::SessionSpec, cfg: &OpenCodeConfig) -> Option<String> {
    use cctui_proto::adapter::PermissionMode;
    let may_build =
        matches!(spec.permission_mode, Some(PermissionMode::Yolo | PermissionMode::Whip));
    let within_mode = |agent: String| {
        if may_build || agent == config::REVIEWER_AGENT || agent == config::STOCK_AGENT {
            return Some(agent);
        }
        tracing::warn!(
            %agent,
            mode = ?spec.permission_mode,
            "opencode agent grants more than the permission mode; using the reviewer"
        );
        None
    };
    let named = spec.env.get(AGENT_ENV).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    named
        .and_then(within_mode)
        .or_else(|| may_build.then(|| config::BUILDER_AGENT.to_owned()))
        .or_else(|| cfg.default_agent.clone().and_then(within_mode))
        .or_else(|| Some(config::REVIEWER_AGENT.to_owned()))
}

/// Announce every session the adapter still drives, which on a `connected`
/// edge re-announces them: only a `SessionStarted` reverts the server's
/// `daemon_lost`, and an opencode session has no other event source.
///
/// The registry alone decides what is live; the server has no say, and its
/// resume marks never name an opencode session (they are keyed on
/// `transcript_offset`, which only the transcript-tailing adapters set).
///
/// `announced` is scoped to one connection: it stops repeated sweeps from
/// emitting twice, and is cleared on each edge because every new connection
/// starts from a server that has marked these sessions lost again.
async fn announce_live_sessions(
    live: &LiveRegistry,
    events: &mpsc::Sender<AdapterEvent>,
    announced: &mut std::collections::HashSet<String>,
) {
    let pending: Vec<(String, cctui_proto::adapter::SessionMeta)> = {
        let guard = live.lock().await;
        guard
            .iter()
            .filter(|(local_id, _)| !announced.contains(*local_id))
            .map(|(local_id, s)| (local_id.clone(), s.meta.clone()))
            .collect()
    };
    for (local_id, meta) in pending {
        announced.insert(local_id.clone());
        let _ = events.send(AdapterEvent::SessionStarted { local_id, meta }).await;
    }
}

async fn route(live: &LiveRegistry, local_id: &str, cmd: SessionCommand) -> bool {
    let Some(tx) = live.lock().await.get(local_id).map(|s| s.commands.clone()) else {
        tracing::warn!(%local_id, "opencode: no live session for command");
        return false;
    };
    tx.send(cmd).await.is_ok()
}

async fn fail(events: &mpsc::Sender<AdapterEvent>, command_id: Option<Uuid>, error: &str) {
    tracing::error!(%error, "opencode command failed");
    if let Some(command_id) = command_id {
        crate::adapters::emit(
            &events,
            AdapterEvent::CommandResult { command_id, ok: false, error: Some(error.to_owned()) },
        )
        .await;
    }
}

async fn diagnose(
    live: &LiveRegistry,
    local_id: &str,
    server: Option<&ServerClient>,
    machine_key: Option<&String>,
) -> cctui_proto::diagnose::SessionDiagnose {
    use cctui_proto::diagnose::{DiagnoseFact, EffectiveState, GatewayStatus, SessionDiagnose};

    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX);
    let live_present = live.lock().await.contains_key(local_id);
    let snapshot = session_snapshot(live, local_id).await;
    let verdict = match snapshot.as_ref() {
        Some(s) if s.in_flight => "working",
        Some(_) => "live",
        None if live_present => "live",
        None => "unknown session",
    };
    let opencode = opencode_section(snapshot.as_ref(), live_present);

    SessionDiagnose {
        local_id: local_id.to_owned(),
        short: None,
        generated_at_ms: now_ms,
        adapter: ADAPTER_ID.to_owned(),
        effective_state: DiagnoseFact::fresh(
            EffectiveState {
                verdict: verdict.to_owned(),
                tempo: None,
                state: Some(verdict.to_owned()),
                detail: None,
                activity: None,
            },
            "opencode-adapter",
            now_ms,
        ),
        last_hook_event: na(),
        attach: na(),
        pty_output: na(),
        claude_socket: na(),
        transcript: na(),
        prompts: na(),
        permission_mode: na(),
        dispatch: na(),
        gateway: DiagnoseFact::fresh(
            GatewayStatus { server_configured: server.is_some() && machine_key.is_some() },
            "daemon-config",
            now_ms,
        ),
        codex: None,
        opencode: Some(opencode),
    }
}

/// Ask the live driver for its snapshot. A session that exists but cannot
/// answer within the poll window yields `None`, and the section falls back to
/// what the registry alone knows.
async fn session_snapshot(
    live: &LiveRegistry,
    local_id: &str,
) -> Option<session::OpenCodeLiveSnapshot> {
    let tx = live.lock().await.get(local_id).map(|s| s.commands.clone())?;
    let (reply, mut rx) = mpsc::channel(1);
    tx.send(SessionCommand::Diagnose { reply }).await.ok()?;
    tokio::time::timeout(crate::adapters::ring_view::POLL_INTERVAL, rx.recv()).await.ok().flatten()
}

fn opencode_section(
    snapshot: Option<&session::OpenCodeLiveSnapshot>,
    live_present: bool,
) -> cctui_proto::diagnose::OpenCodeDiagnose {
    let pinned = client::OPENCODE_PINNED_VERSION.to_owned();
    let Some(snap) = snapshot else {
        return cctui_proto::diagnose::OpenCodeDiagnose {
            server_url: None,
            server_pid: None,
            pinned_version: pinned,
            server_version: None,
            version_matches: None,
            live: live_present,
            owned_sessions: Vec::new(),
            turn_status: "unknown".to_owned(),
            sse_connected: false,
            last_sse_event_ms: None,
            pending_permissions: Vec::new(),
            protocol_errors: Vec::new(),
            stderr_tail: Vec::new(),
            rpc_tail: Vec::new(),
        };
    };
    cctui_proto::diagnose::OpenCodeDiagnose {
        server_url: snap.server_url.clone(),
        server_pid: snap.server_pid,
        version_matches: snap.server_version.as_ref().map(|v| v.contains(&pinned)),
        pinned_version: pinned,
        server_version: snap.server_version.clone(),
        live: live_present,
        owned_sessions: snap.owned_sessions.clone(),
        turn_status: if snap.in_flight { "working" } else { "idle" }.to_owned(),
        sse_connected: snap.sse_connected,
        last_sse_event_ms: snap.last_sse_event_ms,
        pending_permissions: snap.pending_permissions.clone(),
        protocol_errors: snap.protocol_errors.clone(),
        stderr_tail: snap.stderr_tail.clone(),
        rpc_tail: snap.rpc_tail.clone(),
    }
}

fn na<T>() -> cctui_proto::diagnose::DiagnoseFact<T> {
    cctui_proto::diagnose::DiagnoseFact::missing(ADAPTER_ID, "claude-only fact")
}

pub struct OpenCodeFactory;

impl AdapterFactory for OpenCodeFactory {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }
    fn build(&self, _config: serde_json::Value) -> Box<dyn Adapter> {
        Box::new(OpenCodeAdapter)
    }
    fn pty_watch(&self, _config: &serde_json::Value) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    fn spec_with_env(env: &[(&str, &str)]) -> cctui_proto::adapter::SessionSpec {
        cctui_proto::adapter::SessionSpec {
            service_tier: None,
            adapter_id: ADAPTER_ID.into(),
            working_dir: Some("/repo".to_owned()),
            prompt: None,
            name: None,
            permission_mode: None,
            effort: None,
            model: None,
            env: env_of(env),
            bootstrap: serde_json::Value::Null,
            parent_local_id: None,
        }
    }

    #[test]
    fn dispatch_payload_selects_the_agent_profile() {
        let cfg = OpenCodeConfig::default();
        assert_eq!(
            agent_of(&spec_with_env(&[(AGENT_ENV, config::REVIEWER_AGENT)]), &cfg).as_deref(),
            Some(config::REVIEWER_AGENT)
        );
    }

    #[test]
    fn an_unconfigured_spawn_still_gets_the_locked_down_reviewer() {
        assert_eq!(
            agent_of(&spec_with_env(&[]), &OpenCodeConfig::default()).as_deref(),
            Some(config::REVIEWER_AGENT)
        );
        assert_eq!(
            agent_of(&spec_with_env(&[(AGENT_ENV, "  ")]), &OpenCodeConfig::default()).as_deref(),
            Some(config::REVIEWER_AGENT)
        );
    }

    #[test]
    fn a_yolo_or_whip_spawn_gets_the_builder() {
        use cctui_proto::adapter::PermissionMode;
        let cfg = OpenCodeConfig {
            default_agent: Some(config::REVIEWER_AGENT.to_owned()),
            ..OpenCodeConfig::default()
        };
        for mode in [PermissionMode::Yolo, PermissionMode::Whip] {
            let mut spec = spec_with_env(&[]);
            spec.permission_mode = Some(mode);
            assert_eq!(agent_of(&spec, &cfg).as_deref(), Some(config::BUILDER_AGENT));
        }
        for mode in [PermissionMode::Ask, PermissionMode::Auto] {
            let mut spec = spec_with_env(&[]);
            spec.permission_mode = Some(mode);
            assert_eq!(agent_of(&spec, &cfg).as_deref(), Some(config::REVIEWER_AGENT));
        }
        let mut spec = spec_with_env(&[(AGENT_ENV, config::REVIEWER_AGENT)]);
        spec.permission_mode = Some(PermissionMode::Yolo);
        assert_eq!(agent_of(&spec, &cfg).as_deref(), Some(config::REVIEWER_AGENT));
    }

    #[test]
    fn adapter_default_agent_applies_when_the_payload_is_silent() {
        let cfg = OpenCodeConfig {
            default_agent: Some(config::REVIEWER_AGENT.to_owned()),
            ..OpenCodeConfig::default()
        };
        assert_eq!(agent_of(&spec_with_env(&[]), &cfg).as_deref(), Some(config::REVIEWER_AGENT));
        assert_eq!(
            agent_of(&spec_with_env(&[(AGENT_ENV, "build")]), &cfg).as_deref(),
            Some("build")
        );
    }

    #[test]
    fn a_named_agent_never_lifts_a_spawn_above_its_permission_mode() {
        use cctui_proto::adapter::PermissionMode;
        let cfg = OpenCodeConfig::default();
        for named in [config::BUILDER_AGENT, "repo-agent-with-bash"] {
            for mode in [None, Some(PermissionMode::Ask), Some(PermissionMode::Auto)] {
                let mut spec = spec_with_env(&[(AGENT_ENV, named)]);
                spec.permission_mode = mode;
                assert_eq!(
                    agent_of(&spec, &cfg).as_deref(),
                    Some(config::REVIEWER_AGENT),
                    "{named} under {mode:?}"
                );
            }
            for mode in [PermissionMode::Yolo, PermissionMode::Whip] {
                let mut spec = spec_with_env(&[(AGENT_ENV, named)]);
                spec.permission_mode = Some(mode);
                assert_eq!(agent_of(&spec, &cfg).as_deref(), Some(named), "{named} under {mode:?}");
            }
        }
    }

    #[test]
    fn an_adapter_default_agent_is_capped_by_the_permission_mode_too() {
        use cctui_proto::adapter::PermissionMode;
        let cfg = OpenCodeConfig {
            default_agent: Some(config::BUILDER_AGENT.to_owned()),
            ..OpenCodeConfig::default()
        };
        let mut spec = spec_with_env(&[]);
        spec.permission_mode = Some(PermissionMode::Ask);
        assert_eq!(agent_of(&spec, &cfg).as_deref(), Some(config::REVIEWER_AGENT));
        spec.permission_mode = Some(PermissionMode::Yolo);
        assert_eq!(agent_of(&spec, &cfg).as_deref(), Some(config::BUILDER_AGENT));
    }
}

#[cfg(test)]
mod reconnect_tests {
    use cctui_proto::adapter::{AdapterEvent, SessionMeta};
    use tokio::sync::mpsc;

    use super::{LiveRegistry, announce_live_sessions, client, diagnose, pty_view, session};
    use crate::adapters::opencode::session::{LiveSession, SessionCommand};

    async fn registry_with(local_id: &str) -> LiveRegistry {
        let live = LiveRegistry::default();
        let (tx, _rx) = mpsc::channel(1);
        live.lock().await.insert(
            local_id.to_owned(),
            LiveSession {
                commands: tx,
                meta: SessionMeta {
                    working_dir: Some("/repo".to_owned()),
                    parent_local_id: None,
                    extra: serde_json::json!({ "harness": "opencode" }),
                },
            },
        );
        live
    }

    /// A registry entry whose driver answers `Diagnose` with a snapshot of the
    /// traffic it has seen.
    async fn registry_answering_diagnose(
        local_id: &str,
        snapshot: session::OpenCodeLiveSnapshot,
    ) -> LiveRegistry {
        let live = registry_with(local_id).await;
        let (tx, mut rx) = mpsc::channel(4);
        if let Some(entry) = live.lock().await.get_mut(local_id) {
            entry.commands = tx;
        }
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                if let SessionCommand::Diagnose { reply } = cmd {
                    let _ = reply.send(snapshot.clone()).await;
                }
            }
        });
        live
    }

    fn snapshot_with_traffic() -> session::OpenCodeLiveSnapshot {
        session::OpenCodeLiveSnapshot {
            server_url: Some("http://127.0.0.1:41234".to_owned()),
            server_pid: Some(4242),
            server_version: Some(client::OPENCODE_PINNED_VERSION.to_owned()),
            owned_sessions: vec!["ses_live".to_owned()],
            in_flight: true,
            sse_connected: true,
            last_sse_event_ms: Some(1_700_000_000_000),
            pending_permissions: vec!["perm_1".to_owned()],
            protocol_errors: vec![cctui_proto::diagnose::TrafficError {
                ts_ms: 1_700_000_000_000,
                message: "GET /event: 502 Bad Gateway".to_owned(),
                transport: "sse".to_owned(),
            }],
            stderr_tail: vec![cctui_proto::diagnose::TrafficStderrLine {
                ts_ms: 1_700_000_000_000,
                line: "serve listening".to_owned(),
            }],
            rpc_tail: vec![cctui_proto::diagnose::TrafficFrame {
                ts_ms: 1_700_000_000_000,
                direction: "out".to_owned(),
                label: "POST /session/ses_live/prompt_async".to_owned(),
                json: "{}".to_owned(),
                transport: "http".to_owned(),
            }],
        }
    }

    #[tokio::test]
    async fn the_report_carries_the_live_driver_traffic() {
        let live = registry_answering_diagnose("ses_live", snapshot_with_traffic()).await;

        let report = diagnose(&live, "ses_live", None, None).await;
        let oc = report.opencode.expect("no opencode section");

        assert_eq!(oc.server_pid, Some(4242));
        assert_eq!(oc.turn_status, "working");
        assert_eq!(oc.version_matches, Some(true));
        assert!(oc.live && oc.sse_connected);
        assert_eq!(oc.pending_permissions, vec!["perm_1".to_owned()]);
        assert_eq!(oc.rpc_tail[0].transport, "http");
        assert_eq!(oc.protocol_errors[0].transport, "sse");
        assert_eq!(oc.stderr_tail[0].line, "serve listening");
        assert_eq!(report.effective_state.value.expect("no state").verdict, "working");
    }

    /// An unknown session still gets a section, so the panel can say "not live"
    /// rather than rendering nothing at all.
    #[tokio::test]
    async fn an_unknown_session_reports_a_not_live_section() {
        let live = LiveRegistry::default();

        let oc = diagnose(&live, "ses_gone", None, None).await.opencode.expect("no section");

        assert!(!oc.live);
        assert_eq!(oc.turn_status, "unknown");
        assert_eq!(oc.pinned_version, client::OPENCODE_PINNED_VERSION);
        assert!(oc.rpc_tail.is_empty());
    }

    /// A watch must produce `PtyChunk` text; opencode has no PTY, so the rings
    /// rendered as lines are the live view.
    #[tokio::test]
    async fn a_watch_streams_the_traffic_as_pty_chunks() {
        use base64::Engine as _;

        let live = registry_answering_diagnose("ses_live", snapshot_with_traffic()).await;
        let (events, mut rx) = mpsc::channel(8);
        let (watch_tx, watch_rx) = mpsc::channel(4);
        let shutdown = tokio_util::sync::CancellationToken::new();
        let pump =
            tokio::spawn(pty_view::PtyWatchPump::new(live, events, shutdown.clone()).run(watch_rx));

        watch_tx.send(("ses_live".to_owned(), true)).await.expect("pump accepts watches");
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("no PtyChunk within 5s")
            .expect("pump stopped");

        match event {
            AdapterEvent::PtyChunk { local_id, data } => {
                assert_eq!(local_id, "ses_live");
                let text = String::from_utf8(
                    base64::engine::general_purpose::STANDARD.decode(data).expect("not base64"),
                )
                .expect("not utf8");
                assert!(text.contains("[http] -> POST /session/ses_live/prompt_async"), "{text}");
                assert!(text.contains("[sse] !! GET /event: 502"), "{text}");
                assert!(text.contains("stderr  serve listening"), "{text}");
            }
            other => panic!("expected PtyChunk, got {other:?}"),
        }

        shutdown.cancel();
        pump.await.expect("pump panicked");
    }

    #[tokio::test]
    async fn a_surviving_registry_is_re_announced_on_reconnect() {
        let live = registry_with("ses_live").await;
        let (events, mut rx) = mpsc::channel(8);
        let mut announced = std::collections::HashSet::new();

        announce_live_sessions(&live, &events, &mut announced).await;

        match rx.try_recv().expect("no SessionStarted") {
            AdapterEvent::SessionStarted { local_id, meta } => {
                assert_eq!(local_id, "ses_live");
                assert_eq!(meta.working_dir.as_deref(), Some("/repo"));
            }
            other => panic!("unexpected event {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "a session not in the registry was announced");
    }

    #[tokio::test]
    async fn an_id_the_adapter_no_longer_drives_is_not_announced() {
        let live = registry_with("ses_live").await;
        live.lock().await.remove("ses_live");
        let (events, mut rx) = mpsc::channel(8);
        let mut announced = std::collections::HashSet::new();

        announce_live_sessions(&live, &events, &mut announced).await;

        assert!(rx.try_recv().is_err(), "an ended session was announced as live");
    }

    #[tokio::test]
    async fn a_repeated_sweep_does_not_double_announce() {
        let live = registry_with("ses_live").await;
        let (events, mut rx) = mpsc::channel(8);
        let mut announced = std::collections::HashSet::new();

        announce_live_sessions(&live, &events, &mut announced).await;
        announce_live_sessions(&live, &events, &mut announced).await;

        assert!(rx.try_recv().is_ok(), "the first announce was dropped");
        assert!(rx.try_recv().is_err(), "the session was announced twice");
    }

    /// The `connected` edge, not a server frame, is what drives the sweep, and
    /// every edge must announce again — the server re-applies `daemon_lost` on
    /// each drop.
    #[tokio::test]
    async fn every_connected_edge_re_announces_through_the_pump() {
        use tokio_util::sync::CancellationToken;

        use crate::adapter_runtime::AdapterCtx;

        let live = registry_with("ses_live").await;
        let (events, mut rx) = mpsc::channel(8);
        let (_commands_tx, commands) = mpsc::channel(8);
        let (connect_tx, connected) = tokio::sync::broadcast::channel(8);
        let shutdown = CancellationToken::new();
        let ctx = AdapterCtx {
            events,
            commands,
            pty_watch: None,
            interrupts: None,
            shutdown: shutdown.clone(),
            config: serde_json::Value::Null,
            server: None,
            machine_key: None,
            connected,
        };
        let pump = tokio::spawn(super::pump(
            crate::adapters::opencode::session::OpenCodeConfig::default(),
            ctx,
            live,
        ));

        for connection in 1..=2 {
            connect_tx.send(()).expect("no receiver");
            let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                .await
                .unwrap_or_else(|_| panic!("connection {connection} was never announced"))
                .expect("pump stopped");
            match event {
                AdapterEvent::SessionStarted { local_id, .. } => {
                    assert_eq!(local_id, "ses_live", "connection {connection}");
                }
                other => panic!("unexpected event {other:?}"),
            }
        }

        shutdown.cancel();
        pump.await.expect("pump panicked");
    }

    #[tokio::test]
    async fn the_next_connection_announces_the_session_again() {
        let live = registry_with("ses_live").await;
        let (events, mut rx) = mpsc::channel(8);
        let mut announced = std::collections::HashSet::new();

        announce_live_sessions(&live, &events, &mut announced).await;
        assert!(rx.try_recv().is_ok());

        announced.clear();
        announce_live_sessions(&live, &events, &mut announced).await;

        assert!(rx.try_recv().is_ok(), "a second connection left the session daemon_lost");
    }
}
