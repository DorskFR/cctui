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
//! `cargo test -p cctui-daemon --test acp_fake_agent -- --ignored` runs the
//! real-agent probe against `opencode acp` instead.

use std::collections::BTreeMap;
use std::time::Duration;

use cctui_daemon::adapter_runtime::{Adapter, AdapterCtx};
use cctui_daemon::adapters::acp::AcpAdapter;
use cctui_daemon::adapters::acp::modes::{GEMINI_STYLE, ModeTable};
use cctui_daemon::adapters::acp::rows::AgentRow;
use cctui_proto::adapter::{
    AdapterCommand, AdapterEvent, AdapterId, EndReason, PermissionMode, SessionSpec,
};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const AGENT_ENV: &str = "FAKE_ACP_AGENT";
const SCRIPT_ENV: &str = "FAKE_ACP_SCRIPT";
const SPAWN: Uuid = Uuid::from_u128(0x51);
const REPLY: Uuid = Uuid::from_u128(0x52);
const INTERRUPT: Uuid = Uuid::from_u128(0x53);
const DIAGNOSE: Uuid = Uuid::from_u128(0x54);

fn main() {
    if std::env::var_os(AGENT_ENV).is_some() {
        fake_agent::main();
        return;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list") {
        for (name, _) in scenarios::ALL {
            println!("{name}: test");
        }
        return;
    }
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

    use super::SCRIPT_ENV;

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
        let script = Script(
            std::env::var(SCRIPT_ENV)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(Value::Object(serde_json::Map::new())),
        );
        eprintln!("fake-acp: starting");
        let (tx, mut rx) = mpsc::unbounded_channel::<Inbound>();
        tokio::spawn(async move {
            let mut lines = BufReader::new(tokio::io::stdin()).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if tx.send(Inbound::Line(line)).is_err() {
                            return;
                        }
                    }
                    _ => {
                        let _ = tx.send(Inbound::Eof);
                        return;
                    }
                }
            }
        });
        let mut agent = Agent {
            script,
            out: tokio::io::stdout(),
            next_id: 1000,
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
            let child = std::process::Command::new("sleep").arg("300").spawn().expect("sleep");
            std::fs::write(path, child.id().to_string()).unwrap();
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

    struct Agent {
        script: Script,
        out: tokio::io::Stdout,
        next_id: u64,
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
                    self.respond(id, result).await;
                }
                "session/new" => {
                    if self.script.flag("authRequired") {
                        self.respond_error(id, -32000, "Authentication required").await;
                        return true;
                    }
                    let mut result = json!({ "sessionId": "fake-session-1" });
                    if let Some(modes) = self.script.get("modes") {
                        result["modes"] = modes.clone();
                    }
                    if let Some(models) = self.script.get("models") {
                        result["models"] = models.clone();
                    }
                    if !self.config_options.is_empty() {
                        result["configOptions"] = Value::Array(self.config_options.clone());
                    }
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

        async fn on_prompt(&mut self, id: Value, session_id: &str, text: &str) {
            let turn = self.script.turn();
            self.update(
                session_id,
                json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": "Hello " },
                    "messageId": "msg-1",
                }),
            )
            .await;
            self.update(
                session_id,
                json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": format!("world: {text}") },
                    "messageId": "msg-1",
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
                    "messageId": "msg-1",
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
                    "size": 100000,
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
            let picked =
                msg.pointer("/result/outcome/optionId").and_then(Value::as_str).unwrap_or_default();
            let rejected = !picked.starts_with("allow");
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
    _tmp: tempfile::TempDir,
    cwd: String,
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
        server: None,
        machine_key: None,
        connected,
    };
    let adapter = AcpAdapter { row };
    let task = tokio::spawn(async move { adapter.start(ctx).await });
    Harness { events, commands, shutdown, _connected: connected_tx, task, _tmp: tmp, cwd }
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
    use super::*;

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
        scenario!(an_inexpressible_mode_is_refused_before_spawn),
        scenario!(a_missing_login_is_an_actionable_spawn_failure),
        scenario!(interrupt_cancels_a_stalled_turn),
        scenario!(diagnose_carries_agent_info_and_the_rings),
        scenario!(config_options_populate_the_catalog_and_set_model_round_trips),
        scenario!(legacy_models_populate_the_catalog_and_set_model_round_trips),
    ];

    fn exe() -> String {
        std::env::current_exe().unwrap().display().to_string()
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

mod catalog_scenarios {
    use super::*;

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
    use super::*;

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
        let events = h.until(is_idle, Duration::from_secs(180)).await?;
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
        let second = h.until(is_idle, Duration::from_secs(180)).await?;
        anyhow::ensure!(command_result(&second, REPLY) == Some((true, None)), "{second:#?}");
        h.send(AdapterCommand::Kill { local_id, signal: None }).await;
        h.until(|e| matches!(e, AdapterEvent::SessionEnded { .. }), Duration::from_secs(15))
            .await?;
        h.finish().await;
        Ok(())
    }
}
