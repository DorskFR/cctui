//! One task per ACP session: the agent process, its protocol connection and
//! the turn in flight.
//!
//! Commands arrive on a channel the pump owns; agent traffic arrives on the
//! connection's inbox. A `session/prompt` only answers when the turn ends, so
//! it runs on its own task and the loop stays free for interrupts,
//! permission answers and kills.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use cctui_proto::adapter::{AdapterEvent, EndReason, PermissionMode, SessionMeta};
use cctui_proto::diagnose::{TrafficError, TrafficFrame, TrafficStderrLine};
use serde_json::Value;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::connection::{AcpConnection, Incoming, RpcError};
use super::modes::ModeApply;
use super::normalize::{self, Coalescer, Out};
use super::process::Launch;
use super::protocol::{self, InitInfo, NewSession, PromptOutcome, TurnUsage};
use super::rows::AgentRow;
use crate::adapters::traffic_rings::TrafficRings;

const RPC_TIMEOUT: Duration = Duration::from_mins(1);
/// `session/new` may trigger an interactive login flow on some agents.
const NEW_SESSION_TIMEOUT: Duration = Duration::from_mins(2);
/// A turn has no upper bound of its own; this only catches a dead agent.
const TURN_TIMEOUT: Duration = Duration::from_hours(6);

pub type LiveRegistry = Arc<Mutex<HashMap<String, LiveSession>>>;

pub struct LiveSession {
    pub commands: mpsc::Sender<SessionCommand>,
    pub meta: SessionMeta,
}

#[derive(Debug)]
pub enum SessionCommand {
    Prompt { text: String, command_id: Option<Uuid>, turn_id: Option<Uuid> },
    Interrupt { command_id: Option<Uuid> },
    Kill,
    Permission { request_id: String, allow: bool },
    SetModel { model: Option<String>, effort: Option<String>, command_id: Option<Uuid> },
    Diagnose { reply: mpsc::Sender<Snapshot> },
}

/// What the diagnose report and the live view read off a session.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub agent_pid: Option<u32>,
    pub init: InitInfo,
    pub init_raw: Value,
    pub acp_session_id: Option<String>,
    pub in_flight: bool,
    pub current_mode: Option<String>,
    pub available_modes: Vec<String>,
    pub model: Option<String>,
    pub pending_permissions: Vec<String>,
    pub last_cost_usd: Option<f64>,
    pub protocol_errors: Vec<TrafficError>,
    pub stderr_tail: Vec<TrafficStderrLine>,
    pub rpc_tail: Vec<TrafficFrame>,
}

pub struct SpawnParams {
    pub row: &'static AgentRow,
    pub bin: String,
    /// The server's pre-minted id; the launch key uploads and aliases hang on.
    pub key: String,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub prompt: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `None` leaves the agent on its own default mode.
    pub permission_mode: Option<PermissionMode>,
    pub attachments: Vec<String>,
    pub command_id: Option<Uuid>,
    pub parent_local_id: Option<String>,
    pub preflight: Option<crate::preflight::Preflight>,
    pub context: Vec<cctui_proto::api::SessionContextItem>,
}

struct Turn {
    turn_id: Option<Uuid>,
}

struct Pending {
    options: Vec<protocol::PermissionOption>,
    reply: tokio::sync::oneshot::Sender<Value>,
}

pub struct AcpSession {
    params: SpawnParams,
    events: mpsc::Sender<AdapterEvent>,
    live: LiveRegistry,
    shutdown: CancellationToken,
    rings: Arc<TrafficRings>,
    commands_tx: mpsc::Sender<SessionCommand>,
    commands_rx: mpsc::Receiver<SessionCommand>,
    inbox_rx: mpsc::UnboundedReceiver<Incoming>,
    inbox_tx: mpsc::UnboundedSender<Incoming>,
    connection: Option<AcpConnection>,
    init: InitInfo,
    init_raw: Value,
    session: Option<NewSession>,
    local_id: Option<String>,
    coalescer: Coalescer,
    turn: Option<Turn>,
    turn_task: Option<tokio::task::JoinHandle<anyhow::Result<Value>>>,
    turns: u64,
    queued: VecDeque<(String, Option<Uuid>, Option<Uuid>)>,
    pending: HashMap<String, Pending>,
    current_mode: Option<String>,
    model: Option<String>,
    catalog: super::catalog::Catalog,
    quota: Option<(u64, u64)>,
    context_usage: Option<(u64, Option<f64>)>,
    last_cost_usd: Option<f64>,
    turn_end_supported: bool,
    ending: bool,
}

impl AcpSession {
    #[must_use]
    pub fn new(
        params: SpawnParams,
        events: mpsc::Sender<AdapterEvent>,
        live: LiveRegistry,
        shutdown: CancellationToken,
    ) -> Self {
        let (commands_tx, commands_rx) = mpsc::channel(32);
        let (inbox_tx, inbox_rx) = mpsc::unbounded_channel();
        Self {
            params,
            events,
            live,
            shutdown,
            rings: Arc::new(TrafficRings::default()),
            commands_tx,
            commands_rx,
            inbox_rx,
            inbox_tx,
            connection: None,
            init: InitInfo::default(),
            init_raw: Value::Null,
            session: None,
            local_id: None,
            coalescer: Coalescer::default(),
            turn: None,
            turn_task: None,
            turns: 0,
            queued: VecDeque::new(),
            pending: HashMap::new(),
            current_mode: None,
            model: None,
            catalog: super::catalog::Catalog::default(),
            quota: None,
            context_usage: None,
            last_cost_usd: None,
            turn_end_supported: crate::adapters::turn_end::supported(),
            ending: false,
        }
    }

    pub async fn run(mut self) {
        let command_id = self.params.command_id;
        match self.start().await {
            Ok(()) => {}
            Err(err) => {
                let detail = err.to_string();
                tracing::error!(agent = self.params.row.id, %detail, "acp spawn failed");
                self.teardown().await;
                if let Some(command_id) = command_id {
                    crate::adapters::emit(
                        &self.events,
                        AdapterEvent::CommandResult {
                            command_id,
                            ok: false,
                            error: Some(detail.clone()),
                        },
                    )
                    .await;
                }
                let local_id = self.local_id.clone().unwrap_or_else(|| self.params.key.clone());
                if !local_id.is_empty() {
                    self.end(&local_id, EndReason::SpawnFailed { detail }).await;
                }
                return;
            }
        }
        self.serve().await;
    }

    fn launch(&self) -> Result<Launch, super::modes::Inexpressible> {
        let mut args: Vec<String> = self.params.row.args.iter().map(|a| (*a).to_owned()).collect();
        if let Some(mode) = self.params.permission_mode
            && let ModeApply::LaunchFlag(flag) =
                self.params.row.modes.resolve(self.params.row.id, mode)?
        {
            args.push(flag.to_owned());
        }
        Ok(Launch {
            bin: self.params.bin.clone(),
            args,
            cwd: std::path::PathBuf::from(&self.params.cwd),
            env: self.params.env.clone(),
        })
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        let launch = self.launch()?;
        let connection =
            AcpConnection::open(&launch, Arc::clone(&self.rings), self.inbox_tx.clone()).await?;
        self.connection = Some(connection);

        let init_raw = self
            .request("initialize", protocol::initialize_params(), RPC_TIMEOUT)
            .await
            .map_err(|e| anyhow::anyhow!("initialize: {e}"))?;
        self.init = protocol::parse_initialize(&init_raw);
        self.init_raw = init_raw;
        if self.init.protocol_version != protocol::PROTOCOL_VERSION {
            tracing::warn!(
                agent = self.params.row.id,
                version = self.init.protocol_version,
                "acp agent answered with another protocol version"
            );
        }
        super::report_agent_version(self.params.row.id, self.init.agent_version.as_deref());

        let created = match self
            .request(
                "session/new",
                protocol::new_session_params(std::path::Path::new(&self.params.cwd)),
                NEW_SESSION_TIMEOUT,
            )
            .await
        {
            Ok(v) => v,
            Err(err) => {
                let detail = match err.downcast_ref::<RpcError>() {
                    Some(rpc) if rpc.code == protocol::AUTH_REQUIRED => {
                        protocol::auth_required_detail(
                            self.params.row.bin,
                            machine_name().as_deref(),
                        )
                    }
                    _ => format!("session/new: {err}"),
                };
                anyhow::bail!(detail);
            }
        };
        let session = protocol::parse_new_session(&created)?;
        let session_id = session.session_id.clone();
        self.current_mode = session.modes.as_ref().map(|m| m.current.clone());
        self.catalog = super::catalog::Catalog::from_session(&session);
        self.model = self.catalog.current_model();
        self.session = Some(session);

        self.apply_mode(&session_id).await?;
        if (self.params.model.is_some() || self.params.effort.is_some())
            && let Err(err) = self
                .set_model(&session_id, self.params.model.clone(), self.params.effort.clone())
                .await
        {
            tracing::warn!(%err, "acp: launch model not applied");
        }
        self.register(session_id.clone()).await;
        if let Some(preflight) = self.params.preflight.take() {
            preflight.run_bound(&session_id).await;
        }
        if let Some(first) = self.first_turn(&session_id) {
            self.begin_turn(&session_id, &first, self.params.command_id, None);
        } else if let Some(command_id) = self.params.command_id {
            crate::adapters::emit(
                &self.events,
                AdapterEvent::CommandResult { command_id, ok: true, error: None },
            )
            .await;
        }
        Ok(())
    }

    async fn apply_mode(&mut self, session_id: &str) -> anyhow::Result<()> {
        let Some(mode) = self.params.permission_mode else { return Ok(()) };
        let apply = self.params.row.modes.resolve(self.params.row.id, mode)?;
        match apply {
            ModeApply::SessionMode(id) => {
                self.request(
                    "session/set_mode",
                    protocol::set_mode_params(session_id, id),
                    RPC_TIMEOUT,
                )
                .await
                .map_err(|e| anyhow::anyhow!("could not apply permission mode `{id}`: {e}"))?;
                self.current_mode = Some(id.to_owned());
            }
            ModeApply::ConfigOption { id, value } => {
                self.request(
                    "session/set_config_option",
                    protocol::set_config_option_params(session_id, id, value),
                    RPC_TIMEOUT,
                )
                .await
                .map_err(|e| anyhow::anyhow!("could not apply permission mode `{value}`: {e}"))?;
                self.current_mode = Some(value.to_owned());
            }
            ModeApply::LaunchFlag(_) => {}
        }
        Ok(())
    }

    /// The spawn prompt behind the neutral preamble, delivered even when the
    /// spawn carried no prompt, as the opencode adapter does.
    fn first_turn(&self, local_id: &str) -> Option<String> {
        let prompt = self.params.prompt.clone().unwrap_or_default();
        let files: Vec<&String> = self
            .params
            .attachments
            .iter()
            .filter(|p| !crate::adapters::uploads::is_image_path(p))
            .collect();
        let body = if files.is_empty() {
            prompt.trim().to_owned()
        } else {
            let list: Vec<&str> = files.iter().map(|s| s.as_str()).collect();
            format!("{prompt}\n\nAttached files:\n{}", list.join("\n")).trim().to_owned()
        };
        let preamble = crate::preamble::for_launch(
            Some(self.params.key.as_str()).filter(|k| !k.is_empty()),
            &self.params.cwd,
            Some(local_id),
            &self.params.context,
        );
        crate::preamble::merge(preamble, (!body.is_empty()).then_some(body))
    }

    fn images(&self) -> Vec<protocol::Image> {
        if !self.init.image_prompts {
            return Vec::new();
        }
        self.params
            .attachments
            .iter()
            .filter(|p| crate::adapters::uploads::is_image_path(p))
            .filter_map(|p| image_block(p))
            .collect()
    }

    async fn register(&mut self, local_id: String) {
        if !self.params.key.is_empty() && self.params.key != local_id {
            crate::agenttool::bind_session_alias(&self.params.key, &local_id);
        }
        let started_at_ms = crate::neighbours::now_ms();
        let meta = SessionMeta {
            working_dir: Some(self.params.cwd.clone()),
            parent_local_id: self.params.parent_local_id.clone(),
            extra: serde_json::json!({
                "harness": self.params.row.id,
                "spawn_key": self.params.key,
                "started_at_ms": started_at_ms,
                "agent": self.init.agent_name,
                "agent_version": self.init.agent_version,
            }),
        };
        self.live.lock().await.insert(
            local_id.clone(),
            LiveSession { commands: self.commands_tx.clone(), meta: meta.clone() },
        );
        self.local_id = Some(local_id.clone());
        let _ = self
            .events
            .send(AdapterEvent::SessionStarted { local_id: local_id.clone(), meta })
            .await;
        if let Some(name) = self.params.name.clone() {
            let _ = self.events.send(status(&local_id, None, None, Some(name))).await;
        }
        if let Some(model) = self.model.clone() {
            let _ = self
                .events
                .send(AdapterEvent::SessionModel { local_id: local_id.clone(), model })
                .await;
        }
        if let Some(event) = self.catalog.event(self.params.row.id) {
            let _ = self.events.send(event).await;
        }
        if let Some(mode) = self.current_mode.clone() {
            let posture = normalize::posture(&self.params.row.modes, &mode);
            if let Some(evt) =
                normalize::address(&local_id, Out::Mode { agent_mode: mode, posture })
            {
                let _ = self.events.send(evt).await;
            }
        }
    }

    /// Biased: the inbox drains before a finished turn is handled, so every
    /// update the agent sent ahead of its prompt response is emitted before
    /// the idle status.
    async fn serve(&mut self) {
        loop {
            tokio::select! {
                biased;
                () = self.shutdown.cancelled() => {
                    self.kill(EndReason::Killed).await;
                    return;
                }
                cmd = self.commands_rx.recv() => {
                    let Some(cmd) = cmd else {
                        self.kill(EndReason::Killed).await;
                        return;
                    };
                    if !self.on_command(cmd).await {
                        return;
                    }
                }
                incoming = self.inbox_rx.recv() => {
                    let Some(incoming) = incoming else {
                        self.kill(EndReason::Crashed { detail: "agent inbox closed".to_owned() }).await;
                        return;
                    };
                    if !self.on_incoming(incoming).await {
                        return;
                    }
                }
                done = turn_done(&mut self.turn_task) => {
                    self.turn_task = None;
                    self.on_turn_done(done).await;
                }
            }
        }
    }

    fn local_id(&self) -> String {
        self.local_id.clone().unwrap_or_default()
    }

    /// `false` when the session ended.
    async fn on_command(&mut self, cmd: SessionCommand) -> bool {
        let local_id = self.local_id();
        match cmd {
            SessionCommand::Prompt { text, command_id, turn_id } => {
                if self.turn.is_some() {
                    self.queued.push_back((text, command_id, turn_id));
                } else {
                    self.begin_turn(&local_id, &text, command_id, turn_id);
                }
            }
            SessionCommand::Interrupt { command_id } => {
                let outcome = if self.turn.is_some() {
                    self.notify("session/cancel", protocol::cancel_params(&local_id))
                } else {
                    Ok(())
                };
                if let Some(command_id) = command_id {
                    let error = outcome.err().map(|e| e.to_string());
                    crate::adapters::emit(
                        &self.events,
                        AdapterEvent::CommandResult { command_id, ok: error.is_none(), error },
                    )
                    .await;
                }
            }
            SessionCommand::Kill => {
                self.kill(EndReason::Killed).await;
                return false;
            }
            SessionCommand::Permission { request_id, allow } => {
                self.answer_permission(&request_id, allow).await;
            }
            SessionCommand::SetModel { model, effort, command_id } => {
                let outcome = self.set_model(&local_id, model, effort).await;
                if let Some(command_id) = command_id {
                    let error = outcome.err().map(|e| e.to_string());
                    crate::adapters::emit(
                        &self.events,
                        AdapterEvent::CommandResult { command_id, ok: error.is_none(), error },
                    )
                    .await;
                }
            }
            SessionCommand::Diagnose { reply } => {
                let _ = reply.try_send(self.snapshot());
            }
        }
        true
    }

    async fn on_incoming(&mut self, incoming: Incoming) -> bool {
        let local_id = self.local_id();
        match incoming {
            Incoming::Notification { method, params } => {
                if method == "session/update" {
                    self.on_update(&local_id, &params).await;
                }
            }
            Incoming::Permission { params, reply } => {
                self.on_permission(&local_id, &params, reply).await;
            }
            Incoming::Closed { detail } => {
                if self.ending {
                    return true;
                }
                let reason = EndReason::Crashed {
                    detail: detail.unwrap_or_else(|| "the agent exited".to_owned()),
                };
                self.kill(reason).await;
                return false;
            }
        }
        true
    }

    async fn on_update(&mut self, local_id: &str, params: &Value) {
        let Some(update) = params.get("update") else { return };
        for meta in [params.get("_meta"), update.get("_meta")].into_iter().flatten() {
            if let Some(tokens) = protocol::quota_tokens(meta) {
                self.quota = Some(tokens);
            }
        }
        for out in self.coalescer.update(update) {
            self.emit_out(local_id, out).await;
        }
    }

    async fn emit_out(&mut self, local_id: &str, out: Out) {
        match out {
            Out::Usage { used, cost_usd, .. } => {
                self.context_usage = Some((used, cost_usd));
                if cost_usd.is_some() {
                    self.last_cost_usd = cost_usd;
                }
            }
            Out::ConfigOptions(options) => {
                self.catalog.apply_config_options(&options);
                let model = self.catalog.current_model();
                if model != self.model
                    && let Some(model) = model.clone()
                {
                    let _ = self.events.send(status_model(local_id, model)).await;
                }
                self.model = model;
                if let Some(event) = self.catalog.event(self.params.row.id) {
                    let _ = self.events.send(event).await;
                }
            }
            Out::Mode { agent_mode, .. } => {
                self.current_mode = Some(agent_mode.clone());
                let posture = normalize::posture(&self.params.row.modes, &agent_mode);
                if let Some(evt) = normalize::address(local_id, Out::Mode { agent_mode, posture }) {
                    let _ = self.events.send(evt).await;
                }
            }
            other => {
                if let Some(evt) = normalize::address(local_id, other) {
                    let _ = self.events.send(evt).await;
                }
            }
        }
    }

    async fn on_permission(
        &mut self,
        local_id: &str,
        params: &Value,
        reply: tokio::sync::oneshot::Sender<Value>,
    ) {
        let options = protocol::parse_permission_options(params);
        let tool_call = params.get("toolCall").cloned().unwrap_or(Value::Null);
        let request_id = tool_call
            .get("toolCallId")
            .and_then(Value::as_str)
            .map_or_else(|| Uuid::new_v4().to_string(), str::to_owned);
        if matches!(self.params.permission_mode, Some(PermissionMode::Yolo | PermissionMode::Whip))
        {
            let _ = reply.send(protocol::permission_response(&options, true));
            return;
        }
        self.pending.insert(request_id.clone(), Pending { options, reply });
        let _ = self
            .events
            .send(normalize::permission_request(local_id, &request_id, &tool_call))
            .await;
    }

    async fn answer_permission(&mut self, request_id: &str, allow: bool) {
        let local_id = self.local_id();
        let Some(pending) = self.pending.remove(request_id) else {
            tracing::warn!(%request_id, "acp: no pending permission request");
            return;
        };
        let _ = pending.reply.send(protocol::permission_response(&pending.options, allow));
        let _ = self
            .events
            .send(AdapterEvent::PermissionResolved { local_id, request_id: request_id.to_owned() })
            .await;
    }

    fn begin_turn(
        &mut self,
        local_id: &str,
        text: &str,
        command_id: Option<Uuid>,
        turn_id: Option<Uuid>,
    ) {
        let Some(cx) = self.connection.as_ref().map(AcpConnection::requester) else { return };
        self.turns += 1;
        let images = if self.turns == 1 { self.images() } else { Vec::new() };
        let params = protocol::prompt_params(local_id, text, &images);
        self.turn = Some(Turn { turn_id });
        self.quota = None;
        self.context_usage = None;
        let events = self.events.clone();
        let id = local_id.to_owned();
        self.turn_task = Some(tokio::spawn(async move {
            let _ = events.send(status(&id, Some("active".to_owned()), None, None)).await;
            cx.request("session/prompt", params, TURN_TIMEOUT).await
        }));
        if let Some(command_id) = command_id {
            let events = self.events.clone();
            tokio::spawn(async move {
                crate::adapters::emit(
                    &events,
                    AdapterEvent::CommandResult { command_id, ok: true, error: None },
                )
                .await;
            });
        }
    }

    async fn on_turn_done(&mut self, done: Result<anyhow::Result<Value>, tokio::task::JoinError>) {
        let local_id = self.local_id();
        let Some(turn) = self.turn.take() else { return };
        let outcome = match done {
            Ok(Ok(resp)) => Ok(protocol::parse_prompt(&resp)),
            Ok(Err(err)) => Err(err.to_string()),
            Err(err) => Err(format!("turn task failed: {err}")),
        };
        for out in self.coalescer.flush() {
            self.emit_out(&local_id, out).await;
        }
        match outcome {
            Ok(PromptOutcome { stop_reason, usage }) => {
                self.emit_usage(&local_id, usage).await;
                let detail = normalize::stop_detail(&stop_reason);
                let _ = self
                    .events
                    .send(status(&local_id, Some("idle".to_owned()), detail, None))
                    .await;
            }
            Err(detail) => {
                if !self.ending {
                    let text = format!("prompt failed: {detail}");
                    let _ = self
                        .events
                        .send(AdapterEvent::Message {
                            local_id: local_id.clone(),
                            payload: serde_json::json!({
                                "type": "text", "content": text, "role": "assistant", "text": text, "meta": true,
                            }),
                            turn_id: turn.turn_id,
                        })
                        .await;
                    let _ = self
                        .events
                        .send(status(&local_id, Some("error".to_owned()), Some(text), None))
                        .await;
                }
            }
        }
        crate::adapters::turn_end::emit_gated(&self.events, &local_id, self.turn_end_supported)
            .await;
        if let Some((text, command_id, turn_id)) = self.queued.pop_front() {
            self.begin_turn(&local_id, &text, command_id, turn_id);
        }
    }

    async fn emit_usage(&mut self, local_id: &str, usage: Option<TurnUsage>) {
        let message_id = format!("{local_id}-turn-{}", self.turns);
        let event = if let Some(u) = usage {
            AdapterEvent::TokenUsage {
                local_id: local_id.to_owned(),
                message_id,
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                cache_read_tokens: u.cached_read_tokens,
                cache_creation_tokens: u.cached_write_tokens,
            }
        } else if let Some((input, output)) = self.quota.take() {
            AdapterEvent::TokenUsage {
                local_id: local_id.to_owned(),
                message_id,
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            }
        } else if let Some((used, _)) = self.context_usage.take().filter(|(used, _)| *used > 0) {
            AdapterEvent::TokenUsage {
                local_id: local_id.to_owned(),
                message_id,
                input_tokens: used,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            }
        } else {
            return;
        };
        let _ = self.events.send(event).await;
    }

    /// `set_config_option` when the agent exposes the option, legacy
    /// `set_model` otherwise; the answer's config options refresh the catalog.
    async fn set_model(
        &mut self,
        session_id: &str,
        model: Option<String>,
        effort: Option<String>,
    ) -> anyhow::Result<()> {
        let requests =
            self.catalog.set_model_requests(session_id, model.as_deref(), effort.as_deref())?;
        for (method, params) in requests {
            let resp = self.request(&method, params, RPC_TIMEOUT).await?;
            if let Some(options) = resp.get("configOptions").and_then(Value::as_array) {
                self.catalog.apply_config_options(options);
            }
        }
        if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
            self.catalog.note_legacy_model(&model);
        }
        let model = self.catalog.current_model();
        if self.local_id.is_some() {
            if model != self.model
                && let Some(model) = model.clone()
            {
                let _ = self.events.send(status_model(session_id, model)).await;
            }
            if let Some(event) = self.catalog.event(self.params.row.id) {
                let _ = self.events.send(event).await;
            }
        }
        self.model = model;
        Ok(())
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let connection =
            self.connection.as_ref().ok_or_else(|| anyhow::anyhow!("no connection"))?;
        connection.request(method, params, timeout).await
    }

    fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        let connection =
            self.connection.as_ref().ok_or_else(|| anyhow::anyhow!("no connection"))?;
        connection.notify(method, params)
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            agent_pid: self.connection.as_ref().and_then(|c| c.child.id()),
            init: self.init.clone(),
            init_raw: self.init_raw.clone(),
            acp_session_id: self.session.as_ref().map(|s| s.session_id.clone()),
            in_flight: self.turn.is_some(),
            current_mode: self.current_mode.clone(),
            available_modes: self
                .session
                .as_ref()
                .and_then(|s| s.modes.as_ref())
                .map(|m| m.available.iter().map(|c| c.id.clone()).collect())
                .unwrap_or_default(),
            model: self.model.clone(),
            pending_permissions: self.pending.keys().cloned().collect(),
            last_cost_usd: self.last_cost_usd,
            protocol_errors: self.rings.protocol_errors(),
            stderr_tail: self.rings.stderr_tail(),
            rpc_tail: self.rings.rpc_tail(),
        }
    }

    /// Cancel the turn, close the session when the agent can, stop the
    /// protocol and take the process group down.
    async fn teardown(&mut self) {
        self.ending = true;
        if let Some(task) = self.turn_task.take() {
            if let Some(session_id) = self.session.as_ref().map(|s| s.session_id.clone()) {
                let _ = self.notify("session/cancel", protocol::cancel_params(&session_id));
            }
            task.abort();
        }
        for (_, pending) in self.pending.drain() {
            drop(pending.reply);
        }
        let Some(mut connection) = self.connection.take() else { return };
        if self.init.can_close
            && let Some(session_id) = self.session.as_ref().map(|s| s.session_id.clone())
        {
            let _ = connection
                .request(
                    "session/close",
                    protocol::close_params(&session_id),
                    Duration::from_secs(5),
                )
                .await;
        }
        connection.close();
        super::process::shutdown(&mut connection.child).await;
    }

    async fn kill(&mut self, reason: EndReason) {
        let local_id = self.local_id();
        self.turn = None;
        self.teardown().await;
        if !local_id.is_empty() {
            self.end(&local_id, reason).await;
        }
    }

    async fn end(&self, local_id: &str, reason: EndReason) {
        self.live.lock().await.remove(local_id);
        if !self.params.key.is_empty() {
            crate::adapters::uploads::remove_session_dir(&self.params.key);
        }
        let _ = self
            .events
            .send(AdapterEvent::SessionEnded { local_id: local_id.to_owned(), reason })
            .await;
    }
}

/// Resolves with the turn's outcome, or never while no turn runs.
async fn turn_done(
    task: &mut Option<tokio::task::JoinHandle<anyhow::Result<Value>>>,
) -> Result<anyhow::Result<Value>, tokio::task::JoinError> {
    match task {
        Some(task) => task.await,
        None => std::future::pending().await,
    }
}

fn machine_name() -> Option<String> {
    std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()).or_else(|| {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|h| h.trim().to_owned())
            .filter(|h| !h.is_empty())
    })
}

fn image_block(path: &str) -> Option<protocol::Image> {
    use base64::Engine as _;
    let bytes = std::fs::read(path).ok()?;
    let ext = std::path::Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    let mime_type = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        _ => return None,
    };
    Some(protocol::Image {
        data_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime_type: mime_type.to_owned(),
    })
}

fn status(
    local_id: &str,
    tempo: Option<String>,
    detail: Option<String>,
    name: Option<String>,
) -> AdapterEvent {
    AdapterEvent::Status {
        local_id: local_id.to_owned(),
        state: tempo.clone(),
        tempo,
        detail,
        activity: None,
        name,
        intent: None,
        model: None,
        effort: None,
        permission_mode: None,
        children: Vec::new(),
    }
}

fn status_model(local_id: &str, model: String) -> AdapterEvent {
    AdapterEvent::Status {
        local_id: local_id.to_owned(),
        tempo: None,
        state: None,
        detail: None,
        activity: None,
        name: None,
        intent: None,
        model: Some(model),
        effort: None,
        permission_mode: None,
        children: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mirrors_tempo_into_state() {
        let AdapterEvent::Status { tempo, state, name, .. } =
            status("s", Some("idle".to_owned()), None, Some("n".to_owned()))
        else {
            panic!()
        };
        assert_eq!(tempo.as_deref(), Some("idle"));
        assert_eq!(state.as_deref(), Some("idle"));
        assert_eq!(name.as_deref(), Some("n"));
    }

    #[test]
    fn image_blocks_carry_the_mime_type_of_the_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let png = tmp.path().join("shot.PNG");
        std::fs::write(&png, b"\x89PNG").unwrap();
        let block = image_block(png.to_str().unwrap()).unwrap();
        assert_eq!(block.mime_type, "image/png");
        assert_eq!(block.data_b64, "iVBORw==");
        let txt = tmp.path().join("notes.txt");
        std::fs::write(&txt, b"hi").unwrap();
        assert!(image_block(txt.to_str().unwrap()).is_none());
        assert!(image_block(tmp.path().join("missing.png").to_str().unwrap()).is_none());
    }
}
