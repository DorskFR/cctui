use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cctui_proto::adapter::{AdapterEvent, EndReason, SessionMeta};
use cctui_proto::codex_catalog::CodexModelCatalog;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::config::launch_overrides;
use super::diagnose::{DiagnoseRings, stderr_tail};
use super::registry::{CodexLiveSnapshot, SessionCommand, SessionRecord};
use super::requests::{
    ThreadConfig, ThreadInfo, initialize_req, initialized_notification, record_codex_version,
    thread_info, turn_interrupt_req, turn_start_req, turn_steer_req,
};
use super::rpc::{
    HANDSHAKE_TIMEOUT, ID_INITIALIZE, ID_THREAD_START, Incoming, NO_TURN_IN_FLIGHT, PendingRpc,
    PendingRpcs, RPC_TIMEOUT, RUN_BASE, RpcSink, RpcSource, RpcStdin, approval_reply, classify,
    user_input_reply,
};
use super::session::{
    CodexSession, SIGTERM, SessionLaunch, SpawnAck, child_linkage, kill_child, partition_drained,
    record_model_override, removes_record, set_thread_name, spawn_resumed_session,
    subagent_started_event, terminate_child, unknown_model,
};
use super::thread_state::{
    PromptDispatch, SteerRecovery, ThreadState, TurnLifecycle, prompt_dispatch, steer_recovery,
    turn_lifecycle,
};
use crate::adapters::codex::model_list;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    Break,
}

/// Why the pump stopped on purpose; `None` means the app-server went away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// Daemon shutdown or a Kill command: the epilogue reports `Killed`
    /// rather than treating the non-zero exit as a crash.
    Killed,
    Reexec,
    HandshakeFailed,
}

struct Transport {
    child: Option<Child>,
    stdin: RpcStdin,
    lines: RpcSource,
    stderr_drain: Option<JoinHandle<()>>,
    thread_config: ThreadConfig,
}

impl CodexSession {
    pub(super) async fn run_inner(
        &self,
        ack: &mut SpawnAck,
        rings: &Arc<DiagnoseRings>,
    ) -> Result<()> {
        let cwd_path = std::path::Path::new(&self.cwd);
        if !cwd_path.is_dir() {
            anyhow::bail!("spawn: working_dir does not exist or is not a directory: {}", self.cwd);
        }
        let Transport { child, stdin, mut lines, stderr_drain, thread_config } =
            self.open_transport(cwd_path, rings).await?;

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<SessionCommand>(32);
        let mut pump = EventLoop::new(self, ack, rings, child, stdin, thread_config, cmd_tx);
        pump.send_initialize().await;
        let mut sweep = tokio::time::interval(Duration::from_secs(1));
        sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let reexec = crate::selfupdate::reexec_prep();

        loop {
            let flow = tokio::select! {
                () = self.shutdown.cancelled() => pump.on_shutdown(),
                () = reexec.cancelled() => pump.on_reexec(),
                _ = sweep.tick() => pump.on_sweep().await,
                cmd = cmd_rx.recv(), if pump.registered => pump.on_command(cmd).await,
                line = lines.next_line() => pump.on_line(line).await?,
            };
            if flow == Flow::Break {
                break;
            }
        }
        pump.finish(cmd_rx, stderr_drain).await
    }

    async fn open_transport(
        &self,
        cwd_path: &std::path::Path,
        rings: &Arc<DiagnoseRings>,
    ) -> Result<Transport> {
        if let Some((wire, config)) = self.open_shared().await {
            let stdin = RpcStdin { inner: RpcSink::Shared(wire.sink), rings: rings.clone() };
            return Ok(Transport {
                child: None,
                stdin,
                lines: RpcSource::Shared(wire.frames),
                stderr_drain: None,
                thread_config: config,
            });
        }
        let mut cmd = Command::new(&self.cfg.bin);
        cmd.arg("app-server");
        for (key, value) in launch_overrides(&self.cfg, &self.env) {
            cmd.arg("-c").arg(format!("{key}={value}"));
        }
        // Already TOML literals (quoted scalar / array), unlike the scalar knobs
        // above which are quoted here.
        if let Some(agent_mcp) = &self.agent_mcp {
            for (key, value) in agent_mcp.codex_config_overrides() {
                cmd.arg("-c").arg(format!("{key}={value}"));
            }
        }
        // Forward the resolved launch env — chiefly the gateway
        // credential pulled from the server's `sessions.account_id` binding —
        // onto the app-server child, so a session bound to a named gateway
        // account routes through it instead of hitting the default upstream and
        // 401ing. Applied before `PATH` below so the launchd PATH fix wins even
        // if the resolved env carried a `PATH` of its own. The fail-closed
        // contract (refuse an account-bound launch with empty gateway env) is
        // enforced upstream in the adapter command pump;.
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        crate::childenv::ScrubChildEnv::scrub_child_env(&mut cmd);
        let mut spawned = cmd
            .current_dir(cwd_path)
            // launchd strips `PATH` down to a minimal set that omits
            // `/opt/homebrew/bin`, so a bare `codex` fails ENOENT.
            .env("PATH", crate::childenv::child_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Capture stderr (the app-server's log stream) rather than
            // discarding it — it is the only diagnostic when codex dies
            // unexpectedly (CCT macOS "randomly dies" report).
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn `{} app-server`", self.cfg.bin))?;

        let stdin = RpcStdin {
            inner: RpcSink::Stdio(spawned.stdin.take().context("child stdin missing")?),
            rings: rings.clone(),
        };
        let stdout = spawned.stdout.take().context("child stdout missing")?;
        let lines = RpcSource::Stdio(BufReader::new(stdout).lines());

        // Drain stderr into the bounded ring in the background. Each line
        // is also logged at info under its own target; the retained tail is
        // surfaced in every failure detail (handshake and crash).
        let stderr_drain = spawned.stderr.take().map(|stderr| {
            let rings = rings.clone();
            tokio::spawn(async move {
                let mut err_lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = err_lines.next_line().await {
                    tracing::info!(target: "codex_app_server_stderr", "{line}");
                    rings.note_stderr(&line);
                }
            })
        });
        Ok(Transport {
            child: Some(spawned),
            stdin,
            lines,
            stderr_drain,
            thread_config: self.thread_config(false),
        })
    }
}

struct EventLoop<'a> {
    session: &'a CodexSession,
    ack: &'a mut SpawnAck,
    rings: &'a Arc<DiagnoseRings>,
    child: Option<Child>,
    stdin: RpcStdin,
    thread_config: ThreadConfig,
    pending_rpcs: PendingRpcs,
    handshake_deadline: Instant,
    next_id: i64,
    cmd_tx: mpsc::Sender<SessionCommand>,
    registered: bool,
    stop: Option<Stop>,
    retry_after_hibernate: Option<SessionCommand>,
    thread: ThreadState,
}

impl<'a> EventLoop<'a> {
    fn new(
        session: &'a CodexSession,
        ack: &'a mut SpawnAck,
        rings: &'a Arc<DiagnoseRings>,
        child: Option<Child>,
        stdin: RpcStdin,
        thread_config: ThreadConfig,
        cmd_tx: mpsc::Sender<SessionCommand>,
    ) -> Self {
        Self {
            session,
            ack,
            rings,
            child,
            stdin,
            thread_config,
            pending_rpcs: PendingRpcs::default(),
            handshake_deadline: Instant::now() + HANDSHAKE_TIMEOUT,
            next_id: RUN_BASE,
            cmd_tx,
            registered: false,
            stop: None,
            retry_after_hibernate: None,
            thread: ThreadState::default(),
        }
    }

    const fn events(&self) -> &'a mpsc::Sender<AdapterEvent> {
        &self.session.events
    }

    async fn command_result(&self, command_id: Option<Uuid>, error: Option<String>) {
        if let Some(command_id) = command_id {
            let _ = self
                .events()
                .send(AdapterEvent::CommandResult { command_id, ok: error.is_none(), error })
                .await;
        }
    }

    async fn failed_status(&self, detail: String) {
        self.events()
            .send(AdapterEvent::Status {
                local_id: self.thread.local_id.clone(),
                tempo: None,
                state: Some("failed".to_owned()),
                detail: Some(detail),
                activity: Some("failure".to_owned()),
                name: None,
                intent: None,
                model: None,
                effort: None,
                permission_mode: None,
                children: vec![],
            })
            .await
            .ok();
    }

    /// Handshake: initialize → thread/start or thread/resume.
    async fn send_initialize(&mut self) {
        self.pending_rpcs.insert(ID_INITIALIZE, "initialize", None, self.handshake_deadline);
        // EPIPE here means codex already died (auth/config errors exit at
        // once); let the stdout EOF below reach the epilogue, which reports the
        // exit status with the stderr tail.
        if let Err(e) = self.stdin.send(&initialize_req()).await {
            tracing::warn!(%e, "codex: initialize write failed");
        }
    }

    async fn send_thread_request(&mut self) -> Result<()> {
        let (req, method) = self.session.thread_request(&self.thread_config);
        self.pending_rpcs.insert(ID_THREAD_START, method, None, self.handshake_deadline);
        self.stdin.send(&req).await
    }

    async fn send_model_list(&mut self, cursor: Option<&str>, deadline: Instant) -> Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        self.pending_rpcs.insert(id, "model/list", None, deadline);
        self.stdin.send(&model_list::model_list_req(id, cursor)).await
    }

    async fn start_turn(
        &mut self,
        text: &str,
        attachments: &[String],
        command_id: Option<Uuid>,
    ) -> Result<()> {
        let req = turn_start_req(
            self.next_id,
            &self.thread.local_id,
            text,
            attachments,
            self.thread.override_model.as_deref(),
            self.thread.override_effort.as_deref(),
        );
        self.pending_rpcs.insert(
            self.next_id,
            "turn/start",
            command_id,
            Instant::now() + RPC_TIMEOUT,
        );
        self.next_id += 1;
        self.stdin.send(&req).await
    }

    async fn rename(&mut self, name: &str) -> Result<()> {
        set_thread_name(
            &mut self.stdin,
            &mut self.next_id,
            &mut self.pending_rpcs,
            &self.thread.local_id,
            name,
            &self.session.events,
            &self.session.registry,
        )
        .await
    }

    async fn set_model(
        &mut self,
        model: Option<&str>,
        effort: Option<&str>,
        command_id: Option<Uuid>,
    ) {
        record_model_override(
            &mut self.thread.override_model,
            &mut self.thread.override_effort,
            model,
            effort,
            &self.thread.local_id,
            &self.session.events,
            &self.session.registry,
            command_id,
        )
        .await;
    }

    fn on_shutdown(&mut self) -> Flow {
        kill_child(&mut self.child);
        self.stop = Some(Stop::Killed);
        Flow::Break
    }

    /// SIGTERM (not kill) so codex flushes its rollout before the re-exec; the
    /// record stays so the new daemon can resume.
    fn on_reexec(&mut self) -> Flow {
        if let Some(child) = self.child.as_mut() {
            terminate_child(child, Some(SIGTERM));
        }
        self.stop = Some(Stop::Reexec);
        Flow::Break
    }

    async fn on_sweep(&mut self) -> Flow {
        let mut handshake_dead = false;
        for (id, pending) in self.pending_rpcs.expire(Instant::now()) {
            tracing::warn!(rpc_id = id, method = %pending.method, "codex: JSON-RPC request timed out");
            self.command_result(
                pending.command_id,
                Some(format!("codex {} timed out", pending.method)),
            )
            .await;
            if pending.is_handshake() || self.thread.validating_model {
                let detail = format!(
                    "codex {} timed out after {}s{}",
                    pending.method,
                    HANDSHAKE_TIMEOUT.as_secs(),
                    stderr_tail(self.rings)
                );
                self.session.fail_handshake(self.ack, &detail).await;
                handshake_dead = true;
            }
        }
        if handshake_dead {
            self.stop = Some(Stop::HandshakeFailed);
            kill_child(&mut self.child);
            return Flow::Break;
        }
        Flow::Continue
    }

    async fn on_command(&mut self, cmd: Option<SessionCommand>) -> Flow {
        match cmd {
            Some(SessionCommand::Permission { request_id, allow }) => {
                self.on_permission(&request_id, allow).await
            }
            Some(SessionCommand::Send { text, command_id }) => self.on_send(text, command_id).await,
            Some(SessionCommand::Rename { name }) => {
                if let Err(e) = self.rename(&name).await {
                    tracing::warn!(%e, "codex: thread/name/set write failed; ending session");
                    self.retry_after_hibernate = Some(SessionCommand::Rename { name });
                    return Flow::Break;
                }
                Flow::Continue
            }
            Some(SessionCommand::Kill { signal }) => self.on_kill(signal).await,
            Some(SessionCommand::Interrupt { command_id }) => self.on_interrupt(command_id).await,
            Some(SessionCommand::SetModel { model, effort, command_id }) => {
                self.set_model(model.as_deref(), effort.as_deref(), command_id).await;
                Flow::Continue
            }
            Some(SessionCommand::Diagnose { reply }) => {
                let _ = reply.send(self.snapshot()).await;
                Flow::Continue
            }
            None => Flow::Break,
        }
    }

    async fn on_permission(&mut self, request_id: &str, allow: bool) -> Flow {
        if let Some((rpc_id, kind)) = self.thread.pending_approvals.remove(request_id) {
            if let Err(e) = self.stdin.send(&approval_reply(&rpc_id, kind, allow)).await {
                tracing::warn!(%e, "codex: approval write failed; ending session");
                return Flow::Break;
            }
        } else {
            tracing::warn!(%request_id, "codex: no pending approval for response");
        }
        Flow::Continue
    }

    async fn on_send(&mut self, text: String, command_id: Option<Uuid>) -> Flow {
        if let Some((rpc_id, question_ids)) = self.thread.pending_questions.pop_front() {
            let reply = user_input_reply(&rpc_id, &question_ids, &text);
            if let Err(e) = self.stdin.send(&reply).await {
                tracing::warn!(%e, "codex: requestUserInput answer write failed; ending session");
                return Flow::Break;
            }
            self.command_result(command_id, None).await;
            self.events()
                .send(AdapterEvent::AskResolved { local_id: self.thread.local_id.clone() })
                .await
                .ok();
            return Flow::Continue;
        }
        let next_id = self.next_id;
        let (req, method) = match prompt_dispatch(&self.thread.active_turn) {
            PromptDispatch::Steer { turn_id } => {
                self.thread.steer_texts.insert(next_id, text.clone());
                (turn_steer_req(next_id, &self.thread.local_id, &turn_id, &text, &[]), "turn/steer")
            }
            PromptDispatch::Start => (
                turn_start_req(
                    next_id,
                    &self.thread.local_id,
                    &text,
                    &[],
                    self.thread.override_model.as_deref(),
                    self.thread.override_effort.as_deref(),
                ),
                "turn/start",
            ),
        };
        self.pending_rpcs.insert(next_id, method, command_id, Instant::now() + RPC_TIMEOUT);
        self.next_id += 1;
        // A write failure here means the app-server is gone — remember the
        // turn and let the epilogue revive the thread if this was a clean
        // hibernation exit.
        if let Err(e) = self.stdin.send(&req).await {
            tracing::warn!(%e, "codex: turn dispatch write failed; ending session");
            self.thread.steer_texts.remove(&next_id);
            self.pending_rpcs.remove(next_id);
            self.retry_after_hibernate = Some(SessionCommand::Send { text, command_id });
            return Flow::Break;
        }
        Flow::Continue
    }

    async fn on_kill(&mut self, signal: Option<i32>) -> Flow {
        if let Some(turn_id) = self.thread.active_turn.id() {
            let req = turn_interrupt_req(self.next_id, &self.thread.local_id, turn_id);
            let _ = self.stdin.send(&req).await;
        }
        if let Some(child) = self.child.as_mut() {
            terminate_child(child, signal);
        }
        self.stop = Some(Stop::Killed);
        Flow::Break
    }

    /// Keep-alive interrupt: abort the turn but leave the app-server running
    /// so the session keeps going — unlike Kill, the child is NOT terminated.
    async fn on_interrupt(&mut self, command_id: Option<Uuid>) -> Flow {
        let Some(turn_id) = self.thread.active_turn.id() else {
            self.command_result(command_id, Some(NO_TURN_IN_FLIGHT.to_owned())).await;
            return Flow::Continue;
        };
        let req = turn_interrupt_req(self.next_id, &self.thread.local_id, turn_id);
        self.pending_rpcs.insert(
            self.next_id,
            "turn/interrupt",
            command_id,
            Instant::now() + RPC_TIMEOUT,
        );
        self.next_id += 1;
        if let Err(e) = self.stdin.send(&req).await {
            tracing::warn!(%e, "codex: turn/interrupt write failed; ending session");
            return Flow::Break;
        }
        Flow::Continue
    }

    fn snapshot(&self) -> CodexLiveSnapshot {
        CodexLiveSnapshot {
            codex_version: self.thread.codex_version.clone(),
            pid: self.child.as_ref().and_then(Child::id),
            active_turn_id: self.thread.active_turn.id().map(str::to_owned),
            pending_rpc_methods: self.pending_rpcs.pending_methods(),
            protocol_errors: self.rings.protocol_errors_with_shared(),
            stderr_tail: self.rings.stderr_tail(),
            rpc_tail: self.rings.rpc_tail_with_shared(),
            rollout_path: self.thread.rollout_path.clone(),
            rollout_size_bytes: self
                .thread
                .rollout_path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.len()),
        }
    }

    /// EOF (`Ok(None)`) or a read error both mean the app-server is gone;
    /// break and let the epilogue classify the exit.
    async fn on_line(&mut self, line: std::io::Result<Option<String>>) -> Result<Flow> {
        let line = match line {
            Ok(Some(line)) => line,
            Ok(None) => return Ok(Flow::Break),
            Err(e) => {
                tracing::warn!(%e, "codex: stdout read error; ending session");
                return Ok(Flow::Break);
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(Flow::Continue);
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            tracing::debug!(line = %trimmed, "codex: non-JSON line");
            return Ok(Flow::Continue);
        };
        self.rings.note_rpc("in", &value);
        if self.child.is_none()
            && let Some(event) = subagent_started_event(&self.thread.local_id, &value)
        {
            self.events().send(event).await.ok();
            return Ok(Flow::Continue);
        }
        if let Some(ev) = turn_lifecycle(&value) {
            self.on_turn_lifecycle(&ev).await;
        }
        self.thread.items.note(&value);
        let value = self.thread.items.enrich_completed(value);
        match classify(&self.thread.local_id, &value) {
            Incoming::Response { id, value } => self.on_response(id, &value).await,
            request @ (Incoming::Approval { .. }
            | Incoming::Question { .. }
            | Incoming::Decline { .. }) => Ok(self.on_server_request(request).await),
            notification => {
                self.on_notification(notification).await;
                Ok(Flow::Continue)
            }
        }
    }

    async fn on_turn_lifecycle(&mut self, ev: &TurnLifecycle) {
        self.thread.active_turn.apply(ev);
        // A spawned child's caller is parked on turn completion; codex has no
        // state.json to flip, so emit the done status childwatch classifies
        // on. Scoped to spawn_key sessions — observed threads keep the
        // successful-turn-is-ignored behavior.
        if matches!(ev, TurnLifecycle::Completed { .. })
            && self.session.spawn_key.is_some()
            && !self.thread.local_id.is_empty()
        {
            self.events()
                .send(AdapterEvent::Status {
                    local_id: self.thread.local_id.clone(),
                    tempo: None,
                    state: Some("done".to_owned()),
                    detail: None,
                    activity: Some("success".to_owned()),
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
    }

    async fn on_server_request(&mut self, request: Incoming) -> Flow {
        match request {
            Incoming::Approval { rpc_id, request_id, tool, kind, input } => {
                self.thread.pending_approvals.insert(request_id.clone(), (rpc_id, kind));
                self.events()
                    .send(AdapterEvent::PermissionRequest {
                        local_id: self.thread.local_id.clone(),
                        request_id,
                        tool,
                        input,
                    })
                    .await
                    .ok();
            }
            Incoming::Question { rpc_id, question, questions, question_ids } => {
                self.thread.pending_questions.push_back((rpc_id, question_ids));
                self.events()
                    .send(AdapterEvent::AskQuestion {
                        local_id: self.thread.local_id.clone(),
                        question,
                        questions: Some(questions),
                        preamble: None,
                    })
                    .await
                    .ok();
            }
            Incoming::Decline { reply } => {
                if let Err(e) = self.stdin.send(&reply).await {
                    tracing::warn!(%e, "codex: decline write failed; ending session");
                    return Flow::Break;
                }
            }
            _ => {}
        }
        Flow::Continue
    }

    async fn on_notification(&self, notification: Incoming) {
        match notification {
            Incoming::Event(evt) => {
                self.events().send(evt).await.ok();
            }
            Incoming::Traced { method, reason } => {
                tracing::trace!(%method, reason, "codex notification consumed out-of-band");
            }
            Incoming::Unhandled { method, event } => {
                tracing::warn!(%method, "unhandled codex notification");
                self.rings.note_protocol_error(&format!("unhandled codex notification {method}"));
                self.events().send(event).await.ok();
            }
            _ => {}
        }
    }

    async fn on_response(&mut self, id: i64, response: &Value) -> Result<Flow> {
        let Some((pending, outcome)) = self.pending_rpcs.resolve(id, response) else {
            tracing::debug!(rpc_id = id, "codex: response for unknown request id");
            return Ok(Flow::Continue);
        };
        if let Err(ref e) = outcome {
            self.rings.note_protocol_error(&format!("{}: {e}", pending.method));
        }
        if outcome.is_ok() && pending.command_id.is_some() {
            self.command_result(pending.command_id, None).await;
        }
        match (pending.method.as_str(), outcome) {
            ("initialize", Ok(_)) => {
                self.on_initialized(response).await?;
                Ok(Flow::Continue)
            }
            ("thread/start" | "thread/resume" | "thread/fork", Ok(result)) => {
                self.on_thread_started(&result).await
            }
            (method, Err(err)) if pending.is_handshake() => {
                tracing::error!(%err, %method, "codex: handshake request failed; ending session");
                let detail = format!("codex {method}: {err}{}", stderr_tail(self.rings));
                self.session.fail_handshake(self.ack, &detail).await;
                self.stop = Some(Stop::HandshakeFailed);
                kill_child(&mut self.child);
                Ok(Flow::Break)
            }
            ("model/list", Err(err)) if self.thread.validating_model => {
                // The catalog is best-effort; codex still rejects a bad
                // model itself on the first turn.
                tracing::debug!(%err, "codex: pre-start model/list failed; skipping model check");
                self.thread.validating_model = false;
                self.thread.model_catalog.clear();
                self.send_thread_request().await?;
                Ok(Flow::Continue)
            }
            ("model/list", Ok(result)) if self.thread.validating_model => {
                self.on_model_check_page(&result).await?;
                Ok(Flow::Continue)
            }
            ("model/list", Ok(result)) => {
                self.on_model_list_page(&result).await;
                Ok(Flow::Continue)
            }
            ("model/list", Err(err)) => {
                tracing::debug!(%err, "codex: model/list refresh failed");
                self.thread.model_catalog.clear();
                Ok(Flow::Continue)
            }
            ("turn/steer", Ok(_)) => {
                self.thread.steer_texts.remove(&id);
                Ok(Flow::Continue)
            }
            ("turn/steer", Err(err)) => Ok(self.on_steer_failed(id, &pending, err).await),
            (method, Err(err)) => {
                tracing::warn!(%err, %method, "codex: JSON-RPC request failed");
                self.command_result(pending.command_id, Some(err.clone())).await;
                if method == "turn/start" {
                    self.failed_status(err).await;
                }
                Ok(Flow::Continue)
            }
            (_, Ok(_)) => Ok(Flow::Continue),
        }
    }

    async fn on_initialized(&mut self, response: &Value) -> Result<()> {
        self.thread.codex_version = record_codex_version(response);
        // Complete the documented handshake before any thread request: the
        // server treats `thread/*` sent before `initialized` as premature.
        self.stdin.send(&initialized_notification()).await?;
        if self.session.cfg.model_catalog && self.session.cfg.model.is_some() {
            self.thread.validating_model = true;
            self.send_model_list(None, self.handshake_deadline).await
        } else {
            self.send_thread_request().await
        }
    }

    /// One page of the pre-start `model/list` that validates `-c model=`.
    async fn on_model_check_page(&mut self, result: &Value) -> Result<()> {
        self.thread.model_catalog.extend(model_list::parse_model_list(result));
        self.thread.model_catalog_pages += 1;
        if let model_list::PageStep::Next { cursor } =
            model_list::page_step(self.thread.model_catalog_pages, result)
        {
            return self.send_model_list(Some(&cursor), self.handshake_deadline).await;
        }
        self.thread.validating_model = false;
        let catalog = CodexModelCatalog {
            models: std::mem::take(&mut self.thread.model_catalog),
            client_version: None,
        };
        if let Some(warning) = unknown_model(self.session.cfg.model.as_deref(), &catalog) {
            tracing::warn!(%warning, "codex: spawning anyway");
        }
        self.thread.catalog_sent = true;
        self.events().send(AdapterEvent::CodexModels { catalog }).await.ok();
        self.send_thread_request().await
    }

    async fn on_model_list_page(&mut self, result: &Value) {
        self.thread.model_catalog.extend(model_list::parse_model_list(result));
        self.thread.model_catalog_pages += 1;
        match model_list::page_step(self.thread.model_catalog_pages, result) {
            model_list::PageStep::Next { cursor } => {
                let id = self.next_id;
                if let Err(e) =
                    self.send_model_list(Some(&cursor), Instant::now() + RPC_TIMEOUT).await
                {
                    tracing::debug!(%e, "codex: model/list page write failed");
                    self.pending_rpcs.resolve(id, &json!({}));
                }
            }
            model_list::PageStep::Done => {
                let catalog = CodexModelCatalog {
                    models: std::mem::take(&mut self.thread.model_catalog),
                    client_version: None,
                };
                self.events().send(AdapterEvent::CodexModels { catalog }).await.ok();
            }
        }
    }

    async fn on_steer_failed(&mut self, id: i64, pending: &PendingRpc, err: String) -> Flow {
        let text = self.thread.steer_texts.remove(&id);
        match (steer_recovery(&err), text) {
            (SteerRecovery::FallbackToStart, Some(text)) => {
                self.thread.active_turn.clear();
                tracing::info!(%err, "codex: turn/steer stale; falling back to turn/start");
                if let Err(e) = self.start_turn(&text, &[], pending.command_id).await {
                    tracing::warn!(%e, "codex: turn/start fallback write failed; ending session");
                    self.pending_rpcs.remove(self.next_id - 1);
                    self.retry_after_hibernate =
                        Some(SessionCommand::Send { text, command_id: pending.command_id });
                    return Flow::Break;
                }
            }
            (recovery, _) => {
                tracing::warn!(%err, ?recovery, "codex: turn/steer rejected");
                self.command_result(pending.command_id, Some(err.clone())).await;
                self.failed_status(err).await;
            }
        }
        Flow::Continue
    }

    async fn on_thread_started(&mut self, result: &Value) -> Result<Flow> {
        let Some(info) = thread_info(result) else {
            anyhow::bail!("codex thread/start response missing thread id");
        };
        self.register_thread(info).await;
        // Refresh the account/machine model catalog over THIS authenticated
        // connection (the gateway credential is in env), so gateway-only
        // machines get the current remote list instead of a stale
        // unauthenticated fallback. Best-effort: a failure is logged, never
        // fatal to the session.
        if self.session.cfg.model_catalog && !self.thread.catalog_sent {
            let id = self.next_id;
            if let Err(e) = self.send_model_list(None, Instant::now() + RPC_TIMEOUT).await {
                tracing::debug!(%e, "codex: model/list write failed");
                self.pending_rpcs.resolve(id, &json!({}));
            }
        }
        self.announce_launch_settings().await;
        let session = self.session;
        let end_after_initial = match &session.launch {
            SessionLaunch::Fresh { name, prompt, attachments }
            | SessionLaunch::Fork { name, prompt, attachments, .. } => {
                self.open_fresh_thread(name.as_deref(), prompt.as_deref(), attachments).await
            }
            SessionLaunch::Resume { initial_commands, .. } => {
                self.replay_initial_commands(initial_commands.clone()).await
            }
        };
        Ok(if end_after_initial { Flow::Break } else { Flow::Continue })
    }

    /// Announce the started thread and make it routable: `SessionStarted`,
    /// the durable record, and the live command sender.
    async fn register_thread(&mut self, info: ThreadInfo) {
        let session = self.session;
        self.thread.local_id.clone_from(&info.thread_id);
        self.thread.rollout_path.clone_from(&info.rollout_path);
        let local_id = self.thread.local_id.clone();
        let (parent_local_id, relation) =
            child_linkage(&session.launch, session.parent_local_id.as_deref());
        // A `CctuiAgent` call from this session arrives keyed by the launch
        // key baked into the relay argv; the thread id it really is only
        // exists now.
        if let Some(agent_mcp) = &session.agent_mcp {
            crate::agenttool::bind_session_alias(agent_mcp.session_key(), &local_id);
            crate::adapters::agent_mcp::remember(&local_id, agent_mcp);
        }
        session
            .events
            .send(AdapterEvent::SessionStarted {
                local_id: local_id.clone(),
                meta: SessionMeta {
                    working_dir: info.cwd.or_else(|| Some(session.cwd.clone())),
                    parent_local_id,
                    extra: json!({
                        "source": "codex-app-server",
                        "rollout_path": info.rollout_path,
                        "codex_version": self.thread.codex_version,
                        "spawn_key": session.spawn_key,
                        "relation": relation,
                    }),
                },
            })
            .await
            .ok();
        let remembered_name = match &session.launch {
            SessionLaunch::Fresh { name, .. } | SessionLaunch::Fork { name, .. } => name.clone(),
            SessionLaunch::Resume { .. } => {
                session.registry.lock().await.get(&local_id).and_then(|r| r.name.clone())
            }
        };
        session.registry.lock().await.insert(
            local_id.clone(),
            SessionRecord {
                cfg: session.cfg.clone(),
                cwd: session.cwd.clone(),
                name: remembered_name,
                env: session.env.clone(),
                spawn_relay: session.agent_mcp.is_some(),
            },
        );
        crate::adapters::codex::persist::save(&session.registry).await;
        session.live.lock().await.insert(local_id, self.cmd_tx.clone());
        self.registered = true;
        self.ack.ok().await;
    }

    /// Surface the configured model + reasoning effort so the session list
    /// shows them (claude gets this for free via state.json; codex has no
    /// equivalent feed). Emit when either is known.
    async fn announce_launch_settings(&self) {
        let model = self.session.cfg.model.clone();
        let effort = self.session.cfg.reasoning_effort.clone();
        if model.is_some() || effort.is_some() {
            self.events()
                .send(AdapterEvent::Status {
                    local_id: self.thread.local_id.clone(),
                    tempo: None,
                    state: None,
                    detail: None,
                    activity: None,
                    name: None,
                    intent: None,
                    model,
                    effort,
                    permission_mode: None,
                    children: vec![],
                })
                .await
                .ok();
        }
    }

    /// Name a fresh or forked thread and send its first turn. `true` ends the
    /// session so the epilogue can retry what failed on a revived thread.
    async fn open_fresh_thread(
        &mut self,
        name: Option<&str>,
        prompt: Option<&str>,
        attachments: &[String],
    ) -> bool {
        if let Some(name) = name
            && let Err(e) = self.rename(name).await
        {
            tracing::warn!(%e, "codex: initial thread/name/set failed");
            self.retry_after_hibernate = Some(SessionCommand::Rename { name: name.to_owned() });
            return true;
        }
        // Send the first turn when there is a prompt OR staged attachments —
        // an image-only spawn carries no prompt text but must still reach
        // codex as a `localImage` turn input.
        if prompt.is_some() || !attachments.is_empty() {
            let prompt_text = prompt.unwrap_or("");
            if let Err(e) = self.start_turn(prompt_text, attachments, None).await {
                tracing::warn!(%e, "codex: initial prompt write failed; ending session");
                self.retry_after_hibernate =
                    Some(SessionCommand::Send { text: prompt_text.to_owned(), command_id: None });
                return true;
            }
        }
        false
    }

    /// Re-issue the commands a resume was launched for. `true` ends the
    /// session so the epilogue can retry what failed on a revived thread.
    async fn replay_initial_commands(&mut self, commands: Vec<SessionCommand>) -> bool {
        for command in commands {
            match command {
                SessionCommand::Send { text, command_id } => {
                    if let Err(e) = self.start_turn(&text, &[], command_id).await {
                        tracing::warn!(%e, "codex: resumed turn/start write failed");
                        self.pending_rpcs.remove(self.next_id - 1);
                        self.retry_after_hibernate =
                            Some(SessionCommand::Send { text, command_id });
                        return true;
                    }
                }
                SessionCommand::Rename { name } => {
                    if let Err(e) = self.rename(&name).await {
                        tracing::warn!(%e, "codex: resumed thread/name/set write failed");
                        self.retry_after_hibernate = Some(SessionCommand::Rename { name });
                        return true;
                    }
                }
                SessionCommand::SetModel { model, effort, command_id } => {
                    self.set_model(model.as_deref(), effort.as_deref(), command_id).await;
                }
                other => {
                    tracing::warn!(?other, "codex: ignoring non-resumable initial command");
                }
            }
        }
        false
    }

    async fn finish(
        mut self,
        mut cmd_rx: mpsc::Receiver<SessionCommand>,
        stderr_drain: Option<JoinHandle<()>>,
    ) -> Result<()> {
        // Drop the live sender NOW so new commands take the Resume path
        // instead of landing in this dead channel's buffer, then drain
        // whatever was already buffered.
        if !self.thread.local_id.is_empty() {
            self.session.live.lock().await.remove(&self.thread.local_id);
        }
        cmd_rx.close();
        let mut drained: Vec<SessionCommand> = Vec::new();
        while let Ok(cmd) = cmd_rx.try_recv() {
            drained.push(cmd);
        }
        self.cancel_pending_rpcs().await;

        // Reap the child and classify why the session ended. An abnormal exit
        // that we did not request is surfaced as `Crashed` with the captured
        // stderr tail — the diagnostic for the macOS "randomly dies" report.
        let status = match self.child.as_mut() {
            Some(child) => Some(child.wait().await),
            None => None,
        };
        // Let the drain catch codex's final lines before any tail is read.
        if let Some(drain) = stderr_drain {
            let _ = tokio::time::timeout(Duration::from_secs(1), drain).await;
        }
        if self.thread.local_id.is_empty() {
            if matches!(self.stop, Some(Stop::Reexec | Stop::HandshakeFailed)) {
                return Ok(());
            }
            let Some(status) = status else {
                anyhow::bail!(
                    "shared codex app-server closed the route before the thread was started"
                );
            };
            let exit = status.map_or_else(|e| e.to_string(), |s| s.to_string());
            anyhow::bail!("codex app-server exited ({exit}) before the thread was started");
        }
        if self.stop == Some(Stop::Reexec) {
            return Ok(());
        }
        self.end_thread(status, drained).await;
        Ok(())
    }

    async fn cancel_pending_rpcs(&mut self) {
        for (id, pending) in self.pending_rpcs.drain() {
            tracing::warn!(rpc_id = id, method = %pending.method, "codex: cancelling pending request — app-server gone");
            self.command_result(
                pending.command_id,
                Some(format!("codex {}: app-server exited before responding", pending.method)),
            )
            .await;
        }

        // Paths that hand a request to a retry (`retry_after_hibernate`) must
        // `pending_rpcs.remove` it first, or it is failed here as well.
        for (_, pending) in self.pending_rpcs.drain() {
            if let Some(command_id) = pending.command_id {
                self.events()
                    .send(AdapterEvent::CommandResult {
                        command_id,
                        ok: false,
                        error: Some(format!(
                            "codex app-server exited before {} was acknowledged",
                            pending.method
                        )),
                    })
                    .await
                    .ok();
            }
        }
    }

    fn end_reason(
        &self,
        status: Option<std::io::Result<ExitStatus>>,
        drained_kill: bool,
    ) -> Option<EndReason> {
        if self.stop == Some(Stop::Killed) || drained_kill {
            return Some(EndReason::Killed);
        }
        match status {
            None => Some(EndReason::Crashed {
                detail: "shared codex app-server lost the thread and could not rejoin it"
                    .to_owned(),
            }),
            Some(Ok(s)) if s.success() => None,
            Some(Ok(s)) => Some(EndReason::Crashed {
                detail: format!("codex app-server exited ({s}){}", stderr_tail(self.rings)),
            }),
            Some(Err(e)) => {
                Some(EndReason::Crashed { detail: format!("codex app-server wait failed: {e}") })
            }
        }
    }

    async fn end_thread(
        self,
        status: Option<std::io::Result<ExitStatus>>,
        drained: Vec<SessionCommand>,
    ) {
        let session = self.session;
        let local_id = &self.thread.local_id;
        let (mut retry, dropped, drained_kill) = partition_drained(drained);
        if let Some(reason) = self.end_reason(status, drained_kill) {
            if let EndReason::Crashed { detail } = &reason {
                tracing::error!(%detail, "codex app-server session crashed");
            }
            // A crash keeps the durable record: `thread/resume` still works,
            // so the next command revives the thread instead of going Missing.
            if removes_record(&reason) {
                session.registry.lock().await.remove(local_id);
                crate::adapters::codex::persist::save(&session.registry).await;
            }
            for cmd in retry.into_iter().chain(dropped) {
                session.fail_dropped_command(local_id, &cmd).await;
            }
            session
                .events
                .send(AdapterEvent::SessionEnded { local_id: local_id.clone(), reason })
                .await
                .ok();
            return;
        }
        session
            .events
            .send(AdapterEvent::Status {
                local_id: local_id.clone(),
                tempo: Some("hibernated".to_owned()),
                state: None,
                detail: None,
                activity: None,
                name: None,
                intent: None,
                model: None,
                effort: None,
                permission_mode: None,
                children: Vec::new(),
            })
            .await
            .ok();
        for cmd in dropped {
            session.fail_dropped_command(local_id, &cmd).await;
        }
        if let Some(command) = self.retry_after_hibernate {
            retry.insert(0, command);
        }
        if !retry.is_empty()
            && let Some(record) = session.registry.lock().await.get(local_id).cloned()
        {
            spawn_resumed_session(
                record,
                local_id,
                retry,
                session.events.clone(),
                session.live.clone(),
                session.registry.clone(),
                session.shutdown.clone(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::config::AppServerConfig;
    use super::super::registry::{LiveSessionRegistry, SessionRegistry};
    use super::*;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    #[ignore = "requires `codex` installed locally; run with `--ignored`"]
    async fn real_codex_handshake_emits_session_started() {
        let (tx, mut rx) = mpsc::channel(64);
        let live = LiveSessionRegistry::default();
        let registry = SessionRegistry::default();
        let shutdown = CancellationToken::new();
        let session = CodexSession::new_fresh(
            AppServerConfig::default(),
            "/tmp".to_string(),
            std::collections::BTreeMap::new(),
            None, // no prompt → no turn/start, so no model auth needed
            None,
            Vec::new(),
            None,
            None,
            None,
            tx,
            live,
            registry.clone(),
            shutdown.clone(),
        );
        let handle = tokio::spawn(session.run());
        let evt = tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv())
            .await
            .expect("timed out waiting for SessionStarted")
            .expect("event channel closed");
        match evt {
            AdapterEvent::SessionStarted { local_id, meta } => {
                assert!(!local_id.is_empty(), "session id should be the rollout uuid");
                assert_eq!(meta.working_dir.as_deref(), Some("/tmp"));
                assert!(registry.lock().await.contains_key(&local_id), "session must register");
            }
            other => panic!("expected SessionStarted, got {other:?}"),
        }
        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    fn fake_codex(script: &str) -> (tempfile::TempDir, String) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("codex");
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = bin.to_string_lossy().into_owned();
        (dir, path)
    }

    /// Answers `initialize`, serves a two-model catalog, and records any
    /// `thread/start` in `marker` so a test can assert it never got there.
    fn catalog_server_script(marker: &std::path::Path) -> String {
        FAKE_CATALOG_SERVER.replace("$MARKER", &marker.to_string_lossy())
    }

    const FAKE_CATALOG_SERVER: &str = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) echo '{"jsonrpc":"2.0","id":1,"result":{"userAgent":"codex/0.144.1"}}' ;;
    *'"method":"model/list"'*) echo '{"jsonrpc":"2.0","id":100,"result":{"data":[{"id":"gpt-5-codex","model":"gpt-5-codex","displayName":"GPT-5 Codex","hidden":false,"isDefault":true},{"id":"gpt-5-secret","model":"gpt-5-secret","displayName":"hidden","hidden":true}]}}' ;;
    *'"method":"thread/start"'*) echo started > "$MARKER"; echo '{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"t-1","cwd":"/tmp"}}}' ;;
  esac
done
"#;

    fn run_fresh_spawn(
        bin: String,
        model: Option<&str>,
    ) -> (Uuid, mpsc::Receiver<AdapterEvent>, CancellationToken) {
        let (tx, rx) = mpsc::channel(64);
        let command_id = Uuid::new_v4();
        let shutdown = CancellationToken::new();
        let session = CodexSession::new_fresh(
            AppServerConfig { bin, model: model.map(str::to_owned), ..AppServerConfig::default() },
            "/tmp".to_string(),
            std::collections::BTreeMap::new(),
            None,
            None,
            Vec::new(),
            Some(command_id),
            None,
            None,
            tx,
            LiveSessionRegistry::default(),
            SessionRegistry::default(),
            shutdown.clone(),
        );
        tokio::spawn(session.run());
        (command_id, rx, shutdown)
    }

    async fn spawn_result(rx: &mut mpsc::Receiver<AdapterEvent>) -> (bool, Option<String>) {
        let wait = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while let Some(evt) = rx.recv().await {
                if let AdapterEvent::CommandResult { ok, error, .. } = evt {
                    return (ok, error);
                }
            }
            panic!("no CommandResult");
        });
        wait.await.expect("spawn ack within 10s")
    }

    #[tokio::test]
    async fn early_exit_folds_stderr_into_spawn_failure() {
        let (_dir, bin) =
            fake_codex("#!/bin/sh\necho 'error: not logged in; run codex login' >&2\nexit 1\n");
        let (command_id, mut rx, shutdown) = run_fresh_spawn(bin, None);
        let (ok, error) = spawn_result(&mut rx).await;
        shutdown.cancel();
        assert!(!ok, "{command_id} should fail");
        let error = error.unwrap();
        assert!(error.contains("exited (exit status: 1) before the thread was started"), "{error}");
        assert!(error.contains("not logged in; run codex login"), "{error}");
    }

    #[tokio::test]
    async fn a_model_the_local_catalog_does_not_list_still_spawns() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started");
        let (_bin_dir, bin) = fake_codex(&catalog_server_script(&marker));
        let (_, mut rx, shutdown) = run_fresh_spawn(bin, Some("gpt-6-astra"));
        let (ok, error) = spawn_result(&mut rx).await;
        shutdown.cancel();
        assert!(ok, "{error:?}");
        assert!(marker.exists(), "a stale catalog must not block a valid model");
    }

    #[tokio::test]
    async fn known_model_passes_the_catalog_check() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started");
        let (_bin_dir, bin) = fake_codex(&catalog_server_script(&marker));
        let (_, mut rx, shutdown) = run_fresh_spawn(bin, Some("gpt-5-codex"));
        let (ok, error) = spawn_result(&mut rx).await;
        shutdown.cancel();
        assert!(ok, "{error:?}");
        assert!(marker.exists());
    }

    /// Answers the handshake, then runs `$TURN` for a `turn/start`. Echoes the
    /// request's own id back so the driver's correlation table resolves.
    fn turn_server_script(turn: &str) -> String {
        format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed 's/^[^}}]*"id":\([0-9]*\).*$/\1/')
  case "$line" in
    *'"method":"initialize"'*) echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"userAgent\":\"codex/0.144.1\"}}}}" ;;
    *'"method":"thread/start"'*) echo "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"thread\":{{\"id\":\"t-1\",\"cwd\":\"/tmp\"}}}}}}" ;;
    *'"method":"turn/start"'*) {turn} ;;
  esac
done
"#
        )
    }

    async fn send_to_live_session(
        bin: String,
        cmd: SessionCommand,
    ) -> (mpsc::Receiver<AdapterEvent>, CancellationToken) {
        let (tx, mut rx) = mpsc::channel(64);
        let shutdown = CancellationToken::new();
        let live = LiveSessionRegistry::default();
        let session = CodexSession::new_fresh(
            AppServerConfig { bin, ..AppServerConfig::default() },
            "/tmp".to_string(),
            std::collections::BTreeMap::new(),
            None,
            None,
            Vec::new(),
            Some(Uuid::new_v4()),
            None,
            None,
            tx,
            live.clone(),
            SessionRegistry::default(),
            shutdown.clone(),
        );
        tokio::spawn(session.run());
        let (ok, error) = spawn_result(&mut rx).await;
        assert!(ok, "spawn failed: {error:?}");
        let sender = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let registered = live.lock().await.get("t-1").cloned();
                if let Some(tx) = registered {
                    return tx;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("session registered within 10s");
        sender.send(cmd).await.unwrap();
        (rx, shutdown)
    }

    #[tokio::test]
    async fn a_send_is_acked_once_the_app_server_accepts_the_turn() {
        let (_dir, bin) = fake_codex(&turn_server_script(
            r#"echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{}}""#,
        ));
        let command_id = Uuid::new_v4();
        let (mut rx, shutdown) = send_to_live_session(
            bin,
            SessionCommand::Send { text: "hi".to_owned(), command_id: Some(command_id) },
        )
        .await;
        let (ok, error) = spawn_result(&mut rx).await;
        shutdown.cancel();
        assert!(ok, "a delivered send must confirm, not stay unconfirmed: {error:?}");
    }

    #[tokio::test]
    async fn a_send_in_flight_when_the_app_server_dies_is_acked_as_failed() {
        let (_dir, bin) = fake_codex(&turn_server_script("exit 0"));
        let command_id = Uuid::new_v4();
        let (mut rx, shutdown) = send_to_live_session(
            bin,
            SessionCommand::Send { text: "hi".to_owned(), command_id: Some(command_id) },
        )
        .await;
        let (ok, error) = spawn_result(&mut rx).await;
        shutdown.cancel();
        assert!(!ok, "an unanswered turn/start must not resolve as success");
        let error = error.unwrap();
        assert!(error.contains("turn/start"), "{error}");
    }
}
