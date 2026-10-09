//! ACP adapter: drives any Agent Client Protocol agent (gemini, qwen, goose,
//! …) over stdio, one agent process per cctui session.
//!
//! One `AcpAdapter` type, one factory per [`rows::AgentRow`] the harness
//! table lists. The agent-specific part of a row is its launch command, its
//! permission-mode table and whether it still speaks the legacy `models`
//! pair; everything else is shared.

pub mod catalog;
pub mod connection;
pub mod elicitation;
pub mod modes;
pub mod normalize;
pub mod persist;
pub mod process;
pub mod protocol;
mod pty_view;
pub mod rows;
pub mod session;

use cctui_proto::adapter::{AdapterEvent, EndReason, SessionSpec};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::adapter_runtime::{
    Adapter, AdapterCtx, AdapterFactory, CommandOutcome, Handled, SessionDriver, dispatch_command,
};
use crate::client::ServerClient;
use persist::SessionStore;
use rows::AgentRow;
use session::{AcpSession, LiveRegistry, Resume, SessionCommand, SpawnParams};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Declarative config: `{ "bin": "/path/to/agent" }` overrides the row's binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpConfig {
    pub bin: Option<String>,
}

impl AcpConfig {
    #[must_use]
    pub fn from_value(v: &serde_json::Value) -> Self {
        Self {
            bin: v
                .get("bin")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .map(str::to_owned),
        }
    }
}

pub struct AcpFactory {
    row: &'static AgentRow,
}

impl AcpFactory {
    #[must_use]
    pub const fn new(row: &'static AgentRow) -> Self {
        Self { row }
    }
}

impl AdapterFactory for AcpFactory {
    fn id(&self) -> &'static str {
        self.row.id
    }

    fn build(&self, _config: serde_json::Value) -> Box<dyn Adapter> {
        Box::new(AcpAdapter::new(self.row))
    }

    fn pty_watch(&self, _config: &serde_json::Value) -> bool {
        true
    }
}

/// One factory per agent row the harness table knows.
#[must_use]
pub fn factories() -> Vec<Box<dyn AdapterFactory>> {
    rows::registered()
        .into_iter()
        .map(|row| Box::new(AcpFactory::new(row)) as Box<dyn AdapterFactory>)
        .collect()
}

pub struct AcpAdapter {
    pub row: &'static AgentRow,
    store: Arc<SessionStore>,
    reexec: CancellationToken,
}

impl AcpAdapter {
    #[must_use]
    pub fn new(row: &'static AgentRow) -> Self {
        Self { row, store: persist::global(), reexec: crate::selfupdate::reexec_prep() }
    }

    #[must_use]
    pub fn with_store(mut self, store: Arc<SessionStore>) -> Self {
        self.store = store;
        self
    }

    #[must_use]
    pub fn with_reexec(mut self, reexec: CancellationToken) -> Self {
        self.reexec = reexec;
        self
    }
}

#[async_trait::async_trait]
impl Adapter for AcpAdapter {
    fn id(&self) -> &'static str {
        self.row.id
    }

    async fn start(&self, ctx: AdapterCtx) -> anyhow::Result<()> {
        let cfg = AcpConfig::from_value(&ctx.config);
        let bin = cfg.bin.clone().unwrap_or_else(|| self.row.bin.to_owned());
        tracing::info!(agent = self.row.id, %bin, "acp adapter ready");
        pump(self.row, bin, ctx, Arc::clone(&self.store), self.reexec.clone()).await;
        Ok(())
    }
}

async fn pump(
    row: &'static AgentRow,
    bin: String,
    ctx: AdapterCtx,
    store: Arc<SessionStore>,
    reexec: CancellationToken,
) {
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
    let live: LiveRegistry = LiveRegistry::default();
    let _watch_handle = pty_watch.map(|watches| {
        let pump = pty_view::PtyWatchPump::new(live.clone(), events.clone(), shutdown.clone());
        tokio::spawn(pump.run(watches))
    });
    let mut pump = Pump { row, bin, events, shutdown, server, machine_key, live, store, reexec };
    pump.restore_sessions().await;
    let mut connect_closed = false;
    loop {
        tokio::select! {
            () = pump.shutdown.cancelled() => return,
            edge = connected.recv(), if !connect_closed => {
                if matches!(edge, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                    connect_closed = true;
                    continue;
                }
                pump.announce_live_sessions().await;
            }
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { return };
                let events = pump.events.clone();
                dispatch_command(&mut pump, &events, cmd).await;
            }
        }
    }
}

struct Pump {
    row: &'static AgentRow,
    bin: String,
    events: mpsc::Sender<AdapterEvent>,
    shutdown: tokio_util::sync::CancellationToken,
    server: Option<ServerClient>,
    machine_key: Option<String>,
    live: LiveRegistry,
    store: Arc<SessionStore>,
    reexec: CancellationToken,
}

#[async_trait::async_trait]
impl SessionDriver for Pump {
    fn adapter_id(&self) -> &'static str {
        self.row.id
    }

    async fn spawn(
        &mut self,
        spec: SessionSpec,
        command_id: Option<Uuid>,
        session_id: Option<Uuid>,
    ) -> CommandOutcome {
        self.start_session(spec, command_id, session_id).await;
        Ok(Handled::Deferred)
    }

    async fn send_message(&mut self, local_id: String, text: String) -> CommandOutcome {
        self.prompt(local_id, text, None, None, None).await;
        Ok(Handled::Deferred)
    }

    async fn reply(
        &mut self,
        local_id: String,
        text: String,
        ask_picks: Option<Vec<Vec<usize>>>,
        _env: std::collections::BTreeMap<String, String>,
        command_id: Option<Uuid>,
        turn_id: Option<Uuid>,
    ) -> CommandOutcome {
        self.prompt(local_id, text, ask_picks, command_id, turn_id).await;
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

    async fn resume(
        &mut self,
        local_id: String,
        working_dir: Option<String>,
        _env: std::collections::BTreeMap<String, String>,
    ) -> CommandOutcome {
        if self.live.lock().await.contains_key(&local_id) {
            return Ok(Handled::Done);
        }
        let record = match (self.store.get(&local_id), working_dir) {
            (Some(record), _) => record,
            (None, Some(cwd)) => persist::Record {
                adapter: self.row.id.to_owned(),
                key: String::new(),
                cwd,
                model: None,
                effort: None,
                permission_mode: None,
                parent_local_id: None,
                started_at_ms: crate::neighbours::now_ms(),
            },
            (None, None) => {
                return Err(anyhow::anyhow!("no recorded {} session {local_id}", self.row.id));
            }
        };
        self.restore(local_id, record).await;
        Ok(Handled::Done)
    }

    async fn interrupt(&mut self, local_id: String, command_id: Option<Uuid>) -> CommandOutcome {
        let delivered =
            route(&self.live, &local_id, SessionCommand::Interrupt { command_id }).await;
        if delivered {
            return Ok(Handled::Deferred);
        }
        Err(anyhow::anyhow!("no live {} session", self.row.id))
    }

    async fn permission_response(
        &mut self,
        local_id: String,
        request_id: String,
        allow: bool,
    ) -> CommandOutcome {
        self.permission_answer(local_id, request_id, allow, None).await
    }

    async fn permission_answer(
        &mut self,
        local_id: String,
        request_id: String,
        allow: bool,
        option_id: Option<String>,
    ) -> CommandOutcome {
        let cmd = SessionCommand::Permission { request_id, allow, option_id };
        route(&self.live, &local_id, cmd).await;
        Ok(Handled::Deferred)
    }

    async fn set_model(
        &mut self,
        local_id: String,
        model: Option<String>,
        effort: Option<String>,
        command_id: Option<Uuid>,
    ) -> CommandOutcome {
        let delivered =
            route(&self.live, &local_id, SessionCommand::SetModel { model, effort, command_id })
                .await;
        if delivered {
            return Ok(Handled::Deferred);
        }
        Err(anyhow::anyhow!("no live {} session", self.row.id))
    }

    async fn diagnose(&mut self, local_id: String, request_id: Uuid) -> CommandOutcome {
        let report = diagnose(
            self.row,
            &self.live,
            &local_id,
            self.server.as_ref(),
            self.machine_key.as_ref(),
        )
        .await;
        let _ = self
            .events
            .send(AdapterEvent::Diagnose { local_id, request_id, report: Box::new(report) })
            .await;
        Ok(Handled::Deferred)
    }
}

impl Pump {
    async fn prompt(
        &self,
        local_id: String,
        text: String,
        ask_picks: Option<Vec<Vec<usize>>>,
        command_id: Option<Uuid>,
        turn_id: Option<Uuid>,
    ) {
        let cmd = SessionCommand::Prompt { text, ask_picks, command_id, turn_id };
        let delivered = route(&self.live, &local_id, cmd).await;
        if !delivered {
            fail(&self.events, command_id, &format!("no live {} session", self.row.id)).await;
        }
    }

    async fn kill_session(&self, local_id: String) {
        if !route(&self.live, &local_id, SessionCommand::Kill).await {
            crate::adapters::emit(
                &self.events,
                AdapterEvent::SessionEnded { local_id, reason: EndReason::Killed },
            )
            .await;
        }
    }

    async fn announce_live_sessions(&self) {
        let snapshot: Vec<(String, cctui_proto::adapter::SessionMeta)> =
            self.live.lock().await.iter().map(|(id, s)| (id.clone(), s.meta.clone())).collect();
        for (local_id, meta) in snapshot {
            let _ = self.events.send(AdapterEvent::SessionStarted { local_id, meta }).await;
        }
    }

    async fn start_session(
        &self,
        spec: SessionSpec,
        command_id: Option<Uuid>,
        session_id: Option<Uuid>,
    ) {
        let Some(working_dir) = spec.working_dir.clone() else {
            fail(&self.events, command_id, "working_dir required").await;
            return;
        };
        if let Some(mode) = spec.permission_mode
            && let Err(refused) = self.row.modes.resolve(self.row.id, mode)
        {
            fail(&self.events, command_id, &refused.to_string()).await;
            return;
        }
        let key = session_id.or(command_id).map_or_else(String::new, |id| id.to_string());
        let launch = match crate::adapters::gateway_env::resolve_launch(
            self.row.id,
            self.server.as_ref(),
            self.machine_key.as_ref(),
            &key,
            &spec.env,
            &[],
        )
        .await
        {
            Ok(launch) => launch,
            Err(err) => {
                fail(&self.events, command_id, &err.to_string()).await;
                return;
            }
        };
        let attachments = match crate::adapters::uploads::stage_bootstrap(&key, &spec.bootstrap) {
            Ok(paths) => paths,
            Err(err) => {
                fail(&self.events, command_id, &format!("attachment staging failed: {err}")).await;
                return;
            }
        };
        let preflight = crate::preflight::Preflight::new(self.events.clone(), spec.model.clone())
            .with_limits(self.server.as_ref(), self.machine_key.as_deref());
        let params = SpawnParams {
            row: self.row,
            bin: self.bin.clone(),
            key,
            cwd: working_dir,
            env: launch.env,
            prompt: spec.prompt.clone(),
            name: spec.name.clone(),
            model: spec.model.clone(),
            effort: spec.effort.clone(),
            permission_mode: spec.permission_mode,
            attachments,
            command_id,
            parent_local_id: spec.parent_local_id.clone(),
            preflight: Some(preflight),
            context: launch.context,
            resume: None,
        };
        self.run_session(params);
    }

    fn run_session(&self, params: SpawnParams) {
        let session =
            AcpSession::new(params, self.events.clone(), self.live.clone(), self.shutdown.clone())
                .with_store(Arc::clone(&self.store))
                .with_reexec(self.reexec.clone());
        tokio::spawn(session.run());
    }

    async fn restore_sessions(&self) {
        for (local_id, record) in self.store.for_adapter(self.row.id) {
            self.restore(local_id, record).await;
        }
    }

    /// The account is bound to the agent's session id once it starts, so the
    /// launch env is pulled by that id rather than the spawn key.
    async fn restore(&self, local_id: String, record: persist::Record) {
        let launch = match crate::adapters::gateway_env::resolve_launch(
            self.row.id,
            self.server.as_ref(),
            self.machine_key.as_ref(),
            &local_id,
            &std::collections::BTreeMap::new(),
            &[],
        )
        .await
        {
            Ok(launch) => launch,
            Err(err) => {
                self.store.remove(&local_id);
                crate::adapters::emit(
                    &self.events,
                    AdapterEvent::SessionEnded {
                        local_id,
                        reason: EndReason::ResumeFailed {
                            detail: format!("launch environment unavailable: {err}"),
                        },
                    },
                )
                .await;
                return;
            }
        };
        tracing::info!(%local_id, agent = self.row.id, "acp: re-attaching session");
        self.run_session(SpawnParams {
            row: self.row,
            bin: self.bin.clone(),
            key: record.key,
            cwd: record.cwd,
            env: launch.env,
            prompt: None,
            name: None,
            model: record.model,
            effort: record.effort,
            permission_mode: record.permission_mode,
            attachments: Vec::new(),
            command_id: None,
            parent_local_id: record.parent_local_id,
            preflight: None,
            context: Vec::new(),
            resume: Some(Resume { session_id: local_id, started_at_ms: record.started_at_ms }),
        });
    }
}

async fn route(live: &LiveRegistry, local_id: &str, cmd: SessionCommand) -> bool {
    let Some(tx) = live.lock().await.get(local_id).map(|s| s.commands.clone()) else {
        tracing::warn!(%local_id, "acp: no live session for command");
        return false;
    };
    tx.send(cmd).await.is_ok()
}

async fn fail(events: &mpsc::Sender<AdapterEvent>, command_id: Option<Uuid>, error: &str) {
    tracing::error!(%error, "acp command failed");
    if let Some(command_id) = command_id {
        crate::adapters::emit(
            events,
            AdapterEvent::CommandResult { command_id, ok: false, error: Some(error.to_owned()) },
        )
        .await;
    }
}

/// The agent version `initialize` reported, recorded under the adapter id
/// for the machine's harness report.
fn report_agent_version(adapter_id: &str, version: Option<&str>) {
    let Some(version) = version.map(str::trim).filter(|v| !v.is_empty()) else { return };
    tracing::info!(agent = adapter_id, %version, "acp agent version");
    crate::harness_update::record_version(
        adapter_id,
        cctui_proto::harness::HarnessVersion { cli: Some(version.to_owned()), daemon: None },
    );
}

/// Ask the live session for its snapshot; `None` when it does not answer
/// within the poll window.
pub(super) async fn session_snapshot(
    live: &LiveRegistry,
    local_id: &str,
) -> Option<session::Snapshot> {
    let tx = live.lock().await.get(local_id).map(|s| s.commands.clone())?;
    let (reply, mut rx) = mpsc::channel(1);
    tx.send(SessionCommand::Diagnose { reply }).await.ok()?;
    tokio::time::timeout(crate::adapters::ring_view::POLL_INTERVAL, rx.recv()).await.ok().flatten()
}

async fn diagnose(
    row: &'static AgentRow,
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
    let acp = acp_section(row, snapshot.as_ref(), live_present);

    SessionDiagnose {
        local_id: local_id.to_owned(),
        short: None,
        generated_at_ms: now_ms,
        adapter: row.id.to_owned(),
        effective_state: DiagnoseFact::fresh(
            EffectiveState {
                verdict: verdict.to_owned(),
                tempo: None,
                state: Some(verdict.to_owned()),
                detail: None,
                activity: None,
            },
            "acp-adapter",
            now_ms,
        ),
        last_hook_event: na(),
        attach: na(),
        pty_output: na(),
        claude_socket: na(),
        transcript: na(),
        prompts: na(),
        permission_mode: acp
            .current_mode
            .clone()
            .map_or_else(na, |m| DiagnoseFact::fresh(m, "acp-adapter", now_ms)),
        dispatch: na(),
        gateway: DiagnoseFact::fresh(
            GatewayStatus { server_configured: server.is_some() && machine_key.is_some() },
            "daemon-config",
            now_ms,
        ),
        codex: None,
        opencode: None,
        acp: Some(acp),
    }
}

fn na<T>() -> cctui_proto::diagnose::DiagnoseFact<T> {
    cctui_proto::diagnose::DiagnoseFact::missing("acp-adapter", "not applicable to an ACP session")
}

fn acp_section(
    row: &AgentRow,
    snapshot: Option<&session::Snapshot>,
    live_present: bool,
) -> cctui_proto::diagnose::AcpDiagnose {
    let Some(snap) = snapshot else {
        return cctui_proto::diagnose::AcpDiagnose {
            agent: row.id.to_owned(),
            agent_name: None,
            agent_version: None,
            protocol_version: None,
            agent_pid: None,
            live: live_present,
            acp_session_id: None,
            turn_status: "unknown".to_owned(),
            current_mode: None,
            available_modes: Vec::new(),
            model: None,
            pending_permissions: Vec::new(),
            last_cost_usd: None,
            initialize: serde_json::Value::Null,
            protocol_errors: Vec::new(),
            stderr_tail: Vec::new(),
            rpc_tail: Vec::new(),
        };
    };
    cctui_proto::diagnose::AcpDiagnose {
        agent: row.id.to_owned(),
        agent_name: snap.init.agent_name.clone(),
        agent_version: snap.init.agent_version.clone(),
        protocol_version: Some(snap.init.protocol_version),
        agent_pid: snap.agent_pid,
        live: live_present,
        acp_session_id: snap.acp_session_id.clone(),
        turn_status: if snap.in_flight { "working" } else { "idle" }.to_owned(),
        current_mode: snap.current_mode.clone(),
        available_modes: snap.available_modes.clone(),
        model: snap.model.clone(),
        pending_permissions: snap.pending_permissions.clone(),
        last_cost_usd: snap.last_cost_usd,
        initialize: snap.init_raw.clone(),
        protocol_errors: snap.protocol_errors.clone(),
        stderr_tail: snap.stderr_tail.clone(),
        rpc_tail: snap.rpc_tail.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_bin_overrides_the_row_binary_when_set() {
        assert_eq!(
            AcpConfig::from_value(&serde_json::json!({ "bin": " /opt/gemini " })).bin.as_deref(),
            Some("/opt/gemini")
        );
        assert_eq!(AcpConfig::from_value(&serde_json::json!({ "bin": "" })).bin, None);
        assert_eq!(AcpConfig::from_value(&serde_json::Value::Null).bin, None);
    }

    #[test]
    fn factories_cover_exactly_the_rows_the_harness_table_lists() {
        let ids: Vec<&str> = factories().iter().map(|f| f.id()).collect();
        let expected: Vec<&str> = rows::registered().iter().map(|r| r.id).collect();
        assert_eq!(ids, expected);
        for f in factories() {
            assert!(f.pty_watch(&serde_json::Value::Null), "{}", f.id());
            assert!(!cctui_proto::adapter::is_default_enabled(f.id()), "{}", f.id());
        }
    }

    #[test]
    fn the_diagnose_section_without_a_live_session_is_honest() {
        let section = acp_section(&rows::GEMINI, None, false);
        assert_eq!(section.agent, "gemini");
        assert!(!section.live);
        assert_eq!(section.turn_status, "unknown");
        assert!(section.agent_version.is_none());
    }
}
