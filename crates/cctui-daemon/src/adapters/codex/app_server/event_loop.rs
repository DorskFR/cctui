use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cctui_proto::adapter::{AdapterEvent, EndReason, SessionMeta};
use cctui_proto::codex_catalog::{CodexModel, CodexModelCatalog};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::config::launch_overrides;
use super::diagnose::{DiagnoseRings, stderr_tail};
use super::registry::{CodexLiveSnapshot, SessionCommand, SessionRecord};
use super::requests::{
    initialize_req, initialized_notification, record_codex_version, thread_info,
    turn_interrupt_req, turn_start_req, turn_steer_req,
};
use super::rpc::{
    ApprovalKind, HANDSHAKE_TIMEOUT, ID_INITIALIZE, ID_THREAD_START, Incoming, NO_TURN_IN_FLIGHT,
    PendingRpcs, RPC_TIMEOUT, RUN_BASE, RpcSink, RpcSource, RpcStdin, approval_reply, classify,
    user_input_reply,
};
use super::session::{
    CodexSession, SIGTERM, SessionLaunch, SpawnAck, child_linkage, kill_child, partition_drained,
    record_model_override, removes_record, set_thread_name, spawn_resumed_session,
    subagent_started_event, terminate_child, unknown_model,
};
use super::thread_state::{
    ActiveTurn, ItemAccumulator, PromptDispatch, SteerRecovery, TurnLifecycle, prompt_dispatch,
    steer_recovery, turn_lifecycle,
};
use crate::adapters::codex::model_list;

impl CodexSession {
    #[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
    pub(super) async fn run_inner(
        &self,
        ack: &mut SpawnAck,
        rings: &Arc<DiagnoseRings>,
    ) -> Result<()> {
        let cwd_path = std::path::Path::new(&self.cwd);
        if !cwd_path.is_dir() {
            anyhow::bail!("spawn: working_dir does not exist or is not a directory: {}", self.cwd);
        }

        let (mut child, mut stdin, mut lines, stderr_drain, thread_config) =
            if let Some((wire, config)) = self.open_shared().await {
                let stdin = RpcStdin { inner: RpcSink::Shared(wire.sink), rings: rings.clone() };
                (None, stdin, RpcSource::Shared(wire.frames), None, config)
            } else {
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
                (Some(spawned), stdin, lines, stderr_drain, self.thread_config(false))
            };

        // Handshake: initialize → thread/start or thread/resume.
        let mut pending_rpcs = PendingRpcs::default();
        let handshake_deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        pending_rpcs.insert(ID_INITIALIZE, "initialize", None, handshake_deadline);
        // EPIPE here means codex already died (auth/config errors exit at
        // once); let the stdout EOF below reach the epilogue, which reports the
        // exit status with the stderr tail.
        if let Err(e) = stdin.send(&initialize_req()).await {
            tracing::warn!(%e, "codex: initialize write failed");
        }
        // `model/list` issued before the thread request to reject an unknown
        // `-c model=` up front; while set, that request is part of the handshake.
        let mut validating_model = false;
        let mut catalog_sent = false;
        let mut handshake_failed = false;
        let mut local_id = String::new();
        let mut codex_version: Option<String> = None;
        let mut rollout_path: Option<String> = None;
        let mut next_id = RUN_BASE;
        // request_id (surfaced to TUI) → (rpc_id echoed to codex, decision kind).
        let mut pending_approvals: HashMap<String, (Value, ApprovalKind)> = HashMap::new();
        // Parked `item/tool/requestUserInput` requests: the next user
        // reply answers the oldest one (codex blocks the turn on it) rather than
        // starting a fresh turn.
        let mut pending_questions: VecDeque<(Value, Vec<String>)> = VecDeque::new();
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<SessionCommand>(32);
        let mut registered = false;
        // Set when the session is terminated on purpose (daemon shutdown or a
        // Kill command) so the epilogue reports `Killed` rather than treating
        // the non-zero exit as a crash.
        let mut killed = false;
        let mut retry_after_hibernate: Option<SessionCommand> = None;
        let mut active_turn = ActiveTurn::default();
        let mut items = ItemAccumulator::default();
        // In-place model/effort override. A SetModel records it here;
        // every subsequent `turn/start` carries it so codex adopts it as the
        // later default. Left `None` at launch — the spawn-time `-c model=`/
        // `-c model_reasoning_effort=` flags already seed the initial turns.
        let mut override_model: Option<String> = None;
        let mut override_effort: Option<String> = None;
        let mut steer_texts: HashMap<i64, String> = HashMap::new();
        // `model/list` pages accumulated over this session's
        // authenticated connection; the counter bounds `nextCursor` following.
        let mut model_catalog: Vec<CodexModel> = Vec::new();
        let mut model_catalog_pages: usize = 0;
        let mut sweep = tokio::time::interval(Duration::from_secs(1));
        sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let reexec = crate::selfupdate::reexec_prep();
        let mut reexec_exit = false;

        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => {
                    kill_child(&mut child);
                    killed = true;
                    break;
                }
                // SIGTERM (not kill) so codex flushes its rollout before the
                // re-exec; the record stays so the new daemon can resume.
                () = reexec.cancelled() => {
                    if let Some(child) = child.as_mut() {
                        terminate_child(child, Some(SIGTERM));
                    }
                    reexec_exit = true;
                    break;
                }
                _ = sweep.tick() => {
                    let mut handshake_dead = false;
                    for (id, pending) in pending_rpcs.expire(Instant::now()) {
                        tracing::warn!(rpc_id = id, method = %pending.method, "codex: JSON-RPC request timed out");
                        if let Some(command_id) = pending.command_id {
                            let _ = self.events
                                .send(AdapterEvent::CommandResult {
                                    command_id,
                                    ok: false,
                                    error: Some(format!("codex {} timed out", pending.method)),
                                })
                                .await;
                        }
                        if pending.is_handshake() || validating_model {
                            let detail = format!(
                                "codex {} timed out after {}s{}",
                                pending.method,
                                HANDSHAKE_TIMEOUT.as_secs(),
                                stderr_tail(rings)
                            );
                            self.fail_handshake(ack, &detail).await;
                            handshake_dead = true;
                        }
                    }
                    if handshake_dead {
                        handshake_failed = true;
                        kill_child(&mut child);
                        break;
                    }
                }
                cmd = cmd_rx.recv(), if registered => {
                    match cmd {
                        Some(SessionCommand::Permission { request_id, allow }) => {
                            if let Some((rpc_id, kind)) = pending_approvals.remove(&request_id) {
                                if let Err(e) =
                                    stdin.send(&approval_reply(&rpc_id, kind, allow)).await
                                {
                                    tracing::warn!(%e, "codex: approval write failed; ending session");
                                    break;
                                }
                            } else {
                                tracing::warn!(%request_id, "codex: no pending approval for response");
                            }
                        }
                        Some(SessionCommand::Send { text, command_id }) => {
                            if let Some((rpc_id, question_ids)) = pending_questions.pop_front() {
                                let reply = user_input_reply(&rpc_id, &question_ids, &text);
                                if let Err(e) = stdin.send(&reply).await {
                                    tracing::warn!(%e, "codex: requestUserInput answer write failed; ending session");
                                    break;
                                }
                                if let Some(command_id) = command_id {
                                    let _ = self.events
                                        .send(AdapterEvent::CommandResult { command_id, ok: true, error: None })
                                        .await;
                                }
                                self.events
                                    .send(AdapterEvent::AskResolved { local_id: local_id.clone() })
                                    .await
                                    .ok();
                                continue;
                            }
                            let (req, method) = match prompt_dispatch(&active_turn) {
                                PromptDispatch::Steer { turn_id } => {
                                    steer_texts.insert(next_id, text.clone());
                                    (turn_steer_req(next_id, &local_id, &turn_id, &text, &[]), "turn/steer")
                                }
                                PromptDispatch::Start => {
                                    (
                                        turn_start_req(
                                            next_id,
                                            &local_id,
                                            &text,
                                            &[],
                                            override_model.as_deref(),
                                            override_effort.as_deref(),
                                        ),
                                        "turn/start",
                                    )
                                }
                            };
                            pending_rpcs.insert(next_id, method, command_id, Instant::now() + RPC_TIMEOUT);
                            next_id += 1;
                            // A write failure here means the app-server is gone
                            // — remember the turn and let the epilogue revive
                            // the thread if this was a clean hibernation exit.
                            if let Err(e) = stdin.send(&req).await {
                                tracing::warn!(%e, "codex: turn dispatch write failed; ending session");
                                steer_texts.remove(&(next_id - 1));
                                pending_rpcs.remove(next_id - 1);
                                retry_after_hibernate = Some(SessionCommand::Send { text, command_id });
                                break;
                            }
                        }
                        Some(SessionCommand::Rename { name }) => {
                            if let Err(e) = set_thread_name(
                                &mut stdin,
                                &mut next_id,
                                &mut pending_rpcs,
                                &local_id,
                                &name,
                                &self.events,
                                &self.registry,
                            )
                            .await
                            {
                                tracing::warn!(%e, "codex: thread/name/set write failed; ending session");
                                retry_after_hibernate = Some(SessionCommand::Rename { name });
                                break;
                            }
                        }
                        Some(SessionCommand::Kill { signal }) => {
                            if let Some(turn_id) = active_turn.id() {
                                let req = turn_interrupt_req(next_id, &local_id, turn_id);
                                let _ = stdin.send(&req).await;
                            }
                            if let Some(child) = child.as_mut() {
                                terminate_child(child, signal);
                            }
                            killed = true;
                            break;
                        }
                        Some(SessionCommand::Interrupt { command_id }) => {
                            // Keep-alive interrupt: abort the turn but
                            // leave the app-server running so the session keeps
                            // going — unlike Kill, we do NOT terminate the child.
                            let Some(turn_id) = active_turn.id() else {
                                if let Some(command_id) = command_id {
                                    let _ = self.events
                                        .send(AdapterEvent::CommandResult {
                                            command_id,
                                            ok: false,
                                            error: Some(NO_TURN_IN_FLIGHT.to_owned()),
                                        })
                                        .await;
                                }
                                continue;
                            };
                            let req = turn_interrupt_req(next_id, &local_id, turn_id);
                            pending_rpcs.insert(next_id, "turn/interrupt", command_id, Instant::now() + RPC_TIMEOUT);
                            next_id += 1;
                            if let Err(e) = stdin.send(&req).await {
                                tracing::warn!(%e, "codex: turn/interrupt write failed; ending session");
                                break;
                            }
                        }
                        Some(SessionCommand::SetModel { model, effort, command_id }) => {
                            record_model_override(
                                &mut override_model,
                                &mut override_effort,
                                model.as_deref(),
                                effort.as_deref(),
                                &local_id,
                                &self.events,
                                &self.registry,
                                command_id,
                            )
                            .await;
                        }
                        Some(SessionCommand::Diagnose { reply }) => {
                            let snapshot = CodexLiveSnapshot {
                                codex_version: codex_version.clone(),
                                pid: child.as_ref().and_then(tokio::process::Child::id),
                                active_turn_id: active_turn.id().map(str::to_owned),
                                pending_rpc_methods: pending_rpcs.pending_methods(),
                                protocol_errors: rings.protocol_errors_with_shared(),
                                stderr_tail: rings.stderr_tail(),
                                rpc_tail: rings.rpc_tail_with_shared(),
                                rollout_path: rollout_path.clone(),
                                rollout_size_bytes: rollout_path
                                    .as_ref()
                                    .and_then(|p| std::fs::metadata(p).ok())
                                    .map(|m| m.len()),
                            };
                            let _ = reply.send(snapshot).await;
                        }
                        None => break,
                    }
                }
                line = lines.next_line() => {
                    // EOF (`Ok(None)`) or a read error both mean the app-server
                    // is gone; break and let the epilogue classify the exit.
                    let line = match line {
                        Ok(Some(line)) => line,
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!(%e, "codex: stdout read error; ending session");
                            break;
                        }
                    };
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                        tracing::debug!(line = %trimmed, "codex: non-JSON line");
                        continue;
                    };
                    rings.note_rpc("in", &value);
                    if child.is_none()
                        && let Some(event) = subagent_started_event(&local_id, &value)
                    {
                        self.events.send(event).await.ok();
                        continue;
                    }
                    if let Some(ev) = turn_lifecycle(&value) {
                        active_turn.apply(&ev);
                        // A spawned child's caller is parked on turn
                        // completion; codex has no state.json to flip, so emit
                        // the done status childwatch classifies on. Scoped to
                        // spawn_key sessions — observed threads keep the
                        // successful-turn-is-ignored behavior.
                        if matches!(ev, TurnLifecycle::Completed { .. })
                            && self.spawn_key.is_some()
                            && !local_id.is_empty()
                        {
                            self.events
                                .send(AdapterEvent::Status {
                                    local_id: local_id.clone(),
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
                    items.note(&value);
                    let value = items.enrich_completed(value);
                    match classify(&local_id, &value) {
                        Incoming::Response { id, value } => {
                            let Some((pending, outcome)) = pending_rpcs.resolve(id, &value) else {
                                tracing::debug!(rpc_id = id, "codex: response for unknown request id");
                                continue;
                            };
                            if let Err(ref e) = outcome {
                                rings.note_protocol_error(&format!("{}: {e}", pending.method));
                            }
                            if outcome.is_ok() && let Some(command_id) = pending.command_id {
                                let _ = self.events
                                    .send(AdapterEvent::CommandResult { command_id, ok: true, error: None })
                                    .await;
                            }
                            match (pending.method.as_str(), outcome) {
                        ("initialize", Ok(_)) => {
                            codex_version = record_codex_version(&value);
                            // Complete the documented handshake before any
                            // thread request: the server treats
                            // `thread/*` sent before `initialized` as premature.
                            stdin.send(&initialized_notification()).await?;
                            if self.cfg.model_catalog && self.cfg.model.is_some() {
                                validating_model = true;
                                pending_rpcs.insert(next_id, "model/list", None, handshake_deadline);
                                stdin.send(&model_list::model_list_req(next_id, None))
                                    .await?;
                                next_id += 1;
                            } else {
                                let (req, method) = self.thread_request(&thread_config);
                                pending_rpcs.insert(ID_THREAD_START, method, None, handshake_deadline);
                                stdin.send(&req).await?;
                            }
                        }
                        ("thread/start" | "thread/resume" | "thread/fork", Ok(result)) => {
                            let Some(info) = thread_info(&result) else {
                                anyhow::bail!("codex thread/start response missing thread id");
                            };
                            local_id.clone_from(&info.thread_id);
                            rollout_path.clone_from(&info.rollout_path);
                            let (parent_local_id, relation) =
                                child_linkage(&self.launch, self.parent_local_id.as_deref());
                            // A `CctuiAgent` call from this session arrives keyed
                            // by the launch key baked into the relay argv; the
                            // thread id it really is only exists now.
                            if let Some(agent_mcp) = &self.agent_mcp {
                                crate::agenttool::bind_session_alias(
                                    agent_mcp.session_key(),
                                    &local_id,
                                );
                                crate::adapters::agent_mcp::remember(&local_id, agent_mcp);
                            }
                            self.events
                                .send(AdapterEvent::SessionStarted {
                                    local_id: local_id.clone(),
                                    meta: SessionMeta {
                                        working_dir: info.cwd.or_else(|| Some(self.cwd.clone())),
                                        parent_local_id,
                                        extra: json!({
                                            "source": "codex-app-server",
                                            "rollout_path": info.rollout_path,
                                            "codex_version": codex_version,
                                            "spawn_key": self.spawn_key,
                                            "relation": relation,
                                        }),
                                    },
                                })
                                .await
                                .ok();
                            let remembered_name = match &self.launch {
                                SessionLaunch::Fresh { name, .. }
                                | SessionLaunch::Fork { name, .. } => name.clone(),
                                SessionLaunch::Resume { .. } => self
                                    .registry
                                    .lock()
                                    .await
                                    .get(&local_id)
                                    .and_then(|r| r.name.clone()),
                            };
                            self.registry.lock().await.insert(
                                local_id.clone(),
                                SessionRecord {
                                    cfg: self.cfg.clone(),
                                    cwd: self.cwd.clone(),
                                    name: remembered_name.clone(),
                                    env: self.env.clone(),
                                    spawn_relay: self.agent_mcp.is_some(),
                                },
                            );
                            crate::adapters::codex::persist::save(&self.registry).await;
                            self.live.lock().await.insert(local_id.clone(), cmd_tx.clone());
                            registered = true;
                            ack.ok().await;
                            // refresh the account/machine model catalog
                            // over THIS authenticated connection (the gateway
                            // credential is in env), so gateway-only machines get
                            // the current remote list instead of a stale
                            // unauthenticated fallback. Best-effort: a failure is
                            // logged, never fatal to the session.
                            if self.cfg.model_catalog && !catalog_sent {
                                pending_rpcs.insert(
                                    next_id,
                                    "model/list",
                                    None,
                                    Instant::now() + RPC_TIMEOUT,
                                );
                                if let Err(e) =
                                    stdin.send(&model_list::model_list_req(next_id, None))
                                        .await
                                {
                                    tracing::debug!(%e, "codex: model/list write failed");
                                    pending_rpcs.resolve(next_id, &json!({}));
                                }
                                next_id += 1;
                            }
                            // Surface the configured model + reasoning effort so
                            // the session list shows them (claude gets this for
                            // free via state.json; codex has no equivalent feed).
                            // Emit when either is known.
                            let model = self.cfg.model.clone();
                            let effort = self.cfg.reasoning_effort.clone();
                            if model.is_some() || effort.is_some() {
                                self.events
                                    .send(AdapterEvent::Status {
                                        local_id: local_id.clone(),
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

                            let mut end_after_initial = false;
                            match &self.launch {
                                SessionLaunch::Fresh { name, prompt, attachments }
                                | SessionLaunch::Fork { name, prompt, attachments, .. } => {
                                    if let Some(name) = name.as_deref() {
                                        let result = set_thread_name(
                                            &mut stdin,
                                            &mut next_id,
                                            &mut pending_rpcs,
                                            &local_id,
                                            name,
                                            &self.events,
                                            &self.registry,
                                        )
                                        .await;
                                        if let Err(e) = result {
                                            tracing::warn!(%e, "codex: initial thread/name/set failed");
                                            retry_after_hibernate =
                                                Some(SessionCommand::Rename { name: name.to_owned() });
                                            end_after_initial = true;
                                        }
                                    }
                                    // Send the first turn when there is a prompt OR
                                    // staged attachments — an image-only
                                    // spawn carries no prompt text but must still
                                    // reach codex as a `localImage` turn input.
                                    if !end_after_initial
                                        && (prompt.is_some() || !attachments.is_empty())
                                    {
                                        let prompt_text = prompt.as_deref().unwrap_or("");
                                        let req = turn_start_req(
                                            next_id,
                                            &local_id,
                                            prompt_text,
                                            attachments,
                                            override_model.as_deref(),
                                            override_effort.as_deref(),
                                        );
                                        pending_rpcs.insert(
                                            next_id,
                                            "turn/start",
                                            None,
                                            Instant::now() + RPC_TIMEOUT,
                                        );
                                        next_id += 1;
                                        if let Err(e) = stdin.send(&req).await {
                                            tracing::warn!(%e, "codex: initial prompt write failed; ending session");
                                            retry_after_hibernate =
                                                Some(SessionCommand::Send { text: prompt_text.to_owned(), command_id: None });
                                            end_after_initial = true;
                                        }
                                    }
                                }
                                SessionLaunch::Resume { initial_commands, .. } => {
                                    for command in initial_commands.clone() {
                                        match command {
                                            SessionCommand::Send { text, command_id } => {
                                                let req = turn_start_req(
                                                    next_id,
                                                    &local_id,
                                                    &text,
                                                    &[],
                                                    override_model.as_deref(),
                                                    override_effort.as_deref(),
                                                );
                                                pending_rpcs.insert(
                                                    next_id,
                                                    "turn/start",
                                                    command_id,
                                                    Instant::now() + RPC_TIMEOUT,
                                                );
                                                next_id += 1;
                                                if let Err(e) = stdin.send(&req).await {
                                                    tracing::warn!(%e, "codex: resumed turn/start write failed");
                                                    pending_rpcs.remove(next_id - 1);
                                                    retry_after_hibernate =
                                                        Some(SessionCommand::Send { text, command_id });
                                                    end_after_initial = true;
                                                    break;
                                                }
                                            }
                                            SessionCommand::Rename { name } => {
                                                if let Err(e) = set_thread_name(
                                                    &mut stdin,
                                                    &mut next_id,
                                                    &mut pending_rpcs,
                                                    &local_id,
                                                    &name,
                                                    &self.events,
                                                    &self.registry,
                                                )
                                                .await
                                                {
                                                    tracing::warn!(%e, "codex: resumed thread/name/set write failed");
                                                    retry_after_hibernate =
                                                        Some(SessionCommand::Rename { name });
                                                    end_after_initial = true;
                                                    break;
                                                }
                                            }
                                            SessionCommand::SetModel { model, effort, command_id } => {
                                                record_model_override(
                                                    &mut override_model,
                                                    &mut override_effort,
                                                    model.as_deref(),
                                                    effort.as_deref(),
                                                    &local_id,
                                                    &self.events,
                                                    &self.registry,
                                                    command_id,
                                                )
                                                .await;
                                            }
                                            other => {
                                                tracing::warn!(?other, "codex: ignoring non-resumable initial command");
                                            }
                                        }
                                    }
                                }
                            }
                            if end_after_initial {
                                break;
                            }
                        }
                        (method, Err(err)) if pending.is_handshake() => {
                            tracing::error!(%err, %method, "codex: handshake request failed; ending session");
                            let detail = format!("codex {method}: {err}{}", stderr_tail(rings));
                            self.fail_handshake(ack, &detail).await;
                            handshake_failed = true;
                            kill_child(&mut child);
                            break;
                        }
                        ("model/list", Err(err)) if validating_model => {
                            // The catalog is best-effort; codex still rejects a bad
                            // model itself on the first turn.
                            tracing::debug!(%err, "codex: pre-start model/list failed; skipping model check");
                            validating_model = false;
                            model_catalog.clear();
                            let (req, method) = self.thread_request(&thread_config);
                            pending_rpcs.insert(ID_THREAD_START, method, None, handshake_deadline);
                            stdin.send(&req).await?;
                        }
                        ("model/list", Ok(result)) if validating_model => {
                            model_catalog.extend(model_list::parse_model_list(&result));
                            model_catalog_pages += 1;
                            if let model_list::PageStep::Next { cursor } =
                                model_list::page_step(model_catalog_pages, &result)
                            {
                                pending_rpcs.insert(next_id, "model/list", None, handshake_deadline);
                                stdin.send(
                                    &model_list::model_list_req(next_id, Some(&cursor)),
                                )
                                .await?;
                                next_id += 1;
                                continue;
                            }
                            validating_model = false;
                            let catalog =
                                CodexModelCatalog {
                                    models: std::mem::take(&mut model_catalog),
                                    client_version: None,
                                };
                            if let Some(warning) = unknown_model(self.cfg.model.as_deref(), &catalog)
                            {
                                tracing::warn!(%warning, "codex: spawning anyway");
                            }
                            catalog_sent = true;
                            self.events.send(AdapterEvent::CodexModels { catalog }).await.ok();
                            let (req, method) = self.thread_request(&thread_config);
                            pending_rpcs.insert(ID_THREAD_START, method, None, handshake_deadline);
                            stdin.send(&req).await?;
                        }
                        ("model/list", Ok(result)) => {
                            model_catalog.extend(model_list::parse_model_list(&result));
                            model_catalog_pages += 1;
                            match model_list::page_step(model_catalog_pages, &result) {
                                model_list::PageStep::Next { cursor } => {
                                    pending_rpcs.insert(
                                        next_id,
                                        "model/list",
                                        None,
                                        Instant::now() + RPC_TIMEOUT,
                                    );
                                    if let Err(e) = stdin.send(
                                        &model_list::model_list_req(next_id, Some(&cursor)),
                                    )
                                    .await
                                    {
                                        tracing::debug!(%e, "codex: model/list page write failed");
                                        pending_rpcs.resolve(next_id, &json!({}));
                                    }
                                    next_id += 1;
                                }
                                model_list::PageStep::Done => {
                                    let catalog = CodexModelCatalog {
                                        models: std::mem::take(&mut model_catalog),
                                        client_version: None,
                                    };
                                    self.events
                                        .send(AdapterEvent::CodexModels { catalog })
                                        .await
                                        .ok();
                                }
                            }
                        }
                        ("model/list", Err(err)) => {
                            tracing::debug!(%err, "codex: model/list refresh failed");
                            model_catalog.clear();
                        }
                        ("turn/steer", Ok(_)) => {
                            steer_texts.remove(&id);
                        }
                        ("turn/steer", Err(err)) => {
                            let text = steer_texts.remove(&id);
                            match (steer_recovery(&err), text) {
                                (SteerRecovery::FallbackToStart, Some(text)) => {
                                    active_turn.clear();
                                    tracing::info!(%err, "codex: turn/steer stale; falling back to turn/start");
                                    let req = turn_start_req(
                                        next_id,
                                        &local_id,
                                        &text,
                                        &[],
                                        override_model.as_deref(),
                                        override_effort.as_deref(),
                                    );
                                    pending_rpcs.insert(next_id, "turn/start", pending.command_id, Instant::now() + RPC_TIMEOUT);
                                    next_id += 1;
                                    if let Err(e) = stdin.send(&req).await {
                                        tracing::warn!(%e, "codex: turn/start fallback write failed; ending session");
                                        pending_rpcs.remove(next_id - 1);
                                        retry_after_hibernate = Some(SessionCommand::Send { text, command_id: pending.command_id });
                                        break;
                                    }
                                }
                                (recovery, _) => {
                                    tracing::warn!(%err, ?recovery, "codex: turn/steer rejected");
                                    if let Some(command_id) = pending.command_id {
                                        let _ = self.events
                                            .send(AdapterEvent::CommandResult {
                                                command_id,
                                                ok: false,
                                                error: Some(err.clone()),
                                            })
                                            .await;
                                    }
                                    self.events
                                        .send(AdapterEvent::Status {
                                            local_id: local_id.clone(),
                                            tempo: None,
                                            state: Some("failed".to_owned()),
                                            detail: Some(err),
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
                            }
                        }
                        (method, Err(err)) => {
                            tracing::warn!(%err, %method, "codex: JSON-RPC request failed");
                            if let Some(command_id) = pending.command_id {
                                let _ = self.events
                                    .send(AdapterEvent::CommandResult {
                                        command_id,
                                        ok: false,
                                        error: Some(err.clone()),
                                    })
                                    .await;
                            }
                            if method == "turn/start" {
                                self.events
                                    .send(AdapterEvent::Status {
                                        local_id: local_id.clone(),
                                        tempo: None,
                                        state: Some("failed".to_owned()),
                                        detail: Some(err),
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
                        }
                        (_, Ok(_)) => {}
                            }
                        }
                        Incoming::Approval { rpc_id, request_id, tool, kind, input } => {
                            pending_approvals.insert(request_id.clone(), (rpc_id, kind));
                            self.events
                                .send(AdapterEvent::PermissionRequest {
                                    local_id: local_id.clone(),
                                    request_id,
                                    tool,
                                    input,
                                })
                                .await
                                .ok();
                        }
                        Incoming::Question { rpc_id, question, questions, question_ids } => {
                            pending_questions.push_back((rpc_id, question_ids));
                            self.events
                                .send(AdapterEvent::AskQuestion {
                                    local_id: local_id.clone(),
                                    question,
                                    questions: Some(questions),
                                    preamble: None,
                                })
                                .await
                                .ok();
                        }
                        Incoming::Decline { reply } => {
                            if let Err(e) = stdin.send(&reply).await {
                                tracing::warn!(%e, "codex: decline write failed; ending session");
                                break;
                            }
                        }
                        Incoming::Event(evt) => {
                            self.events.send(evt).await.ok();
                        }
                        Incoming::Traced { method, reason } => {
                            tracing::trace!(%method, reason, "codex notification consumed out-of-band");
                        }
                        Incoming::Unhandled { method, event } => {
                            tracing::warn!(%method, "unhandled codex notification");
                            rings.note_protocol_error(&format!(
                                "unhandled codex notification {method}"
                            ));
                            self.events.send(event).await.ok();
                        }
                        Incoming::Ignored => {}
                    }
                }
            }
        }

        // The pump has broken: drop the live sender NOW so new commands take
        // the Resume path instead of landing in this dead channel's buffer,
        // then drain whatever was already buffered.
        if !local_id.is_empty() {
            self.live.lock().await.remove(&local_id);
        }
        cmd_rx.close();
        let mut drained: Vec<SessionCommand> = Vec::new();
        while let Ok(cmd) = cmd_rx.try_recv() {
            drained.push(cmd);
        }

        for (id, pending) in pending_rpcs.drain() {
            tracing::warn!(rpc_id = id, method = %pending.method, "codex: cancelling pending request — app-server gone");
            if let Some(command_id) = pending.command_id {
                let _ = self
                    .events
                    .send(AdapterEvent::CommandResult {
                        command_id,
                        ok: false,
                        error: Some(format!(
                            "codex {}: app-server exited before responding",
                            pending.method
                        )),
                    })
                    .await;
            }
        }

        // Paths that hand a request to a retry (`retry_after_hibernate`) must
        // `pending_rpcs.remove` it first, or it is failed here as well.
        for (_, pending) in pending_rpcs.drain() {
            if let Some(command_id) = pending.command_id {
                self.events
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

        // Reap the child and classify why the session ended. An abnormal exit
        // that we did not request is surfaced as `Crashed` with the captured
        // stderr tail — the diagnostic for the macOS "randomly dies" report.
        let status = match child.as_mut() {
            Some(child) => Some(child.wait().await),
            None => None,
        };
        // Let the drain catch codex's final lines before any tail is read.
        if let Some(drain) = stderr_drain {
            let _ = tokio::time::timeout(Duration::from_secs(1), drain).await;
        }
        if local_id.is_empty() {
            if reexec_exit || handshake_failed {
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
        if reexec_exit {
            return Ok(());
        }
        let (mut retry, dropped, drained_kill) = partition_drained(drained);
        let reason = if killed || drained_kill {
            Some(EndReason::Killed)
        } else {
            match status {
                None => Some(EndReason::Crashed {
                    detail: "shared codex app-server lost the thread and could not rejoin it"
                        .to_owned(),
                }),
                Some(Ok(s)) if s.success() => None,
                Some(Ok(s)) => Some(EndReason::Crashed {
                    detail: format!("codex app-server exited ({s}){}", stderr_tail(rings)),
                }),
                Some(Err(e)) => Some(EndReason::Crashed {
                    detail: format!("codex app-server wait failed: {e}"),
                }),
            }
        };
        if let Some(reason) = reason {
            if let EndReason::Crashed { detail } = &reason {
                tracing::error!(%detail, "codex app-server session crashed");
            }
            // A crash keeps the durable record: `thread/resume` still works,
            // so the next command revives the thread instead of going Missing.
            if removes_record(&reason) {
                self.registry.lock().await.remove(&local_id);
                crate::adapters::codex::persist::save(&self.registry).await;
            }
            for cmd in retry.into_iter().chain(dropped) {
                self.fail_dropped_command(&local_id, &cmd).await;
            }
            self.events
                .send(AdapterEvent::SessionEnded { local_id: local_id.clone(), reason })
                .await
                .ok();
        } else {
            self.events
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
                self.fail_dropped_command(&local_id, &cmd).await;
            }
            if let Some(command) = retry_after_hibernate {
                retry.insert(0, command);
            }
            if !retry.is_empty()
                && let Some(record) = self.registry.lock().await.get(&local_id).cloned()
            {
                spawn_resumed_session(
                    record,
                    &local_id,
                    retry,
                    self.events.clone(),
                    self.live.clone(),
                    self.registry.clone(),
                    self.shutdown.clone(),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::config::AppServerConfig;
    use super::super::registry::{LiveSessionRegistry, SessionRegistry};
    use super::*;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

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
