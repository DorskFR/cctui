//! End-to-end scenarios for the ACP adapter against a fake stdio ACP agent.
//!
//! `harness = false`: this binary is both the test runner and, when
//! `FAKE_ACP_AGENT` is set in its environment, the agent itself. The adapter
//! spawns `current_exe()` as the agent, so nothing extra ships or installs.
//!
//! The fake speaks the ACP v1 wire directly (newline JSON-RPC), scripted by
//! the JSON in `FAKE_ACP_SCRIPT`, and can stream every update variant,
//! request a permission, stall, refuse to start without a login, and leave a
//! grandchild behind to prove the kill reaches the whole process group.
//!
//! A re-attached agent gets no spawn env (env is never persisted), so a row
//! can carry the role and script in argv instead: `--fake-acp-agent
//! --script <json>`.
//!
//! `cargo test -p cctui-daemon --test acp_fake_agent -- --ignored` runs the
//! real-agent probes against `opencode acp` instead.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use cctui_daemon::adapter_runtime::{Adapter, AdapterCtx};
use cctui_daemon::adapters::acp::AcpAdapter;
use cctui_daemon::adapters::acp::modes::{GEMINI_STYLE, ModeTable};
use cctui_daemon::adapters::acp::persist::SessionStore;
use cctui_daemon::adapters::acp::rows::AgentRow;
use cctui_daemon::client::ServerClient;
use cctui_proto::adapter::{
    AdapterCommand, AdapterEvent, AdapterId, EndReason, PermissionMode, SessionSpec,
};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const AGENT_ENV: &str = "FAKE_ACP_AGENT";
const AGENT_ARG: &str = "--fake-acp-agent";
const SCRIPT_ARG: &str = "--script";
const SCRIPT_ENV: &str = "FAKE_ACP_SCRIPT";
const SPAWN: Uuid = Uuid::from_u128(0x51);
const REPLY: Uuid = Uuid::from_u128(0x52);
const INTERRUPT: Uuid = Uuid::from_u128(0x53);
const DIAGNOSE: Uuid = Uuid::from_u128(0x54);
const MACHINE_KEY: &str = "test-machine-key";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "mcp-agent") {
        let flag = |name: &str| args.iter().skip_while(|a| *a != name).nth(1).cloned();
        let (Some(session), Some(sock)) = (flag("--session"), flag("--sock")) else {
            std::process::exit(2);
        };
        cctui_daemon::mcp::run(&session, std::path::Path::new(&sock)).unwrap();
        return;
    }
    if std::env::var_os(AGENT_ENV).is_some() || args.iter().any(|a| a == AGENT_ARG) {
        fake_agent::main();
        return;
    }
    if args.iter().any(|a| a == "--list") {
        for (name, _) in scenarios::ALL {
            println!("{name}: test");
        }
        return;
    }
    isolate_runtime_dir();
    let ignored = args.iter().any(|a| a == "--ignored" || a == "--include-ignored");
    let filters: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let failures = rt.block_on(async {
        let mut failures = 0;
        if ignored {
            report(
                "real_opencode_agent_prompts_reports_usage_and_ends_its_turn",
                real_agent::run().await,
                &mut failures,
            );
            report(
                "real_opencode_agent_resumes_in_a_fresh_process_after_a_reexec",
                real_agent::resume().await,
                &mut failures,
            );
        } else {
            for (name, run) in scenarios::ALL {
                if !filters.is_empty() && !filters.iter().any(|f| name.contains(f.as_str())) {
                    continue;
                }
                let outcome = tokio::time::timeout(Duration::from_secs(90), run())
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("timed out after 90s")));
                report(name, outcome, &mut failures);
            }
        }
        failures
    });
    if failures > 0 {
        eprintln!("\n{failures} scenario(s) failed");
        std::process::exit(1);
    }
    println!("\nall acp scenarios passed");
}

/// The `CctuiAgent` socket lives in the runtime dir; a scenario serving it
/// must never bind over the socket of a daemon running on this machine.
#[allow(unsafe_code)]
fn isolate_runtime_dir() {
    let dir = std::env::temp_dir().join(format!("cctui-acp-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: called from `main` before the runtime or any other thread exists.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", &dir) };
}

fn report(name: &str, outcome: anyhow::Result<()>, failures: &mut u32) {
    match outcome {
        Ok(()) => println!("test {name} ... ok"),
        Err(err) => {
            *failures += 1;
            println!("test {name} ... FAILED\n    {err:#}");
        }
    }
}

// ---------------------------------------------------------------------------
// The fake agent
// ---------------------------------------------------------------------------

mod fake_agent {
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::mpsc;

    use super::{SCRIPT_ARG, SCRIPT_ENV};

    #[derive(Clone)]
    struct Script(Value);

    impl Script {
        fn get(&self, key: &str) -> Option<&Value> {
            self.0.get(key).filter(|v| !v.is_null())
        }
        fn flag(&self, key: &str) -> bool {
            self.get(key).and_then(Value::as_bool) == Some(true)
        }
        fn turn(&self) -> String {
            self.get("turn").and_then(Value::as_str).unwrap_or("full").to_owned()
        }
    }

    enum Inbound {
        Line(String),
        Eof,
    }

    pub fn main() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(run());
    }

    async fn run() {
        let from_argv = std::env::args().skip_while(|a| a != SCRIPT_ARG).nth(1);
        let script = Script(
            std::env::var(SCRIPT_ENV)
                .ok()
                .or(from_argv)
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(Value::Object(serde_json::Map::new())),
        );
        eprintln!("fake-acp: starting");
        let (tx, mut rx) = mpsc::unbounded_channel::<Inbound>();
        tokio::spawn(async move {
            let mut lines = BufReader::new(tokio::io::stdin()).lines();
            loop {
                let Ok(Some(line)) = lines.next_line().await else {
                    let _ = tx.send(Inbound::Eof);
                    return;
                };
                if tx.send(Inbound::Line(line)).is_err() {
                    return;
                }
            }
        });
        let mut agent = Agent {
            relay: None,
            script,
            out: tokio::io::stdout(),
            next_id: 1000,
            message_id: String::new(),
            pending_prompt: None,
            pending_permission: None,
            config_options: Vec::new(),
            current_mode: None,
        };
        agent.config_options = agent
            .script
            .get("configOptions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        agent.current_mode = agent
            .script
            .get("modes")
            .and_then(|m| m.get("currentModeId"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(path) = agent.script.get("orphanPidFile").and_then(Value::as_str) {
            // Left running on purpose: the kill must reach it through the process
            // group, which the scenario checks by pid.
            let child = tokio::process::Command::new("sleep").arg("300").spawn().expect("sleep");
            std::fs::write(path, child.id().expect("pid").to_string()).unwrap();
            std::mem::forget(child);
        }
        while let Some(inbound) = rx.recv().await {
            match inbound {
                Inbound::Eof => {
                    eprintln!("fake-acp: stdin closed");
                    return;
                }
                Inbound::Line(line) => {
                    let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                    if !agent.on_message(&msg).await {
                        return;
                    }
                }
            }
        }
    }

    struct Relay {
        stdin: tokio::process::ChildStdin,
        lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
        _child: tokio::process::Child,
        next_id: u64,
    }

    impl Relay {
        /// Start the stdio MCP server the client declared, as a real agent does
        /// when it opens the session.
        async fn launch(server: &Value) -> Option<Self> {
            let args: Vec<&str> =
                server["args"].as_array()?.iter().filter_map(Value::as_str).collect();
            let mut child = tokio::process::Command::new(server["command"].as_str()?)
                .args(args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .ok()?;
            let stdin = child.stdin.take()?;
            let lines = BufReader::new(child.stdout.take()?).lines();
            let mut relay = Self { stdin, lines, _child: child, next_id: 1 };
            relay
                .call("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {} }))
                .await?;
            relay.write(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
            Some(relay)
        }

        async fn write(&mut self, msg: Value) {
            let mut line = msg.to_string();
            line.push('\n');
            let _ = self.stdin.write_all(line.as_bytes()).await;
            let _ = self.stdin.flush().await;
        }

        async fn call(&mut self, method: &str, params: Value) -> Option<Value> {
            let id = self.next_id;
            self.next_id += 1;
            self.write(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
                .await;
            while let Ok(Some(line)) = self.lines.next_line().await {
                let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                if msg["id"] == id {
                    return Some(msg);
                }
            }
            None
        }
    }

    struct Agent {
        relay: Option<Relay>,
        script: Script,
        out: tokio::io::Stdout,
        next_id: u64,
        message_id: String,
        pending_prompt: Option<(Value, String)>,
        pending_permission: Option<(u64, Value, String)>,
        config_options: Vec<Value>,
        current_mode: Option<String>,
    }

    impl Agent {
        async fn send(&mut self, msg: Value) {
            let mut line = msg.to_string();
            line.push('\n');
            let _ = self.out.write_all(line.as_bytes()).await;
            let _ = self.out.flush().await;
        }

        async fn respond(&mut self, id: Value, result: Value) {
            self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result })).await;
        }

        async fn respond_error(&mut self, id: Value, code: i64, message: &str) {
            self.send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
                .await;
        }

        async fn update(&mut self, session_id: &str, update: Value) {
            let mut params = json!({ "sessionId": session_id, "update": update });
            if let Some(quota) = self.script.get("quota").cloned() {
                params["_meta"] = json!({ "quota": quota });
            }
            self.send(json!({ "jsonrpc": "2.0", "method": "session/update", "params": params }))
                .await;
        }

        /// `false` once the agent should exit.
        async fn on_message(&mut self, msg: &Value) -> bool {
            if msg.get("method").is_none() {
                return self.on_response(msg).await;
            }
            let method = msg["method"].as_str().unwrap_or_default().to_owned();
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let Some(id) = msg.get("id").cloned() else {
                if method == "session/cancel"
                    && let Some((prompt_id, _)) = self.pending_prompt.take()
                {
                    self.respond(prompt_id, json!({ "stopReason": "cancelled" })).await;
                }
                return true;
            };
            self.on_request(id, &method, &params).await
        }

        async fn on_request(&mut self, id: Value, method: &str, params: &Value) -> bool {
            match method {
                "initialize" => {
                    let caps = self.script.get("capabilities").cloned().unwrap_or(json!({}));
                    let agent = self
                        .script
                        .get("agent")
                        .cloned()
                        .unwrap_or(json!({ "name": "fake-acp", "version": "9.9.9" }));
                    let mut result = json!({
                        "protocolVersion": 1,
                        "agentCapabilities": {
                            "loadSession": caps.get("loadSession").and_then(Value::as_bool).unwrap_or(false),
                            "promptCapabilities": { "image": caps.get("image").and_then(Value::as_bool).unwrap_or(true) },
                            "sessionCapabilities": {},
                        },
                        "agentInfo": agent,
                    });
                    if caps.get("close").and_then(Value::as_bool).unwrap_or(true) {
                        result["agentCapabilities"]["sessionCapabilities"]["close"] = json!({});
                    }
                    if caps.get("resume").and_then(Value::as_bool) == Some(true) {
                        result["agentCapabilities"]["sessionCapabilities"]["resume"] = json!({});
                    }
                    self.respond(id, result).await;
                }
                "session/new" => {
                    if self.script.flag("authRequired") {
                        self.respond_error(id, -32000, "Authentication required").await;
                        return true;
                    }
                    self.open_relay(params).await;
                    let mut result = self.session_state();
                    result["sessionId"] = json!("fake-session-1");
                    self.respond(id, result).await;
                }
                "session/resume" => {
                    self.open_relay(params).await;
                    let result = self.session_state();
                    self.respond(id, result).await;
                }
                "session/load" => {
                    let session_id = params["sessionId"].as_str().unwrap_or_default().to_owned();
                    self.replay(&session_id).await;
                    let result = self.session_state();
                    self.respond(id, result).await;
                }
                "session/set_mode" => {
                    let mode = params["modeId"].as_str().unwrap_or_default().to_owned();
                    self.current_mode = Some(mode.clone());
                    self.respond(id, json!({})).await;
                    self.update(
                        "fake-session-1",
                        json!({ "sessionUpdate": "current_mode_update", "currentModeId": mode }),
                    )
                    .await;
                }
                "session/set_config_option" => {
                    let config_id = params["configId"].as_str().unwrap_or_default();
                    let value = params["value"].clone();
                    for option in &mut self.config_options {
                        if option["id"] == config_id {
                            option["currentValue"] = value.clone();
                        }
                    }
                    self.respond(id, json!({ "configOptions": self.config_options })).await;
                }
                "session/set_model" => {
                    if self.script.get("models").is_some() {
                        self.respond(id, json!({})).await;
                    } else {
                        self.respond_error(id, -32601, "Method not found").await;
                    }
                }
                "session/close" => {
                    self.respond(id, json!({})).await;
                    return !self.script.flag("ignoreClose");
                }
                "session/prompt" => {
                    let session_id =
                        params["sessionId"].as_str().unwrap_or("fake-session-1").to_owned();
                    let text = params["prompt"][0]["text"].as_str().unwrap_or_default().to_owned();
                    self.on_prompt(id, &session_id, &text).await;
                }
                _ => self.respond_error(id, -32601, "Method not found").await,
            }
            true
        }

        async fn open_relay(&mut self, params: &Value) {
            let declared = params["mcpServers"]
                .as_array()
                .and_then(|servers| servers.iter().find(|s| s["name"] == "cctui"));
            if let Some(server) = declared {
                self.relay = Relay::launch(server).await;
            }
        }

        /// One `CctuiAgent` call through the relay; the answer becomes the
        /// turn's whole reply.
        async fn relay_turn(&mut self, id: Value, session_id: &str) {
            let answer = match self.relay.as_mut() {
                Some(relay) => relay
                    .call(
                        "tools/call",
                        json!({
                            "name": "CctuiAgent",
                            "arguments": {
                                "prompt": "hello child",
                                "model": "claude-opus-5-5",
                                "adapter": "claude-code",
                            },
                        }),
                    )
                    .await
                    .map_or_else(|| "relay closed".to_owned(), |r| r["result"].to_string()),
                None => "no relay declared".to_owned(),
            };
            self.update(
                session_id,
                json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": format!("relay: {answer}") },
                    "messageId": format!("relay-{}", self.next_id),
                }),
            )
            .await;
            self.next_id += 1;
            self.respond(id, json!({ "stopReason": "end_turn" })).await;
        }

        fn session_state(&self) -> Value {
            let mut result = json!({});
            if let Some(modes) = self.script.get("modes") {
                result["modes"] = modes.clone();
            }
            if let Some(models) = self.script.get("models") {
                result["models"] = models.clone();
            }
            if !self.config_options.is_empty() {
                result["configOptions"] = Value::Array(self.config_options.clone());
            }
            result
        }

        /// What `session/load` streams back before answering: the earlier
        /// conversation, as the agent stored it.
        async fn replay(&mut self, session_id: &str) {
            self.update(
                session_id,
                json!({
                    "sessionUpdate": "user_message_chunk",
                    "content": { "type": "text", "text": "replayed ping" },
                    "messageId": "replay-user",
                }),
            )
            .await;
            self.update(
                session_id,
                json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": "replayed answer" },
                    "messageId": "replay-agent",
                }),
            )
            .await;
        }

        #[allow(clippy::too_many_lines)]
        async fn on_prompt(&mut self, id: Value, session_id: &str, text: &str) {
            let turn = self.script.turn();
            if turn == "relay" {
                self.relay_turn(id, session_id).await;
                return;
            }
            self.message_id = format!(
                "msg-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );
            let message_id = self.message_id.clone();
            self.update(
                session_id,
                json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": "Hello " },
                    "messageId": message_id,
                }),
            )
            .await;
            self.update(
                session_id,
                json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": format!("world: {text}") },
                    "messageId": message_id,
                }),
            )
            .await;
            if turn == "stall" {
                self.pending_prompt = Some((id, session_id.to_owned()));
                return;
            }
            self.update(
                session_id,
                json!({
                    "sessionUpdate": "agent_thought_chunk",
                    "content": { "type": "text", "text": "thinking about it" },
                    "messageId": message_id,
                }),
            )
            .await;
            self.update(
                session_id,
                json!({
                    "sessionUpdate": "tool_call",
                    "toolCallId": "call-1",
                    "title": "List files",
                    "name": "list_directory",
                    "kind": "read",
                    "status": "pending",
                    "rawInput": { "path": "." },
                }),
            )
            .await;
            if turn == "permission" {
                let request_id = self.next_id;
                self.next_id += 1;
                self.send(json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": "session/request_permission",
                    "params": {
                        "sessionId": session_id,
                        "toolCall": { "toolCallId": "call-1", "title": "List files", "kind": "read", "rawInput": { "path": "." } },
                        "options": [
                            { "optionId": "allow-always", "name": "Always", "kind": "allow_always" },
                            { "optionId": "allow-once", "name": "Once", "kind": "allow_once" },
                            { "optionId": "reject-once", "name": "No", "kind": "reject_once" },
                        ],
                    },
                })).await;
                self.pending_permission = Some((request_id, id, session_id.to_owned()));
                return;
            }
            if turn == "elicit" {
                let request_id = self.next_id;
                self.next_id += 1;
                self.send(json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": "elicitation/create",
                    "params": {
                        "mode": "form",
                        "sessionId": session_id,
                        "message": "Deploy settings",
                        "requestedSchema": {
                            "type": "object",
                            "properties": {
                                "env": { "type": "string", "title": "Environment",
                                         "oneOf": [{ "const": "stg", "title": "Staging" }, { "const": "prd", "title": "Production" }] },
                                "replicas": { "type": "integer", "title": "Replicas" },
                                "verbose": { "type": "boolean", "title": "Verbose" },
                            },
                            "required": ["env", "replicas"],
                        },
                    },
                })).await;
                self.pending_permission = Some((request_id, id, session_id.to_owned()));
                return;
            }
            self.finish_turn(id, session_id, false).await;
        }

        async fn finish_turn(&mut self, id: Value, session_id: &str, rejected: bool) {
            self.update(session_id, json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "call-1",
                "status": if rejected { "failed" } else { "completed" },
                "content": [{ "type": "content", "content": { "type": "text", "text": if rejected { "permission denied" } else { "Cargo.toml\nsrc/" } } }],
            })).await;
            self.update(
                session_id,
                json!({
                    "sessionUpdate": "plan",
                    "entries": [
                        { "content": "Look around", "priority": "high", "status": "completed" },
                        { "content": "Answer", "priority": "medium", "status": "in_progress" },
                    ],
                }),
            )
            .await;
            self.update(
                session_id,
                json!({
                    "sessionUpdate": "usage_update",
                    "used": 1234,
                    "size": 100_000,
                    "cost": { "amount": 0.0042, "currency": "USD" },
                }),
            )
            .await;
            self.respond(id, json!({
                "stopReason": "end_turn",
                "usage": { "totalTokens": 130, "inputTokens": 100, "outputTokens": 30, "cachedReadTokens": 10 },
            })).await;
        }

        async fn on_response(&mut self, msg: &Value) -> bool {
            let Some((request_id, prompt_id, session_id)) = self.pending_permission.take() else {
                return true;
            };
            if msg.get("id").and_then(Value::as_u64) != Some(request_id) {
                self.pending_permission = Some((request_id, prompt_id, session_id));
                return true;
            }
            let result = msg.get("result").cloned().unwrap_or(Value::Null);
            self.update(
                &session_id,
                json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": format!("answered: {result}") },
                    "messageId": "answer",
                }),
            )
            .await;
            let rejected = result
                .pointer("/outcome/optionId")
                .and_then(Value::as_str)
                .is_some_and(|picked| !picked.starts_with("allow"))
                || result.pointer("/outcome/outcome").is_some_and(|o| o == "cancelled");
            self.finish_turn(prompt_id, &session_id, rejected).await;
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Driving the adapter
// ---------------------------------------------------------------------------

struct Harness {
    events: mpsc::Receiver<AdapterEvent>,
    commands: mpsc::Sender<AdapterCommand>,
    shutdown: CancellationToken,
    _connected: broadcast::Sender<()>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    tmp: tempfile::TempDir,
    cwd: String,
    row: &'static AgentRow,
    bin: String,
    store: Arc<SessionStore>,
    reexec: CancellationToken,
    server: Option<ServerClient>,
}

static FAKE_ROW: AgentRow = AgentRow {
    id: "fake-acp",
    label: "Fake ACP agent",
    bin: "fake-acp-agent",
    args: &[],
    update: None,
    modes: GEMINI_STYLE,
    legacy_models: false,
};

fn start(row: &'static AgentRow, bin: &str) -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::at(tmp.path().join("acp-sessions.json")));
    start_in(row, bin, tmp, store, None)
}

fn start_in(
    row: &'static AgentRow,
    bin: &str,
    tmp: tempfile::TempDir,
    store: Arc<SessionStore>,
    server: Option<ServerClient>,
) -> Harness {
    let cwd = tmp.path().display().to_string();
    let (events_tx, events) = mpsc::channel(512);
    let (commands, commands_rx) = mpsc::channel(64);
    let (connected_tx, connected) = broadcast::channel(4);
    let shutdown = CancellationToken::new();
    let ctx = AdapterCtx {
        events: events_tx,
        commands: commands_rx,
        pty_watch: None,
        interrupts: None,
        shutdown: shutdown.clone(),
        config: json!({ "bin": bin }),
        server: server.clone(),
        machine_key: server.as_ref().map(|_| MACHINE_KEY.to_owned()),
        connected,
    };
    let reexec = CancellationToken::new();
    let adapter = AcpAdapter::new(row).with_store(Arc::clone(&store)).with_reexec(reexec.clone());
    let task = tokio::spawn(async move { adapter.start(ctx).await });
    Harness {
        events,
        commands,
        shutdown,
        _connected: connected_tx,
        task,
        tmp,
        cwd,
        row,
        bin: bin.to_owned(),
        store,
        reexec,
        server,
    }
}

/// A row that makes `current_exe()` the fake agent through argv alone, as a
/// re-attach launches it with no spawn env.
fn argv_row(script: &Value) -> &'static AgentRow {
    let args: Vec<&'static str> =
        vec![AGENT_ARG, SCRIPT_ARG, Box::leak(script.to_string().into_boxed_str())];
    Box::leak(Box::new(AgentRow { args: Box::leak(args.into_boxed_slice()), ..FAKE_ROW }))
}

impl Harness {
    fn spec(&self, script: &Value, mode: Option<PermissionMode>, prompt: &str) -> SessionSpec {
        let mut env = BTreeMap::new();
        env.insert(AGENT_ENV.to_owned(), "1".to_owned());
        env.insert(SCRIPT_ENV.to_owned(), script.to_string());
        SessionSpec {
            adapter_id: AdapterId::new(FAKE_ROW.id),
            working_dir: Some(self.cwd.clone()),
            prompt: Some(prompt.to_owned()),
            name: None,
            permission_mode: mode,
            effort: None,
            model: None,
            service_tier: None,
            env,
            bootstrap: Value::Null,
            parent_local_id: None,
        }
    }

    async fn spawn(&self, script: &Value, mode: Option<PermissionMode>, prompt: &str) {
        let spec = self.spec(script, mode, prompt);
        self.commands
            .send(AdapterCommand::Spawn { spec, command_id: Some(SPAWN), session_id: None })
            .await
            .unwrap();
    }

    async fn send(&self, cmd: AdapterCommand) {
        self.commands.send(cmd).await.unwrap();
    }

    /// Events until `stop` matches (inclusive), or the timeout.
    async fn until(
        &mut self,
        stop: impl Fn(&AdapterEvent) -> bool,
        timeout: Duration,
    ) -> anyhow::Result<Vec<AdapterEvent>> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut out = Vec::new();
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some(evt)) => {
                    let done = stop(&evt);
                    out.push(evt);
                    if done {
                        return Ok(out);
                    }
                }
                Ok(None) => anyhow::bail!("event channel closed; got {out:#?}"),
                Err(_) => anyhow::bail!("timed out waiting; got {out:#?}"),
            }
        }
    }

    async fn finish(self) {
        self.shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(10), self.task).await;
    }

    /// What a daemon self-update does: the re-exec warning, then this
    /// process gone, then a fresh adapter over the same state file.
    async fn restart(self, agent_pid: u32) -> anyhow::Result<Self> {
        self.reexec.cancel();
        let pid = i32::try_from(agent_pid)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while pid_alive(pid) {
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "agent {pid} outlived the re-exec"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let Self { shutdown, task, tmp, row, bin, store, events, server, .. } = self;
        shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(10), task).await;
        drop(events);
        Ok(start_in(row, &bin, tmp, store, server))
    }

    async fn agent_pid(&mut self, local_id: &str) -> anyhow::Result<u32> {
        let report = self.diagnose(local_id).await?;
        report
            .acp
            .as_ref()
            .and_then(|a| a.agent_pid)
            .ok_or_else(|| anyhow::anyhow!("no agent pid in {report:#?}"))
    }

    async fn diagnose(
        &mut self,
        local_id: &str,
    ) -> anyhow::Result<Box<cctui_proto::diagnose::SessionDiagnose>> {
        self.send(AdapterCommand::Diagnose { local_id: local_id.to_owned(), request_id: DIAGNOSE })
            .await;
        let diag = self
            .until(|e| matches!(e, AdapterEvent::Diagnose { .. }), Duration::from_secs(10))
            .await?;
        let Some(AdapterEvent::Diagnose { report, .. }) = diag.into_iter().last() else {
            unreachable!()
        };
        Ok(report)
    }

    async fn reply(&self, local_id: &str, text: &str) {
        self.send(AdapterCommand::Reply {
            local_id: local_id.to_owned(),
            text: text.to_owned(),
            ask_picks: None,
            env: BTreeMap::new(),
            command_id: Some(REPLY),
            turn_id: None,
        })
        .await;
    }
}

fn is_idle(evt: &AdapterEvent) -> bool {
    matches!(evt, AdapterEvent::Status { tempo: Some(t), .. } if t == "idle")
}

fn started(events: &[AdapterEvent]) -> Option<&str> {
    events.iter().find_map(|e| match e {
        AdapterEvent::SessionStarted { local_id, .. } => Some(local_id.as_str()),
        _ => None,
    })
}

fn messages<'a>(events: &'a [AdapterEvent], role: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter_map(|e| match e {
            AdapterEvent::Message { payload, .. } if payload["role"] == role => Some(payload),
            _ => None,
        })
        .collect()
}

fn tool_uses<'a>(events: &'a [AdapterEvent], kind: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter_map(|e| match e {
            AdapterEvent::ToolUse { payload, .. } if payload["type"] == kind => Some(payload),
            _ => None,
        })
        .collect()
}

fn command_result(events: &[AdapterEvent], id: Uuid) -> Option<(bool, Option<String>)> {
    events.iter().find_map(|e| match e {
        AdapterEvent::CommandResult { command_id, ok, error } if *command_id == id => {
            Some((*ok, error.clone()))
        }
        _ => None,
    })
}

fn pid_alive(pid: i32) -> bool {
    rustix::process::Pid::from_raw(pid)
        .is_some_and(|p| rustix::process::test_kill_process(p).is_ok())
}

fn full_script() -> Value {
    json!({
        "modes": {
            "currentModeId": "default",
            "availableModes": [
                { "id": "default", "name": "Default" },
                { "id": "autoEdit", "name": "Auto edit" },
                { "id": "yolo", "name": "YOLO" },
            ],
        },
    })
}

mod scenarios {
    use super::{
        AdapterCommand, AdapterEvent, DIAGNOSE, Duration, EndReason, FAKE_ROW, INTERRUPT,
        PermissionMode, SPAWN, Value, catalog_scenarios, command_result, full_script, is_idle,
        json, messages, pid_alive, start, started, tool_uses,
    };

    type Run =
        fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>;

    macro_rules! scenario {
        ($name:ident) => {
            (stringify!($name), (|| Box::pin($name())) as Run)
        };
    }

    pub const ALL: &[(&str, Run)] = &[
        scenario!(spawn_streams_every_update_variant_then_ends_the_turn),
        scenario!(kill_takes_the_agent_and_its_grandchild_down),
        scenario!(a_permission_request_is_answered_from_the_ui_path),
        scenario!(a_rejected_permission_fails_the_tool),
        scenario!(yolo_answers_permissions_itself),
        scenario!(an_allow_always_pick_round_trips_its_option_id),
        scenario!(an_elicitation_form_is_answered_with_typed_values),
        scenario!(an_inexpressible_mode_is_refused_before_spawn),
        scenario!(a_missing_login_is_an_actionable_spawn_failure),
        scenario!(interrupt_cancels_a_stalled_turn),
        scenario!(diagnose_carries_agent_info_and_the_rings),
        scenario!(config_options_populate_the_catalog_and_set_model_round_trips),
        scenario!(legacy_models_populate_the_catalog_and_set_model_round_trips),
        scenario!(a_reexec_reattaches_through_session_resume),
        scenario!(a_reexec_reattaches_through_session_load_without_replaying_rows),
        scenario!(an_agent_that_cannot_resume_ends_the_session_as_not_resumable),
        scenario!(the_agent_calls_cctui_agent_through_the_declared_relay),
        scenario!(a_reattached_session_redeclares_the_relay),
    ];

    fn exe() -> String {
        std::env::current_exe().unwrap().display().to_string()
    }

    pub async fn the_agent_calls_cctui_agent_through_the_declared_relay() -> anyhow::Result<()> {
        super::relay::run(false).await
    }

    pub async fn a_reattached_session_redeclares_the_relay() -> anyhow::Result<()> {
        super::relay::run(true).await
    }

    pub async fn spawn_streams_every_update_variant_then_ends_the_turn() -> anyhow::Result<()> {
        let mut h = start(&FAKE_ROW, &exe());
        h.spawn(&full_script(), Some(PermissionMode::Ask), "ping").await;
        let events = h.until(is_idle, Duration::from_secs(30)).await?;
        let local_id =
            started(&events).ok_or_else(|| anyhow::anyhow!("no SessionStarted: {events:#?}"))?;
        anyhow::ensure!(
            local_id == "fake-session-1",
            "local id is the agent's session id, got {local_id}"
        );
        anyhow::ensure!(
            command_result(&events, SPAWN) == Some((true, None)),
            "spawn must be acked ok: {events:#?}"
        );
        let assistant = messages(&events, "assistant");
        anyhow::ensure!(
            assistant
                .iter()
                .any(|m| m["content"] == "Hello world: ping" && m["text"] == "Hello world: ping"),
            "coalesced assistant message in both dialects: {assistant:#?}"
        );
        anyhow::ensure!(
            messages(&events, "assistant_thinking")
                .iter()
                .any(|m| m["content"] == "thinking about it"),
            "thought chunk: {events:#?}"
        );
        let calls = tool_uses(&events, "tool_call");
        anyhow::ensure!(
            calls.iter().any(|c| c["tool"] == "list_directory" && c["tool_use_id"] == "call-1"),
            "tool call: {calls:#?}"
        );
        anyhow::ensure!(
            calls
                .iter()
                .any(|c| c["tool"] == "update_plan"
                    && c["input"]["plan"][1]["status"] == "in_progress"),
            "plan as update_plan: {calls:#?}"
        );
        let results = tool_uses(&events, "tool_result");
        anyhow::ensure!(
            results.iter().any(|r| r["tool_use_id"] == "call-1"
                && r["is_error"] == false
                && r["output_summary"] == "Cargo.toml\nsrc/"),
            "tool result: {results:#?}"
        );
        anyhow::ensure!(
            events.iter().any(|e| matches!(
                e,
                AdapterEvent::TokenUsage {
                    input_tokens: 100,
                    output_tokens: 30,
                    cache_read_tokens: 10,
                    ..
                }
            )),
            "token usage from the prompt response: {events:#?}"
        );
        anyhow::ensure!(
            events.iter().any(|e| matches!(
                e,
                AdapterEvent::Status { permission_mode: Some(PermissionMode::Ask), .. }
            )),
            "the applied mode is reported as a posture: {events:#?}"
        );
        let idle = events.last().unwrap();
        anyhow::ensure!(
            matches!(idle, AdapterEvent::Status { detail: None, .. }),
            "end_turn carries no detail: {idle:?}"
        );
        h.send(AdapterCommand::Kill { local_id: local_id.to_owned(), signal: None }).await;
        let ended = h
            .until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(15))
            .await?;
        anyhow::ensure!(
            matches!(
                ended.last(),
                Some(AdapterEvent::SessionEnded { reason: EndReason::Killed, .. })
            ),
            "{ended:#?}"
        );
        h.finish().await;
        Ok(())
    }

    pub async fn kill_takes_the_agent_and_its_grandchild_down() -> anyhow::Result<()> {
        let mut h = start(&FAKE_ROW, &exe());
        let pid_file = std::path::Path::new(&h.cwd).join("orphan.pid");
        let mut script = full_script();
        script["orphanPidFile"] = json!(pid_file.display().to_string());
        h.spawn(&script, None, "ping").await;
        let events = h.until(is_idle, Duration::from_secs(30)).await?;
        let local_id =
            started(&events).ok_or_else(|| anyhow::anyhow!("no SessionStarted"))?.to_owned();
        h.send(AdapterCommand::Diagnose { local_id: local_id.clone(), request_id: DIAGNOSE }).await;
        let diag = h
            .until(|e| matches!(e, AdapterEvent::Diagnose { .. }), Duration::from_secs(10))
            .await?;
        let Some(AdapterEvent::Diagnose { report, .. }) = diag.last() else { unreachable!() };
        let agent_pid = report
            .acp
            .as_ref()
            .and_then(|a| a.agent_pid)
            .ok_or_else(|| anyhow::anyhow!("no agent pid in {report:#?}"))?;
        let grandchild: i32 = std::fs::read_to_string(&pid_file)?.trim().parse()?;
        anyhow::ensure!(pid_alive(i32::try_from(agent_pid)?), "agent alive before kill");
        anyhow::ensure!(pid_alive(grandchild), "grandchild alive before kill");
        h.send(AdapterCommand::Kill { local_id, signal: None }).await;
        h.until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(15))
            .await?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        anyhow::ensure!(!pid_alive(i32::try_from(agent_pid)?), "agent survived the kill");
        anyhow::ensure!(!pid_alive(grandchild), "the grandchild sleep survived the kill: orphan");
        h.finish().await;
        Ok(())
    }

    async fn permission_round_trip(allow: bool) -> anyhow::Result<()> {
        let mut h = start(&FAKE_ROW, &exe());
        let mut script = full_script();
        script["turn"] = json!("permission");
        h.spawn(&script, Some(PermissionMode::Ask), "do it").await;
        let events = h
            .until(|e| matches!(e, AdapterEvent::PermissionRequest { .. }), Duration::from_secs(30))
            .await?;
        let local_id =
            started(&events).ok_or_else(|| anyhow::anyhow!("no SessionStarted"))?.to_owned();
        let Some(AdapterEvent::PermissionRequest { request_id, tool, input, .. }) = events.last()
        else {
            unreachable!()
        };
        anyhow::ensure!(request_id == "call-1", "request id is the tool call id: {request_id}");
        anyhow::ensure!(tool == "read", "tool from kind: {tool}");
        anyhow::ensure!(input["path"] == ".", "raw input: {input}");
        h.send(AdapterCommand::PermissionResponse {
            local_id: local_id.clone(),
            request_id: request_id.clone(),
            allow,
            option_id: None,
        })
        .await;
        let rest = h.until(is_idle, Duration::from_secs(30)).await?;
        anyhow::ensure!(
            rest.iter().any(|e| matches!(e, AdapterEvent::PermissionResolved { .. })),
            "resolved: {rest:#?}"
        );
        let results = tool_uses(&rest, "tool_result");
        anyhow::ensure!(
            results.iter().any(|r| r["is_error"] == !allow),
            "tool result follows the answer (allow={allow}): {results:#?}"
        );
        h.send(AdapterCommand::Kill { local_id, signal: None }).await;
        h.until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(15))
            .await?;
        h.finish().await;
        Ok(())
    }

    pub async fn a_permission_request_is_answered_from_the_ui_path() -> anyhow::Result<()> {
        permission_round_trip(true).await
    }

    pub async fn a_rejected_permission_fails_the_tool() -> anyhow::Result<()> {
        permission_round_trip(false).await
    }

    fn answered(events: &[AdapterEvent]) -> Option<Value> {
        messages(events, "assistant").iter().find_map(|m| {
            m["content"]
                .as_str()?
                .strip_prefix("answered: ")
                .and_then(|j| serde_json::from_str(j).ok())
        })
    }

    pub async fn an_allow_always_pick_round_trips_its_option_id() -> anyhow::Result<()> {
        let mut h = start(&FAKE_ROW, &exe());
        let mut script = full_script();
        script["turn"] = json!("permission");
        h.spawn(&script, Some(PermissionMode::Ask), "do it").await;
        let events = h
            .until(|e| matches!(e, AdapterEvent::PermissionRequest { .. }), Duration::from_secs(30))
            .await?;
        let local_id =
            started(&events).ok_or_else(|| anyhow::anyhow!("no SessionStarted"))?.to_owned();
        let Some(AdapterEvent::PermissionRequest { request_id, options, .. }) = events.last()
        else {
            unreachable!()
        };
        let ids: Vec<(&str, &str, &str)> = options
            .iter()
            .map(|o| (o.option_id.as_str(), o.name.as_str(), o.kind.as_str()))
            .collect();
        anyhow::ensure!(
            ids == [
                ("allow-always", "Always", "allow_always"),
                ("allow-once", "Once", "allow_once"),
                ("reject-once", "No", "reject_once"),
            ],
            "the agent's options reach the clients: {ids:?}"
        );
        h.send(AdapterCommand::PermissionResponse {
            local_id: local_id.clone(),
            request_id: request_id.clone(),
            allow: true,
            option_id: Some("allow-always".to_owned()),
        })
        .await;
        let rest = h.until(is_idle, Duration::from_secs(30)).await?;
        let answer = answered(&rest).ok_or_else(|| anyhow::anyhow!("no echo: {rest:#?}"))?;
        anyhow::ensure!(
            answer == json!({ "outcome": { "outcome": "selected", "optionId": "allow-always" } }),
            "the picked option id reaches the agent: {answer}"
        );
        h.send(AdapterCommand::Kill { local_id, signal: None }).await;
        h.until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(15))
            .await?;
        h.finish().await;
        Ok(())
    }

    pub async fn an_elicitation_form_is_answered_with_typed_values() -> anyhow::Result<()> {
        let mut h = start(&FAKE_ROW, &exe());
        let mut script = full_script();
        script["turn"] = json!("elicit");
        h.spawn(&script, Some(PermissionMode::Ask), "deploy").await;
        let events = h
            .until(|e| matches!(e, AdapterEvent::AskQuestion { .. }), Duration::from_secs(30))
            .await?;
        let local_id =
            started(&events).ok_or_else(|| anyhow::anyhow!("no SessionStarted"))?.to_owned();
        let Some(AdapterEvent::AskQuestion { question, questions: Some(questions), .. }) =
            events.last()
        else {
            anyhow::bail!("no structured ask: {events:#?}")
        };
        anyhow::ensure!(question == "Deploy settings", "message as the question: {question}");
        let headers: Vec<&str> =
            questions.as_array().unwrap().iter().filter_map(|q| q["header"].as_str()).collect();
        anyhow::ensure!(headers == ["Environment", "Replicas", "Verbose"], "{questions}");
        anyhow::ensure!(questions[0]["options"][1]["label"] == "Production", "{questions}");
        h.send(AdapterCommand::Reply {
            local_id: local_id.clone(),
            text: "**Environment** — Environment\n→ Production\n\n**Replicas** — Replicas\n→ 3\n\n**Verbose** — Verbose\n→ No".to_owned(),
            ask_picks: None,
            env: std::collections::BTreeMap::new(),
            command_id: None,
            turn_id: None,
        })
        .await;
        let rest = h.until(is_idle, Duration::from_secs(30)).await?;
        anyhow::ensure!(
            rest.iter().any(|e| matches!(e, AdapterEvent::AskResolved { .. })),
            "the form closes: {rest:#?}"
        );
        let answer = answered(&rest).ok_or_else(|| anyhow::anyhow!("no echo: {rest:#?}"))?;
        anyhow::ensure!(
            answer
                == json!({ "action": "accept", "content": { "env": "prd", "replicas": 3, "verbose": false } }),
            "typed values reach the agent: {answer}"
        );
        h.send(AdapterCommand::Kill { local_id, signal: None }).await;
        h.until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(15))
            .await?;
        h.finish().await;
        Ok(())
    }

    pub async fn yolo_answers_permissions_itself() -> anyhow::Result<()> {
        let mut h = start(&FAKE_ROW, &exe());
        let mut script = full_script();
        script["turn"] = json!("permission");
        h.spawn(&script, Some(PermissionMode::Yolo), "do it").await;
        let events = h.until(is_idle, Duration::from_secs(30)).await?;
        anyhow::ensure!(
            !events.iter().any(|e| matches!(e, AdapterEvent::PermissionRequest { .. })),
            "yolo must not prompt: {events:#?}"
        );
        anyhow::ensure!(
            tool_uses(&events, "tool_result").iter().any(|r| r["is_error"] == false),
            "allow_once was picked: {events:#?}"
        );
        let local_id = started(&events).unwrap().to_owned();
        h.send(AdapterCommand::Kill { local_id, signal: None }).await;
        h.until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(15))
            .await?;
        h.finish().await;
        Ok(())
    }

    pub async fn an_inexpressible_mode_is_refused_before_spawn() -> anyhow::Result<()> {
        let mut h = start(&FAKE_ROW, &exe());
        h.spawn(&full_script(), Some(PermissionMode::Whip), "x").await;
        let events = h
            .until(|e| matches!(e, AdapterEvent::CommandResult { .. }), Duration::from_secs(10))
            .await?;
        let (ok, error) = command_result(&events, SPAWN).unwrap();
        anyhow::ensure!(!ok);
        let error = error.unwrap_or_default();
        anyhow::ensure!(error.contains("`whip`") && error.contains("refusing to spawn"), "{error}");
        anyhow::ensure!(started(&events).is_none(), "nothing may start: {events:#?}");
        h.finish().await;
        Ok(())
    }

    pub async fn a_missing_login_is_an_actionable_spawn_failure() -> anyhow::Result<()> {
        let mut h = start(&FAKE_ROW, &exe());
        let mut script = full_script();
        script["authRequired"] = json!(true);
        h.spawn(&script, None, "x").await;
        let events = h
            .until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(30))
            .await?;
        let Some(AdapterEvent::SessionEnded { reason: EndReason::SpawnFailed { detail }, .. }) =
            events.last()
        else {
            anyhow::bail!("expected SpawnFailed, got {events:#?}");
        };
        anyhow::ensure!(
            detail.contains("not logged in") && detail.contains("run `fake-acp-agent` once on"),
            "{detail}"
        );
        let (ok, error) =
            command_result(&events, SPAWN).ok_or_else(|| anyhow::anyhow!("no spawn result"))?;
        anyhow::ensure!(!ok && error.as_deref() == Some(detail.as_str()), "{error:?}");
        h.finish().await;
        Ok(())
    }

    pub async fn interrupt_cancels_a_stalled_turn() -> anyhow::Result<()> {
        let mut h = start(&FAKE_ROW, &exe());
        let mut script = full_script();
        script["turn"] = json!("stall");
        h.spawn(&script, None, "hang").await;
        let events = h
            .until(
                |e| matches!(e, AdapterEvent::Status { tempo: Some(t), .. } if t == "active"),
                Duration::from_secs(30),
            )
            .await?;
        let local_id =
            started(&events).ok_or_else(|| anyhow::anyhow!("no SessionStarted"))?.to_owned();
        h.send(AdapterCommand::Interrupt {
            local_id: local_id.clone(),
            command_id: Some(INTERRUPT),
        })
        .await;
        let rest = h.until(is_idle, Duration::from_secs(30)).await?;
        anyhow::ensure!(
            command_result(&rest, INTERRUPT) == Some((true, None)),
            "interrupt acked: {rest:#?}"
        );
        anyhow::ensure!(
            matches!(rest.last(), Some(AdapterEvent::Status { detail: Some(d), .. }) if d == "turn interrupted"),
            "cancelled stop reason: {rest:#?}"
        );
        anyhow::ensure!(
            messages(&rest, "assistant").iter().any(|m| m["content"] == "Hello world: hang"),
            "the partial message is flushed at turn end: {rest:#?}"
        );
        h.send(AdapterCommand::Kill { local_id, signal: None }).await;
        h.until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(15))
            .await?;
        h.finish().await;
        Ok(())
    }

    pub async fn config_options_populate_the_catalog_and_set_model_round_trips()
    -> anyhow::Result<()> {
        catalog_scenarios::config_options().await
    }

    pub async fn legacy_models_populate_the_catalog_and_set_model_round_trips() -> anyhow::Result<()>
    {
        catalog_scenarios::legacy().await
    }

    pub async fn a_reexec_reattaches_through_session_resume() -> anyhow::Result<()> {
        let mut script = full_script();
        script["capabilities"] = json!({ "resume": true });
        super::durable::reattach(&script, "session/resume").await
    }

    pub async fn a_reexec_reattaches_through_session_load_without_replaying_rows()
    -> anyhow::Result<()> {
        let mut script = full_script();
        script["capabilities"] = json!({ "loadSession": true });
        super::durable::reattach(&script, "session/load").await
    }

    pub async fn an_agent_that_cannot_resume_ends_the_session_as_not_resumable()
    -> anyhow::Result<()> {
        super::durable::not_resumable().await
    }

    pub async fn diagnose_carries_agent_info_and_the_rings() -> anyhow::Result<()> {
        let mut h = start(&FAKE_ROW, &exe());
        h.spawn(&full_script(), Some(PermissionMode::Auto), "ping").await;
        let events = h.until(is_idle, Duration::from_secs(30)).await?;
        let local_id = started(&events).unwrap().to_owned();
        h.send(AdapterCommand::Diagnose { local_id: local_id.clone(), request_id: DIAGNOSE }).await;
        let diag = h
            .until(|e| matches!(e, AdapterEvent::Diagnose { .. }), Duration::from_secs(10))
            .await?;
        let Some(AdapterEvent::Diagnose { report, request_id, .. }) = diag.last() else {
            unreachable!()
        };
        anyhow::ensure!(*request_id == DIAGNOSE);
        anyhow::ensure!(report.adapter == "fake-acp");
        let acp = report.acp.as_ref().ok_or_else(|| anyhow::anyhow!("no acp section"))?;
        anyhow::ensure!(acp.agent_version.as_deref() == Some("9.9.9"), "{acp:#?}");
        anyhow::ensure!(acp.agent_name.as_deref() == Some("fake-acp"), "{acp:#?}");
        anyhow::ensure!(acp.live && acp.turn_status == "idle", "{acp:#?}");
        anyhow::ensure!(acp.current_mode.as_deref() == Some("autoEdit"), "{acp:#?}");
        anyhow::ensure!(acp.available_modes == ["default", "autoEdit", "yolo"], "{acp:#?}");
        anyhow::ensure!(acp.last_cost_usd == Some(0.0042), "{acp:#?}");
        anyhow::ensure!(
            acp.rpc_tail.iter().any(|f| f.label == "initialize" && f.direction == "out"),
            "{:#?}",
            acp.rpc_tail
        );
        anyhow::ensure!(
            acp.rpc_tail.iter().any(|f| f.label == "session/update" && f.direction == "in"),
            "{:#?}",
            acp.rpc_tail
        );
        anyhow::ensure!(
            acp.stderr_tail.iter().any(|l| l.line.contains("fake-acp: starting")),
            "{:#?}",
            acp.stderr_tail
        );
        anyhow::ensure!(
            report.permission_mode.value.as_deref() == Some("autoEdit"),
            "{:?}",
            report.permission_mode
        );
        h.send(AdapterCommand::Kill { local_id, signal: None }).await;
        h.until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(15))
            .await?;
        h.finish().await;
        Ok(())
    }
}

mod durable {
    use super::{
        AdapterEvent, Duration, EndReason, PermissionMode, REPLY, SPAWN, Value, argv_row,
        command_result, full_script, is_idle, messages, start, started,
    };

    fn spawn_spec(
        h: &super::Harness,
        mode: Option<PermissionMode>,
    ) -> cctui_proto::adapter::SessionSpec {
        let mut spec = h.spec(&Value::Null, mode, "ping");
        spec.env.clear();
        spec
    }

    fn started_at(events: &[AdapterEvent]) -> Option<u64> {
        events.iter().find_map(|e| match e {
            AdapterEvent::SessionStarted { meta, .. } => meta.extra["started_at_ms"].as_u64(),
            _ => None,
        })
    }

    fn texts(events: &[AdapterEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                AdapterEvent::Message { payload, .. } => {
                    payload["content"].as_str().map(str::to_owned)
                }
                _ => None,
            })
            .collect()
    }

    pub async fn reattach(script: &Value, method: &str) -> anyhow::Result<()> {
        let exe = std::env::current_exe().unwrap().display().to_string();
        let mut h = start(argv_row(script), &exe);
        let spec = spawn_spec(&h, Some(PermissionMode::Auto));
        h.send(cctui_proto::adapter::AdapterCommand::Spawn {
            spec,
            command_id: Some(SPAWN),
            session_id: None,
        })
        .await;
        let first = h.until(is_idle, Duration::from_secs(30)).await?;
        let local_id = started(&first)
            .ok_or_else(|| anyhow::anyhow!("no SessionStarted: {first:#?}"))?
            .to_owned();
        let born = started_at(&first).ok_or_else(|| anyhow::anyhow!("no start time"))?;
        let pid = h.agent_pid(&local_id).await?;
        anyhow::ensure!(h.store.get(&local_id).is_some(), "a live session is recorded");

        let mut h = h.restart(pid).await?;
        let back = h
            .until(|e| matches!(e, AdapterEvent::SessionStarted { .. }), Duration::from_secs(30))
            .await?;
        anyhow::ensure!(
            started(&back) == Some(local_id.as_str()),
            "same session listed: {back:#?}"
        );
        anyhow::ensure!(started_at(&back) == Some(born), "start time survives: {back:#?}");
        anyhow::ensure!(
            !back.iter().any(|e| matches!(e, AdapterEvent::SessionEnded { .. })),
            "not ended: {back:#?}"
        );
        let report = h.diagnose(&local_id).await?;
        let acp = report.acp.as_ref().ok_or_else(|| anyhow::anyhow!("no acp section"))?;
        anyhow::ensure!(acp.agent_pid.is_some_and(|p| p != pid), "a fresh agent: {acp:#?}");
        anyhow::ensure!(
            acp.rpc_tail.iter().any(|f| f.label == method && f.direction == "out"),
            "re-attached with {method}: {:#?}",
            acp.rpc_tail
        );
        anyhow::ensure!(
            !acp.rpc_tail.iter().any(|f| f.label == "session/new"),
            "no new session: {:#?}",
            acp.rpc_tail
        );
        anyhow::ensure!(
            acp.rpc_tail.iter().any(|f| f.label == "session/set_mode" && f.direction == "out"),
            "the permission mode is re-applied: {:#?}",
            acp.rpc_tail
        );
        anyhow::ensure!(acp.current_mode.as_deref() == Some("autoEdit"), "{acp:#?}");

        h.reply(&local_id, "again").await;
        let second = h.until(is_idle, Duration::from_secs(30)).await?;
        anyhow::ensure!(command_result(&second, REPLY) == Some((true, None)), "{second:#?}");
        let mut rows = texts(&back);
        rows.extend(texts(&second));
        let assistant: Vec<&Value> = messages(&second, "assistant");
        anyhow::ensure!(
            assistant.len() == 1 && assistant[0]["content"] == "Hello world: again",
            "exactly the follow-up's answer: {assistant:#?}"
        );
        anyhow::ensure!(
            !rows.iter().any(|t| t.contains("replayed") || t.contains("ping")),
            "no earlier row re-emitted: {rows:#?}"
        );
        h.send(cctui_proto::adapter::AdapterCommand::Kill {
            local_id: local_id.clone(),
            signal: None,
        })
        .await;
        h.until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(15))
            .await?;
        anyhow::ensure!(h.store.get(&local_id).is_none(), "a killed session is forgotten");
        h.finish().await;
        Ok(())
    }

    pub async fn not_resumable() -> anyhow::Result<()> {
        let exe = std::env::current_exe().unwrap().display().to_string();
        let mut h = start(argv_row(&full_script()), &exe);
        let spec = spawn_spec(&h, None);
        h.send(cctui_proto::adapter::AdapterCommand::Spawn {
            spec,
            command_id: Some(SPAWN),
            session_id: None,
        })
        .await;
        let first = h.until(is_idle, Duration::from_secs(30)).await?;
        let local_id =
            started(&first).ok_or_else(|| anyhow::anyhow!("no SessionStarted"))?.to_owned();
        let pid = h.agent_pid(&local_id).await?;
        let mut h = h.restart(pid).await?;
        let events = h
            .until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(30))
            .await?;
        let Some(AdapterEvent::SessionEnded {
            local_id: ended,
            reason: EndReason::ResumeFailed { detail },
        }) = events.last()
        else {
            anyhow::bail!("expected ResumeFailed: {events:#?}");
        };
        anyhow::ensure!(*ended == local_id, "{ended}");
        anyhow::ensure!(detail.contains("not resumable"), "{detail}");
        anyhow::ensure!(started(&events).is_none(), "never listed again: {events:#?}");
        anyhow::ensure!(h.store.get(&local_id).is_none(), "dropped from the registry");
        h.finish().await;
        Ok(())
    }
}

mod catalog_scenarios {
    use super::{
        AdapterCommand, AdapterEvent, DIAGNOSE, Duration, FAKE_ROW, Uuid, Value, command_result,
        full_script, is_idle, json, start, started,
    };

    const SET_MODEL: Uuid = Uuid::from_u128(0x55);

    fn config_options_json() -> Value {
        json!([
            {
                "id": "model", "name": "Model", "category": "model", "type": "select",
                "currentValue": "flash",
                "options": [
                    { "value": "pro", "name": "Pro" },
                    { "value": "flash", "name": "Flash", "description": "fast" },
                ],
            },
            {
                "id": "thinking", "name": "Thinking level", "category": "thought_level", "type": "select",
                "currentValue": "medium",
                "options": [{ "value": "low", "name": "Low" }, { "value": "high", "name": "High" }],
            },
        ])
    }

    fn legacy_models() -> Value {
        json!({
            "currentModelId": "gemini-2.5-pro",
            "availableModels": [
                { "modelId": "gemini-2.5-pro", "name": "Gemini 2.5 Pro" },
                { "modelId": "gemini-2.5-flash", "name": "Gemini 2.5 Flash" },
            ],
        })
    }

    fn catalogs(events: &[AdapterEvent]) -> Vec<&cctui_proto::codex_catalog::CodexModelCatalog> {
        events
            .iter()
            .filter_map(|e| match e {
                AdapterEvent::HarnessModels { adapter_id, catalog }
                    if adapter_id == FAKE_ROW.id =>
                {
                    Some(catalog)
                }
                _ => None,
            })
            .collect()
    }

    pub async fn run(
        script: Value,
        expect_ids: &[&str],
        initial: &str,
        switch_to: &str,
        effort: Option<&str>,
    ) -> anyhow::Result<()> {
        let exe = std::env::current_exe().unwrap().display().to_string();
        let mut h = start(&FAKE_ROW, &exe);
        h.spawn(&script, None, "ping").await;
        let events = h.until(is_idle, Duration::from_secs(30)).await?;
        let local_id =
            started(&events).ok_or_else(|| anyhow::anyhow!("no SessionStarted"))?.to_owned();
        let first = catalogs(&events);
        anyhow::ensure!(!first.is_empty(), "a HarnessModels event at spawn: {events:#?}");
        let ids: Vec<&str> = first[0].models.iter().map(|m| m.id.as_str()).collect();
        anyhow::ensure!(ids == expect_ids, "catalog ids {ids:?} != {expect_ids:?}");
        anyhow::ensure!(
            first[0].models.iter().any(|m| m.is_default && m.id == initial),
            "the current model is the default: {:#?}",
            first[0]
        );
        anyhow::ensure!(
            events
                .iter()
                .any(|e| matches!(e, AdapterEvent::SessionModel { model, .. } if model == initial)),
            "session model reported: {events:#?}"
        );
        h.send(AdapterCommand::SetModel {
            local_id: local_id.clone(),
            model: Some(switch_to.to_owned()),
            effort: effort.map(str::to_owned),
            command_id: Some(SET_MODEL),
        })
        .await;
        let rest = h
            .until(|e| matches!(e, AdapterEvent::CommandResult { command_id, .. } if *command_id == SET_MODEL), Duration::from_secs(15))
            .await?;
        anyhow::ensure!(
            command_result(&rest, SET_MODEL) == Some((true, None)),
            "set_model acked: {rest:#?}"
        );
        anyhow::ensure!(
            rest.iter().any(
                |e| matches!(e, AdapterEvent::Status { model: Some(m), .. } if m == switch_to)
            ),
            "the new model is reported on the card: {rest:#?}"
        );
        let after = catalogs(&rest);
        anyhow::ensure!(
            after
                .last()
                .is_some_and(|c| c.models.iter().any(|m| m.is_default && m.id == switch_to)),
            "the catalog is re-reported with the new default: {after:#?}"
        );
        if let Some(effort) = effort {
            anyhow::ensure!(
                after.last().is_some_and(|c| c.models[0].default_effort == effort),
                "the effort travels with the catalog: {after:#?}"
            );
        }
        h.send(AdapterCommand::Diagnose { local_id: local_id.clone(), request_id: DIAGNOSE }).await;
        let diag = h
            .until(|e| matches!(e, AdapterEvent::Diagnose { .. }), Duration::from_secs(10))
            .await?;
        let Some(AdapterEvent::Diagnose { report, .. }) = diag.last() else { unreachable!() };
        anyhow::ensure!(
            report.acp.as_ref().and_then(|a| a.model.as_deref()) == Some(switch_to),
            "diagnose shows the switched model: {:#?}",
            report.acp
        );
        h.send(AdapterCommand::Kill { local_id, signal: None }).await;
        h.until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(15))
            .await?;
        h.finish().await;
        Ok(())
    }

    pub async fn config_options() -> anyhow::Result<()> {
        let mut script = full_script();
        script["configOptions"] = config_options_json();
        run(script, &["pro", "flash"], "flash", "pro", Some("high")).await
    }

    pub async fn legacy() -> anyhow::Result<()> {
        let mut script = full_script();
        script["models"] = legacy_models();
        run(
            script,
            &["gemini-2.5-pro", "gemini-2.5-flash"],
            "gemini-2.5-pro",
            "gemini-2.5-flash",
            None,
        )
        .await
    }
}

/// `--ignored`: a real `opencode acp` on a free Zen model. Needs opencode on
/// `PATH` (or `CCTUI_ACP_REAL_BIN`) and a login; `CCTUI_ACP_REAL_MODEL`
/// names the model.
mod real_agent {
    use super::{
        AdapterCommand, AdapterEvent, AgentRow, BTreeMap, DIAGNOSE, Duration, ModeTable, REPLY,
        SPAWN, Value, command_result, is_idle, messages, pid_alive, start, started, tool_uses,
    };

    static OPENCODE_ROW: AgentRow = AgentRow {
        id: "opencode-acp-probe",
        label: "opencode over ACP (probe)",
        bin: "opencode",
        args: &["acp"],
        update: None,
        modes: ModeTable { ask: None, auto: None, yolo: None, whip: None },
        legacy_models: false,
    };

    pub async fn run() -> anyhow::Result<()> {
        let bin = std::env::var("CCTUI_ACP_REAL_BIN").unwrap_or_else(|_| "opencode".to_owned());
        let mut h = start(&OPENCODE_ROW, &bin);
        let mut spec = h.spec(&Value::Null, None, "Reply with the single word: pong");
        spec.env.clear();
        spec.model = std::env::var("CCTUI_ACP_REAL_MODEL").ok();
        h.commands
            .send(AdapterCommand::Spawn { spec, command_id: Some(SPAWN), session_id: None })
            .await
            .unwrap();
        let events = h.until(is_idle, Duration::from_mins(3)).await?;
        let local_id = started(&events)
            .ok_or_else(|| anyhow::anyhow!("no SessionStarted: {events:#?}"))?
            .to_owned();
        anyhow::ensure!(
            !messages(&events, "assistant").is_empty(),
            "an assistant message: {events:#?}"
        );
        anyhow::ensure!(
            matches!(events.last(), Some(AdapterEvent::Status { detail: None, .. })),
            "stopReason end_turn: {:?}",
            events.last()
        );
        anyhow::ensure!(
            events.iter().any(|e| matches!(e, AdapterEvent::TokenUsage { .. })),
            "token usage: {events:#?}"
        );
        h.send(AdapterCommand::Diagnose { local_id: local_id.clone(), request_id: DIAGNOSE }).await;
        let diag = h
            .until(|e| matches!(e, AdapterEvent::Diagnose { .. }), Duration::from_secs(10))
            .await?;
        let Some(AdapterEvent::Diagnose { report, .. }) = diag.last() else { unreachable!() };
        let acp = report.acp.as_ref().ok_or_else(|| anyhow::anyhow!("no acp section"))?;
        anyhow::ensure!(acp.last_cost_usd.is_some(), "a usage_update with cost: {acp:#?}");
        anyhow::ensure!(acp.agent_version.is_some(), "agentInfo.version: {acp:#?}");
        h.send(AdapterCommand::Reply {
            local_id: local_id.clone(),
            text: "And now: ping".to_owned(),
            ask_picks: None,
            env: BTreeMap::new(),
            command_id: Some(REPLY),
            turn_id: None,
        })
        .await;
        let second = h.until(is_idle, Duration::from_mins(3)).await?;
        anyhow::ensure!(command_result(&second, REPLY) == Some((true, None)), "{second:#?}");
        std::fs::write(
            std::path::Path::new(&h.cwd).join("secret.txt"),
            "the codeword is MARMALADE\n",
        )?;
        h.send(AdapterCommand::Reply {
            local_id: local_id.clone(),
            text: "Use your file-read tool on secret.txt in the working directory and tell me the codeword.".to_owned(),
            ask_picks: None,
            env: BTreeMap::new(),
            command_id: None,
            turn_id: None,
        })
        .await;
        let third = h.until(is_idle, Duration::from_mins(3)).await?;
        anyhow::ensure!(
            tool_uses(&third, "tool_call")
                .iter()
                .any(|t| t["input"].as_object().is_some_and(|m| !m.is_empty())),
            "a tool call with its input: {third:#?}"
        );
        anyhow::ensure!(
            messages(&third, "assistant").iter().any(|m| m.to_string().contains("MARMALADE")),
            "the codeword in the answer: {third:#?}"
        );
        for e in &third {
            if let AdapterEvent::ToolUse { payload, .. } | AdapterEvent::Message { payload, .. } = e
            {
                println!("    {payload}");
            }
        }
        let pid = acp.agent_pid.ok_or_else(|| anyhow::anyhow!("no agent pid: {acp:#?}"))?;
        let pid = i32::try_from(pid)?;
        anyhow::ensure!(pid_alive(pid), "agent alive before kill");
        h.send(AdapterCommand::Kill { local_id, signal: None }).await;
        h.until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(15))
            .await?;
        anyhow::ensure!(!pid_alive(pid), "agent pid {pid} survived the kill");
        h.finish().await;
        Ok(())
    }
    /// Resume in a fresh agent process continues the conversation: the
    /// codeword only exists in the turn the first process ran.
    pub async fn resume() -> anyhow::Result<()> {
        let bin = std::env::var("CCTUI_ACP_REAL_BIN").unwrap_or_else(|_| "opencode".to_owned());
        let mut h = start(&OPENCODE_ROW, &bin);
        let mut spec = h.spec(
            &Value::Null,
            None,
            "Remember this codeword for later: PAPAYA-42. Reply with just: noted",
        );
        spec.env.clear();
        spec.model = std::env::var("CCTUI_ACP_REAL_MODEL").ok();
        h.send(AdapterCommand::Spawn { spec, command_id: Some(SPAWN), session_id: None }).await;
        let first = h.until(is_idle, Duration::from_mins(3)).await?;
        let local_id = started(&first)
            .ok_or_else(|| anyhow::anyhow!("no SessionStarted: {first:#?}"))?
            .to_owned();
        let pid = h.agent_pid(&local_id).await?;
        let mut h = h.restart(pid).await?;
        let back = h
            .until(
                |e| {
                    matches!(
                        e,
                        AdapterEvent::SessionStarted { .. } | AdapterEvent::SessionEnded { .. }
                    )
                },
                Duration::from_mins(2),
            )
            .await?;
        anyhow::ensure!(started(&back) == Some(local_id.as_str()), "re-attached: {back:#?}");
        let report = h.diagnose(&local_id).await?;
        let acp = report.acp.as_ref().ok_or_else(|| anyhow::anyhow!("no acp section"))?;
        let method = acp
            .rpc_tail
            .iter()
            .find(|f| {
                f.direction == "out" && (f.label == "session/resume" || f.label == "session/load")
            })
            .map(|f| f.label.clone());
        println!("    re-attached through {method:?}");
        h.reply(&local_id, "What was the codeword I asked you to remember? Reply with it only.")
            .await;
        let second = h.until(is_idle, Duration::from_mins(3)).await?;
        anyhow::ensure!(command_result(&second, REPLY) == Some((true, None)), "{second:#?}");
        let answer: Vec<&Value> = messages(&second, "assistant");
        println!("    answer: {answer:?}");
        anyhow::ensure!(
            answer.iter().any(|m| m.to_string().contains("PAPAYA-42")),
            "the resumed session remembers the codeword: {second:#?}"
        );
        anyhow::ensure!(
            !messages(&second, "user")
                .iter()
                .any(|m| m.to_string().contains("Remember this codeword")),
            "the first prompt is not replayed into the transcript: {second:#?}"
        );
        h.send(AdapterCommand::Kill { local_id, signal: None }).await;
        h.until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(15))
            .await?;
        h.finish().await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The CctuiAgent relay, end to end against a recording server
// ---------------------------------------------------------------------------

mod relay {
    use super::{
        Duration, MACHINE_KEY, PermissionMode, SPAWN, ServerClient, SessionStore, Value, argv_row,
        command_result, is_idle, json, messages, start_in, started,
    };
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    const REFUSAL: &str = "refused by the recording server";

    /// A daemon API that grants spawn rights, then records and refuses every
    /// `spawn-child` so the call returns without a child to follow.
    async fn recording_server() -> (String, mpsc::UnboundedReceiver<(String, Value)>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(answer(stream, tx.clone()));
            }
        });
        (url, rx)
    }

    async fn answer(stream: tokio::net::TcpStream, seen: mpsc::UnboundedSender<(String, Value)>) {
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).await.is_err() {
            return;
        }
        let mut length = 0;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).await.unwrap_or(0) == 0 || header.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0; length];
        let _ = reader.read_exact(&mut body).await;
        let path = request_line.split_whitespace().nth(1).unwrap_or_default().to_owned();
        let (status, reply) = if path.ends_with("/gateway-env") {
            (
                "200 OK",
                json!({ "account_bound": false, "spawn_capability": { "adapters": ["claude-code"] } }),
            )
        } else if path.ends_with("/spawn-child") {
            ("403 Forbidden", json!({ "error": REFUSAL }))
        } else {
            ("404 Not Found", json!({}))
        };
        let _ = seen.send((path, serde_json::from_slice(&body).unwrap_or(Value::Null)));
        let reply = reply.to_string();
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
            reply.len()
        );
        let _ = reader.into_inner().write_all(response.as_bytes()).await;
    }

    fn spawn_children(seen: &mut mpsc::UnboundedReceiver<(String, Value)>) -> Vec<(String, Value)> {
        std::iter::from_fn(|| seen.try_recv().ok())
            .filter(|(path, _)| path.ends_with("/spawn-child"))
            .collect()
    }

    fn relayed(events: &[cctui_proto::adapter::AdapterEvent]) -> anyhow::Result<()> {
        let assistant = messages(events, "assistant");
        anyhow::ensure!(
            assistant.iter().any(|m| m["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("relay: ") && t.contains(REFUSAL))),
            "the server's answer travels back through the relay: {assistant:#?}"
        );
        Ok(())
    }

    pub async fn run(reattach: bool) -> anyhow::Result<()> {
        let (url, mut seen) = recording_server().await;
        let server = ServerClient::new(url);
        let stop = CancellationToken::new();
        let sock = cctui_daemon::agenttool::socket_for_launch().to_path_buf();
        tokio::spawn(cctui_daemon::agenttool::serve(
            sock,
            server.clone(),
            MACHINE_KEY.to_owned(),
            stop.clone(),
        ));

        let script = json!({ "turn": "relay", "capabilities": { "resume": true } });
        let exe = std::env::current_exe().unwrap().display().to_string();
        let tmp = tempfile::tempdir()?;
        let store = Arc::new(SessionStore::at(tmp.path().join("acp-sessions.json")));
        let mut h = start_in(argv_row(&script), &exe, tmp, store, Some(server));
        h.spawn(&script, Some(PermissionMode::Auto), "spawn a child").await;
        let events = h.until(is_idle, Duration::from_secs(45)).await?;
        let local_id = started(&events)
            .ok_or_else(|| anyhow::anyhow!("no SessionStarted: {events:#?}"))?
            .to_owned();
        anyhow::ensure!(command_result(&events, SPAWN) == Some((true, None)), "{events:#?}");
        relayed(&events)?;
        let calls = spawn_children(&mut seen);
        anyhow::ensure!(calls.len() == 1, "one spawn-child reached the server: {calls:#?}");
        let (path, body) = &calls[0];
        anyhow::ensure!(
            path == &format!("/api/v1/daemon/sessions/{local_id}/spawn-child"),
            "the launch key resolves onto the agent's session id: {path}"
        );
        anyhow::ensure!(
            body["prompt"] == "hello child" && body["adapter"] == "claude-code",
            "{body:#}"
        );
        anyhow::ensure!(
            h.store.get(&local_id).is_some_and(|r| r.spawn_relay),
            "the record remembers the relay for a re-attach"
        );

        if reattach {
            let pid = h.agent_pid(&local_id).await?;
            h = h.restart(pid).await?;
            h.until(
                |e| matches!(e, cctui_proto::adapter::AdapterEvent::SessionStarted { .. }),
                Duration::from_secs(30),
            )
            .await?;
            h.reply(&local_id, "again").await;
            let events = h.until(is_idle, Duration::from_secs(45)).await?;
            relayed(&events)?;
            let calls = spawn_children(&mut seen);
            anyhow::ensure!(
                calls.len() == 1
                    && calls[0].0 == format!("/api/v1/daemon/sessions/{local_id}/spawn-child"),
                "the re-attached agent reaches the server through a re-declared relay: {calls:#?}"
            );
        }
        h.finish().await;
        stop.cancel();
        Ok(())
    }
}
