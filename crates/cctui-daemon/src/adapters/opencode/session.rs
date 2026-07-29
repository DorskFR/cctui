//! One `opencode serve` subprocess per cctui session.
//!
//! Serve-per-session (not one shared server) because the provider credential is
//! a per-session gateway token injected through the child's env, and opencode
//! resolves `{env:…}` once per server process — a shared server could only ever
//! hold one session's credential.

use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use cctui_proto::adapter::{AdapterEvent, EndReason, SessionMeta};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::client::{
    CreateSession, OpenCodeClient, Part, PartInput, PromptModelRef, PromptRequest, SessionModelRef,
};
use super::config::{ModelRef, SessionHome, session_config};
use super::events::{OcEvent, SseDecoder, StatusKind, status_kind};
use super::normalize::{self, Kind};

/// A just-started server accepts the connection before its handlers are wired
/// and never answers that first request; probes are bounded so the startup poll
/// keeps retrying instead of blocking on it.
const HEALTH_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

pub type LiveRegistry = Arc<Mutex<HashMap<String, mpsc::Sender<SessionCommand>>>>;

#[derive(Debug)]
pub enum SessionCommand {
    Prompt { session_id: String, text: String },
    Kill { session_id: String },
    Fork { parent: String, prompt: Option<String>, name: Option<String>, command_id: Option<Uuid> },
    Permission { session_id: String, request_id: String, allow: bool },
}

/// Adapter-level knobs from `adapters_enabled.config`.
#[derive(Debug, Clone)]
pub struct OpenCodeConfig {
    pub bin: String,
    pub hostname: String,
    pub state_root: std::path::PathBuf,
    pub startup_timeout_ms: u64,
    pub default_agent: Option<String>,
    pub default_model: Option<String>,
}

impl Default for OpenCodeConfig {
    fn default() -> Self {
        Self {
            bin: "opencode".to_owned(),
            hostname: "127.0.0.1".to_owned(),
            state_root: std::path::PathBuf::from("/tmp/cctui-opencode"),
            startup_timeout_ms: 60_000,
            default_agent: None,
            default_model: None,
        }
    }
}

impl OpenCodeConfig {
    #[must_use]
    pub fn from_value(v: &serde_json::Value) -> Self {
        let d = Self::default();
        Self {
            bin: str_of(v, "bin").unwrap_or(d.bin),
            hostname: str_of(v, "hostname").unwrap_or(d.hostname),
            state_root: str_of(v, "state_root").map_or(d.state_root, std::path::PathBuf::from),
            startup_timeout_ms: v
                .get("startup_timeout_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(d.startup_timeout_ms),
            default_agent: str_of(v, "agent"),
            default_model: str_of(v, "model"),
        }
    }
}

fn str_of(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Probe the binary. The version is not enforced (opencode ships several
/// releases a week) but a mismatch against the pinned one is logged, and an
/// absent binary fails loudly instead of at the first spawn.
pub async fn probe_version(bin: &str) -> Result<String> {
    let out = Command::new(bin)
        .arg("--version")
        .env("PATH", crate::childenv::child_path())
        .output()
        .await
        .with_context(|| format!("`{bin} --version` failed to run — is opencode installed?"))?;
    if !out.status.success() {
        anyhow::bail!("`{bin} --version` exited {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub struct SpawnParams {
    pub cfg: OpenCodeConfig,
    pub key: String,
    pub cwd: String,
    pub env: std::collections::BTreeMap<String, String>,
    pub prompt: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    pub agent: Option<String>,
    pub attachments: Vec<String>,
    pub command_id: Option<Uuid>,
    /// Set for a `CctuiAgent` child so the session registers nested under the
    /// caller instead of as a top-level session.
    pub parent_local_id: Option<String>,
}

pub struct OpenCodeSession {
    params: SpawnParams,
    events: mpsc::Sender<AdapterEvent>,
    live: LiveRegistry,
    shutdown: CancellationToken,
    commands: mpsc::Receiver<SessionCommand>,
    commands_tx: mpsc::Sender<SessionCommand>,
    owned: HashSet<String>,
    roles: HashMap<String, String>,
    emitted_parts: HashSet<String>,
    emitted_usage: HashSet<String>,
}

impl OpenCodeSession {
    #[must_use]
    pub fn new(
        params: SpawnParams,
        events: mpsc::Sender<AdapterEvent>,
        live: LiveRegistry,
        shutdown: CancellationToken,
    ) -> Self {
        let (commands_tx, commands) = mpsc::channel(64);
        Self {
            params,
            events,
            live,
            shutdown,
            commands,
            commands_tx,
            owned: HashSet::new(),
            roles: HashMap::new(),
            emitted_parts: HashSet::new(),
            emitted_usage: HashSet::new(),
        }
    }

    pub async fn run(mut self) {
        let command_id = self.params.command_id.take();
        match self.run_inner(command_id).await {
            Ok(()) => {}
            Err(err) => {
                tracing::error!(%err, "opencode session ended in error");
                if let Some(command_id) = command_id {
                    let _ = self
                        .events
                        .send(AdapterEvent::CommandResult {
                            command_id,
                            ok: false,
                            error: Some(err.to_string()),
                        })
                        .await;
                }
            }
        }
        for id in self.owned.clone() {
            self.live.lock().await.remove(&id);
            let _ = self
                .events
                .send(AdapterEvent::SessionEnded { local_id: id, reason: EndReason::Completed })
                .await;
        }
    }

    #[allow(clippy::too_many_lines, clippy::similar_names, clippy::cognitive_complexity)]
    async fn run_inner(&mut self, command_id: Option<Uuid>) -> Result<()> {
        let cwd = std::path::PathBuf::from(&self.params.cwd);
        anyhow::ensure!(cwd.is_dir(), "working_dir does not exist: {}", self.params.cwd);

        let version = probe_version(&self.params.cfg.bin).await?;
        if !version.contains(super::client::OPENCODE_PINNED_VERSION) {
            tracing::warn!(
                %version,
                pinned = super::client::OPENCODE_PINNED_VERSION,
                "opencode version differs from the pinned one"
            );
        }

        let model = self
            .params
            .model
            .as_deref()
            .or(self.params.cfg.default_model.as_deref())
            .and_then(ModelRef::parse);
        let home = SessionHome::under(&self.params.cfg.state_root, &self.params.key);
        home.write_config(&session_config(model.as_ref(), &self.params.env))?;

        let port = free_port(&self.params.cfg.hostname)?;
        let password = Uuid::new_v4().to_string();

        let mut cmd = Command::new(&self.params.cfg.bin);
        cmd.arg("serve")
            .arg("--hostname")
            .arg(&self.params.cfg.hostname)
            .arg("--port")
            .arg(port.to_string());
        for (k, v) in &self.params.env {
            cmd.env(k, v);
        }
        for (k, v) in home.env() {
            cmd.env(k, v);
        }
        cmd.env("OPENCODE_SERVER_PASSWORD", &password);
        crate::childenv::ScrubChildEnv::scrub_child_env(&mut cmd);
        // Own process group: `opencode serve` runs the model turn in children of
        // its own, and killing only the direct child leaves them generating (and
        // spending) — the whole group has to go.
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd
            .current_dir(&cwd)
            .env("PATH", crate::childenv::child_path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn `{} serve`", self.params.cfg.bin))?;
        drain_child_logs(&mut child);

        let client = Arc::new(OpenCodeClient::new(
            format!("http://{}:{port}", self.params.cfg.hostname),
            password,
        ));
        self.await_health(&client).await?;

        let session = client
            .create_session(&CreateSession {
                title: self.params.name.clone(),
                agent: self.agent(),
                model: model.as_ref().map(|m| SessionModelRef {
                    id: m.model_id.clone(),
                    provider_id: m.provider_id.clone(),
                }),
                parent_id: None,
            })
            .await?;

        let parent = self.params.parent_local_id.clone();
        self.register(&session.id, parent, Some(self.params.cwd.clone())).await;
        if let Some(command_id) = command_id {
            let _ = self
                .events
                .send(AdapterEvent::CommandResult { command_id, ok: true, error: None })
                .await;
        }
        if let Some(m) = model.as_ref() {
            let _ = self
                .events
                .send(AdapterEvent::SessionModel {
                    local_id: session.id.clone(),
                    model: m.qualified(),
                })
                .await;
        }

        let (evt_tx, mut evt_rx) = mpsc::channel(256);
        let stream = tokio::spawn(pump_sse(client.clone(), evt_tx, self.shutdown.clone()));

        if let Some(text) = self.first_turn() {
            self.prompt(&client, &session.id, &text, model.as_ref()).await;
        }

        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => break,
                status = child.wait() => {
                    tracing::info!(?status, "opencode serve exited");
                    break;
                }
                evt = evt_rx.recv() => {
                    let Some(evt) = evt else { break };
                    self.on_event(&client, evt).await;
                }
                cmd = self.commands.recv() => {
                    let Some(cmd) = cmd else { break };
                    if !self.on_command(&client, cmd, model.as_ref()).await {
                        break;
                    }
                }
            }
        }

        stream.abort();
        self.abort_owned(&client).await;
        shutdown_serve(&mut child).await;
        Ok(())
    }

    /// Abort every in-flight turn before the server goes away: a killed session
    /// that only loses its supervisor keeps generating, and on a metered account
    /// keeps spending.
    async fn abort_owned(&self, client: &OpenCodeClient) {
        for id in &self.owned {
            if let Err(err) = client.abort(id).await {
                tracing::warn!(%err, session = %id, "opencode abort failed");
            }
        }
    }

    fn agent(&self) -> Option<String> {
        self.params.agent.clone().or_else(|| self.params.cfg.default_agent.clone())
    }

    fn first_turn(&self) -> Option<String> {
        let prompt = self.params.prompt.clone().unwrap_or_default();
        if self.params.attachments.is_empty() {
            return (!prompt.trim().is_empty()).then_some(prompt);
        }
        let files = self.params.attachments.join("\n");
        Some(format!("{prompt}\n\nAttached files:\n{files}").trim().to_owned())
    }

    async fn await_health(&self, client: &OpenCodeClient) -> Result<()> {
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(self.params.cfg.startup_timeout_ms);
        let mut last: Option<String> = None;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(HEALTH_PROBE_TIMEOUT, client.health()).await {
                Ok(Ok(h)) if h.healthy => return Ok(()),
                Ok(Ok(h)) => last = Some(format!("unhealthy (version {})", h.version)),
                Ok(Err(err)) => last = Some(err.to_string()),
                Err(_) => last = Some("health probe timed out".to_owned()),
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        anyhow::bail!(
            "`opencode serve` did not become healthy at {}: {}",
            client.base(),
            last.unwrap_or_else(|| "no response".to_owned())
        )
    }

    async fn register(
        &mut self,
        local_id: &str,
        parent_local_id: Option<String>,
        working_dir: Option<String>,
    ) {
        self.owned.insert(local_id.to_owned());
        self.live.lock().await.insert(local_id.to_owned(), self.commands_tx.clone());
        let _ = self
            .events
            .send(AdapterEvent::SessionStarted {
                local_id: local_id.to_owned(),
                meta: SessionMeta {
                    working_dir,
                    parent_local_id,
                    extra: serde_json::json!({
                        "harness": "opencode",
                        "spawn_key": self.params.key,
                    }),
                },
            })
            .await;
    }

    async fn prompt(
        &self,
        client: &OpenCodeClient,
        session_id: &str,
        text: &str,
        model: Option<&ModelRef>,
    ) {
        let body = PromptRequest {
            model: model.map(|m| PromptModelRef {
                provider_id: m.provider_id.clone(),
                model_id: m.model_id.clone(),
            }),
            agent: self.agent(),
            parts: vec![PartInput::Text { text: text.to_owned() }],
        };
        if let Err(err) = client.prompt_async(session_id, &body).await {
            tracing::error!(%err, %session_id, "opencode prompt failed");
            let _ = self
                .events
                .send(status(session_id, Some("error".to_owned()), Some(err.to_string()), None))
                .await;
        }
    }

    /// Returns `false` when the driver should stop.
    #[allow(clippy::cognitive_complexity)]
    async fn on_command(
        &mut self,
        client: &OpenCodeClient,
        cmd: SessionCommand,
        model: Option<&ModelRef>,
    ) -> bool {
        match cmd {
            SessionCommand::Prompt { session_id, text } => {
                self.prompt(client, &session_id, &text, model).await;
            }
            SessionCommand::Kill { session_id } => {
                if let Err(err) = client.abort(&session_id).await {
                    tracing::warn!(%err, %session_id, "opencode abort failed");
                }
                if self.owned.len() <= 1 {
                    return false;
                }
                self.owned.remove(&session_id);
                self.live.lock().await.remove(&session_id);
                let _ = self
                    .events
                    .send(AdapterEvent::SessionEnded {
                        local_id: session_id,
                        reason: EndReason::Killed,
                    })
                    .await;
            }
            SessionCommand::Fork { parent, prompt, name, command_id } => {
                self.on_fork(client, &parent, prompt, name, command_id, model).await;
            }
            SessionCommand::Permission { session_id, request_id, allow } => {
                let response = if allow { "once" } else { "reject" };
                if let Err(err) =
                    client.respond_permission(&session_id, &request_id, response).await
                {
                    tracing::warn!(%err, %session_id, "opencode permission response failed");
                }
                let _ = self
                    .events
                    .send(AdapterEvent::PermissionResolved { local_id: session_id, request_id })
                    .await;
            }
        }
        true
    }

    async fn on_fork(
        &mut self,
        client: &OpenCodeClient,
        parent: &str,
        prompt: Option<String>,
        name: Option<String>,
        command_id: Option<Uuid>,
        model: Option<&ModelRef>,
    ) {
        match client.fork(parent).await {
            Ok(child) => {
                self.register(&child.id, Some(parent.to_owned()), Some(self.params.cwd.clone()))
                    .await;
                if let Some(name) = name {
                    let _ = self.events.send(status(&child.id, None, None, Some(name))).await;
                }
                if let Some(command_id) = command_id {
                    let _ = self
                        .events
                        .send(AdapterEvent::CommandResult { command_id, ok: true, error: None })
                        .await;
                }
                if let Some(text) = prompt.filter(|p| !p.trim().is_empty()) {
                    self.prompt(client, &child.id, &text, model).await;
                }
            }
            Err(err) => {
                tracing::error!(%err, %parent, "opencode fork failed");
                if let Some(command_id) = command_id {
                    let _ = self
                        .events
                        .send(AdapterEvent::CommandResult {
                            command_id,
                            ok: false,
                            error: Some(err.to_string()),
                        })
                        .await;
                }
            }
        }
    }

    async fn on_event(&mut self, client: &OpenCodeClient, evt: OcEvent) {
        let Some(session_id) = evt.session_id().map(str::to_owned) else { return };
        if !self.owned.contains(&session_id) {
            return;
        }
        match evt {
            OcEvent::MessageUpdated { properties } => {
                let info = properties.info;
                self.roles.insert(info.id.clone(), info.role.clone());
                if let Some(usage) = normalize::token_usage(&session_id, &info)
                    && self.emitted_usage.insert(info.id.clone())
                {
                    let _ = self.events.send(usage).await;
                }
                if let Some(error) = info.error.as_ref() {
                    self.emit_error(&session_id, &normalize::error_text(error)).await;
                }
            }
            OcEvent::PartUpdated { properties } => {
                self.emit_part(&session_id, &properties.part).await;
            }
            OcEvent::SessionIdle { .. } => {
                let _ = self
                    .events
                    .send(status(&session_id, Some("idle".to_owned()), None, None))
                    .await;
            }
            OcEvent::SessionStatus { properties } => {
                if let Some(kind) = status_kind(&properties.status) {
                    let (tempo, detail) = match kind {
                        StatusKind::Busy => ("working".to_owned(), None),
                        StatusKind::Retry { attempt, message } => (
                            "working".to_owned(),
                            Some(format!("provider retry {attempt}: {message}")),
                        ),
                        StatusKind::Other(other) => (other, None),
                    };
                    let _ = self.events.send(status(&session_id, Some(tempo), detail, None)).await;
                }
            }
            OcEvent::SessionError { properties } => {
                let text = properties
                    .error
                    .as_ref()
                    .map_or_else(|| "session error".to_owned(), normalize::error_text);
                self.emit_error(&session_id, &text).await;
            }
            OcEvent::SessionDeleted { .. } => {
                self.owned.remove(&session_id);
                self.live.lock().await.remove(&session_id);
                let _ = self
                    .events
                    .send(AdapterEvent::SessionEnded {
                        local_id: session_id,
                        reason: EndReason::Completed,
                    })
                    .await;
            }
            OcEvent::PermissionAsked { properties } => {
                let _ = self
                    .events
                    .send(AdapterEvent::PermissionRequest {
                        local_id: session_id,
                        request_id: properties.id.clone(),
                        tool: properties.permission.clone(),
                        input: properties.metadata.clone(),
                    })
                    .await;
                let _ = client;
            }
            OcEvent::PermissionReplied { properties } => {
                let _ = self
                    .events
                    .send(AdapterEvent::PermissionResolved {
                        local_id: session_id,
                        request_id: properties.id,
                    })
                    .await;
            }
            OcEvent::Other => {}
        }
    }

    async fn emit_part(&mut self, session_id: &str, part: &Part) {
        let role = part
            .message_id()
            .and_then(|id| self.roles.get(id))
            .map_or("assistant", String::as_str)
            .to_owned();
        if !normalize::is_final(part, &role) || !self.emitted_parts.insert(part.id().to_owned()) {
            return;
        }
        for (kind, payload) in normalize::part_payloads(part, &role) {
            let event = match kind {
                Kind::Message => AdapterEvent::Message { local_id: session_id.to_owned(), payload },
                Kind::ToolUse => AdapterEvent::ToolUse { local_id: session_id.to_owned(), payload },
            };
            if self.events.send(event).await.is_err() {
                return;
            }
        }
    }

    async fn emit_error(&self, session_id: &str, text: &str) {
        let _ = self
            .events
            .send(AdapterEvent::Message {
                local_id: session_id.to_owned(),
                payload: serde_json::json!({
                    "type": "text",
                    "content": format!("· {text}"),
                    "role": "assistant",
                    "text": format!("· {text}"),
                }),
            })
            .await;
    }
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
        children: Vec::new(),
    }
}

#[allow(clippy::cognitive_complexity)]
async fn pump_sse(
    client: Arc<OpenCodeClient>,
    out: mpsc::Sender<OcEvent>,
    shutdown: CancellationToken,
) {
    use futures_util::StreamExt;

    loop {
        if shutdown.is_cancelled() {
            return;
        }
        match client.events().await {
            Ok(resp) => {
                let mut decoder = SseDecoder::new();
                let mut stream = resp.bytes_stream();
                loop {
                    tokio::select! {
                        () = shutdown.cancelled() => return,
                        chunk = stream.next() => {
                            let Some(chunk) = chunk else { break };
                            let Ok(bytes) = chunk else { break };
                            for data in decoder.push(&String::from_utf8_lossy(&bytes)) {
                                match serde_json::from_str::<OcEvent>(&data) {
                                    Ok(evt) => {
                                        if out.send(evt).await.is_err() {
                                            return;
                                        }
                                    }
                                    Err(err) => {
                                        tracing::debug!(%err, "undecodable opencode event");
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(err) => tracing::warn!(%err, "opencode event stream unavailable"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// How long `opencode serve` gets to exit on SIGTERM before the group is
/// `SIGKILL`ed.
const SERVE_TERM_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Terminate the serve process *tree*: signal the group (see `process_group`
/// at spawn), then escalate. Returns once the direct child is reaped.
async fn shutdown_serve(child: &mut tokio::process::Child) {
    signal_group(child, rustix::process::Signal::Term);
    if tokio::time::timeout(SERVE_TERM_GRACE, child.wait()).await.is_ok() {
        return;
    }
    signal_group(child, rustix::process::Signal::Kill);
    let _ = child.start_kill();
    let _ = child.wait().await;
}

fn signal_group(child: &tokio::process::Child, signal: rustix::process::Signal) {
    let Some(pid) = child.id().and_then(|p| i32::try_from(p).ok()) else { return };
    // The child leads its own group, so its pid is the pgid. A reaped
    // process just yields ESRCH.
    if let Some(pid) = rustix::process::Pid::from_raw(pid) {
        let _ = rustix::process::kill_process_group(pid, signal);
    }
}

fn drain_child_logs(child: &mut tokio::process::Child) {
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "opencode_serve", "{line}");
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "opencode_serve_stderr", "{line}");
            }
        });
    }
}

fn free_port(hostname: &str) -> Result<u16> {
    let listener = std::net::TcpListener::bind((hostname, 0))
        .with_context(|| format!("bind {hostname}:0 for the opencode server"))?;
    Ok(listener.local_addr()?.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_overrides() {
        let d = OpenCodeConfig::from_value(&serde_json::json!({}));
        assert_eq!(d.bin, "opencode");
        assert_eq!(d.hostname, "127.0.0.1");
        assert_eq!(d.startup_timeout_ms, 60_000);
        assert!(d.default_agent.is_none());

        let c = OpenCodeConfig::from_value(&serde_json::json!({
            "bin": "/usr/local/bin/opencode",
            "hostname": "127.0.0.2",
            "state_root": "/run/oc",
            "startup_timeout_ms": 5000,
            "agent": "cctui-reviewer",
            "model": "fireworks-ai/accounts/fireworks/models/kimi-k3",
        }));
        assert_eq!(c.bin, "/usr/local/bin/opencode");
        assert_eq!(c.state_root, std::path::PathBuf::from("/run/oc"));
        assert_eq!(c.startup_timeout_ms, 5000);
        assert_eq!(c.default_agent.as_deref(), Some("cctui-reviewer"));
        assert_eq!(
            c.default_model.as_deref(),
            Some("fireworks-ai/accounts/fireworks/models/kimi-k3")
        );
    }

    #[test]
    fn blank_config_strings_fall_back_to_defaults() {
        let c = OpenCodeConfig::from_value(&serde_json::json!({ "bin": "  ", "agent": "" }));
        assert_eq!(c.bin, "opencode");
        assert!(c.default_agent.is_none());
    }

    #[test]
    fn free_port_is_bindable() {
        let port = free_port("127.0.0.1").unwrap();
        assert!(port > 0);
    }

    /// End-to-end against a real `opencode` binary: point `CCTUI_OPENCODE_BIN`
    /// at one and run with `--ignored`. Ignored by default — CI images have no
    /// opencode.
    #[tokio::test]
    #[ignore = "requires a real opencode binary (CCTUI_OPENCODE_BIN)"]
    async fn spawns_a_real_server_and_creates_a_session() {
        let Ok(bin) = std::env::var("CCTUI_OPENCODE_BIN") else { return };
        let repo = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let shutdown = CancellationToken::new();
        let command_id = Uuid::new_v4();

        let params = SpawnParams {
            cfg: OpenCodeConfig {
                bin,
                state_root: state.path().to_path_buf(),
                startup_timeout_ms: 90_000,
                ..OpenCodeConfig::default()
            },
            key: command_id.to_string(),
            cwd: repo.path().display().to_string(),
            env: std::collections::BTreeMap::new(),
            prompt: None,
            name: Some("probe".to_owned()),
            model: Some("fireworks-ai/accounts/fireworks/models/kimi-k3".to_owned()),
            agent: Some(super::super::config::REVIEWER_AGENT.to_owned()),
            attachments: Vec::new(),
            command_id: Some(command_id),
            parent_local_id: None,
        };
        let live = LiveRegistry::default();
        let handle =
            tokio::spawn(OpenCodeSession::new(params, tx, live.clone(), shutdown.clone()).run());

        let mut started = None;
        let mut acked = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
        while tokio::time::Instant::now() < deadline && !(acked && started.is_some()) {
            match tokio::time::timeout(std::time::Duration::from_secs(120), rx.recv()).await {
                Ok(Some(AdapterEvent::SessionStarted { local_id, .. })) => started = Some(local_id),
                Ok(Some(AdapterEvent::CommandResult { ok, error, .. })) => {
                    assert!(ok, "spawn failed: {error:?}");
                    acked = true;
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        shutdown.cancel();
        let _ = handle.await;

        let local_id = started.expect("no SessionStarted");
        assert!(local_id.starts_with("ses"), "unexpected session id {local_id}");
        assert!(acked, "spawn was never acked");
    }

    #[cfg(unix)]
    fn is_alive(pid: i32) -> bool {
        rustix::process::Pid::from_raw(pid)
            .is_some_and(|p| rustix::process::test_kill_process(p).is_ok())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_serve_takes_down_the_whole_process_tree() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("sleep 300 & echo $!; wait")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .process_group(0);
        let mut child = cmd.spawn().unwrap();

        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let grandchild: i32 = lines.next_line().await.unwrap().unwrap().trim().parse().unwrap();
        assert!(is_alive(grandchild));

        shutdown_serve(&mut child).await;
        for _ in 0..50 {
            if !is_alive(grandchild) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("grandchild {grandchild} survived shutdown_serve");
    }

    #[test]
    fn status_mirrors_tempo_into_state() {
        match status("ses_1", Some("idle".to_owned()), None, None) {
            AdapterEvent::Status { local_id, tempo, state, name, .. } => {
                assert_eq!(local_id, "ses_1");
                assert_eq!(tempo.as_deref(), Some("idle"));
                assert_eq!(state.as_deref(), Some("idle"));
                assert!(name.is_none());
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }
}
