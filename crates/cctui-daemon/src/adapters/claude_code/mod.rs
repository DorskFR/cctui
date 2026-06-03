//! Claude Code adapter.
//!
//! Two paths coexist behind a runtime feature flag:
//!
//! - **`claude daemon` client (default)** — connects to the supervisor
//!   socket at `/tmp/cc-daemon-<uid>/<hash>/control.sock`, polls `list`,
//!   merges identity from `~/.claude/jobs/<short>/state.json`, and emits
//!   real `AdapterEvent`s.
//! - **Legacy uds** — a Unix domain socket at `$CCTUI_DAEMON_SOCK` (or
//!   `$XDG_RUNTIME_DIR/cctui-daemon.sock`) accepts line-delimited
//!   [`AdapterEvent`] JSON from clients. Opt in via
//!   `CCTUI_ADAPTER_CLAUDE_DAEMON=0` or `config.mode = "legacy"`. Kept
//!   until CCT-87 retires it.

mod attach;
mod backfill;
mod control;
mod discovery;
mod kickstart;
mod socket;
mod state;
mod transcript;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cctui_proto::adapter::AdapterEvent;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;
use tokio_util::sync::CancellationToken;

use crate::adapter_runtime::{Adapter, AdapterCtx, AdapterFactory};

/// Shared `session_id → stable local_id` map, populated by the control driver
/// as it pins transcripts and read by the ask-hook listener to translate the
/// live `session_id` a hook reports into the `local_id` the server keys on.
pub(crate) type SessionMap = Arc<Mutex<HashMap<String, String>>>;

pub struct ClaudeCodeAdapter;

#[async_trait::async_trait]
impl Adapter for ClaudeCodeAdapter {
    fn id(&self) -> &'static str {
        "claude-code"
    }

    async fn start(&self, ctx: AdapterCtx) -> anyhow::Result<()> {
        if use_claude_daemon_path(&ctx.config) {
            tracing::info!("claude-code adapter starting in claude-daemon mode");
            let cfg = control::DriverConfig::from_value(&ctx.config);
            let driver =
                control::Driver::new(cfg, ctx.events.clone(), ctx.commands, ctx.shutdown.clone());
            // The `AskUserQuestion` PreToolUse hook (CCT-167) delivers the
            // pending question here over the daemon's local socket. The hook
            // reports claude's live `session_id`; the driver's shared map
            // translates it to the stable `local_id` the rest of the pipeline
            // (and the server) keys on.
            let hook_sock = resolve_legacy_socket_path(&ctx.config);
            let hook_events = ctx.events;
            let hook_shutdown = ctx.shutdown;
            let session_map = driver.session_map();
            tokio::spawn(async move {
                if let Err(err) =
                    run_hook_listener(hook_sock, hook_events, hook_shutdown, session_map).await
                {
                    tracing::warn!(%err, "claude-code ask-hook listener exited");
                }
            });
            return driver.run().await;
        }
        run_legacy_uds(ctx).await
    }
}

fn use_claude_daemon_path(config: &serde_json::Value) -> bool {
    match config.get("mode").and_then(|v| v.as_str()) {
        Some("claude-daemon") => return true,
        Some("legacy") => return false,
        _ => {}
    }
    !matches!(std::env::var("CCTUI_ADAPTER_CLAUDE_DAEMON").as_deref(), Ok("0" | "false"))
}

async fn run_legacy_uds(ctx: AdapterCtx) -> anyhow::Result<()> {
    let path = resolve_legacy_socket_path(&ctx.config);
    let _ = std::fs::remove_file(&path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&path, perms);
    }
    tracing::info!(socket = %path.display(), "claude-code legacy uds adapter listening");

    loop {
        tokio::select! {
            () = ctx.shutdown.cancelled() => {
                let _ = std::fs::remove_file(&path);
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, _) = accept?;
                let events = ctx.events.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle_legacy_connection(stream, events).await {
                        tracing::warn!(%err, "claude-code uds connection error");
                    }
                });
            }
        }
    }
}

async fn handle_legacy_connection(
    stream: tokio::net::UnixStream,
    events: tokio::sync::mpsc::Sender<AdapterEvent>,
) -> anyhow::Result<()> {
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let evt: AdapterEvent = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(%err, ?line, "ignoring non-AdapterEvent uds line");
                continue;
            }
        };
        if events.send(evt).await.is_err() {
            break;
        }
    }
    Ok(())
}

/// Listen on the daemon's local socket for `AskUserQuestion` hook deliveries
/// (CCT-167). Each line is a `{kind, session_id, question?}` message from the
/// `cctui-daemon ask-hook` command; we translate `session_id → local_id` via
/// the shared map and emit the existing `AskQuestion` / `AskResolved` events.
async fn run_hook_listener(
    path: PathBuf,
    events: tokio::sync::mpsc::Sender<AdapterEvent>,
    shutdown: CancellationToken,
    session_map: SessionMap,
) -> anyhow::Result<()> {
    let _ = std::fs::remove_file(&path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    tracing::info!(socket = %path.display(), "claude-code ask-hook listener ready");

    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                let _ = std::fs::remove_file(&path);
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, _) = accept?;
                let events = events.clone();
                let session_map = session_map.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle_hook_connection(stream, events, session_map).await {
                        tracing::debug!(%err, "ask-hook connection error");
                    }
                });
            }
        }
    }
}

async fn handle_hook_connection(
    stream: tokio::net::UnixStream,
    events: tokio::sync::mpsc::Sender<AdapterEvent>,
    session_map: SessionMap,
) -> anyhow::Result<()> {
    let mut lines = BufReader::new(stream).lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(evt) = hook_line_to_event(line, &session_map) else {
            continue;
        };
        if events.send(evt).await.is_err() {
            break;
        }
    }
    Ok(())
}

/// Parse one hook line and resolve it to an `AdapterEvent`. The hook reports
/// claude's live `session_id`; we map it to the stable `local_id` (falling
/// back to the `session_id` itself before the driver has pinned the session).
fn hook_line_to_event(line: &str, session_map: &SessionMap) -> Option<AdapterEvent> {
    let v: serde_json::Value = serde_json::from_str(line)
        .map_err(|err| tracing::warn!(%err, ?line, "ignoring malformed ask-hook line"))
        .ok()?;
    let session_id = v.get("session_id").and_then(|s| s.as_str())?;
    let local_id = session_map
        .lock()
        .ok()
        .and_then(|m| m.get(session_id).cloned())
        .unwrap_or_else(|| session_id.to_owned());
    match v.get("kind").and_then(|k| k.as_str()) {
        Some("ask") => {
            let question =
                v.get("question").and_then(|q| q.as_str()).unwrap_or_default().to_owned();
            // Pass the structured `questions` array through (CCT-181) so the
            // webui renders interactive option cards live. `null`/absent →
            // `None`, leaving clients to fall back to the text form.
            let questions = v.get("questions").filter(|q| !q.is_null()).cloned();
            // The assistant prose preceding the question in the same turn, read
            // from the transcript by the `ask-hook` subcommand so the live card
            // carries its context (CCT-213). Absent/empty → `None`.
            let preamble = v
                .get("preamble")
                .and_then(|p| p.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned);
            Some(AdapterEvent::AskQuestion { local_id, question, questions, preamble })
        }
        Some("resolved") => Some(AdapterEvent::AskResolved { local_id }),
        other => {
            tracing::warn!(?other, "ignoring ask-hook line with unknown kind");
            None
        }
    }
}

pub(crate) fn resolve_legacy_socket_path(config: &serde_json::Value) -> PathBuf {
    if let Some(p) = config.get("socket_path").and_then(|v| v.as_str()) {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("CCTUI_DAEMON_SOCK") {
        return PathBuf::from(p);
    }
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(base).join("cctui-daemon.sock")
}

pub struct ClaudeCodeFactory;

impl AdapterFactory for ClaudeCodeFactory {
    fn id(&self) -> &'static str {
        "claude-code"
    }
    fn build(&self, _config: serde_json::Value) -> Box<dyn Adapter> {
        Box::new(ClaudeCodeAdapter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_via_config_mode() {
        let v = serde_json::json!({"mode": "claude-daemon"});
        assert!(use_claude_daemon_path(&v));
    }

    #[test]
    fn flag_via_config_mode_other_value() {
        assert!(!use_claude_daemon_path(&serde_json::json!({"mode": "legacy"})));
    }

    #[test]
    fn ask_hook_line_carries_structured_questions() {
        // CCT-181: the hook forwards the raw `questions` array so the webui can
        // render the interactive form live, not just the flattened text.
        let map: SessionMap = Arc::default();
        let line = r#"{"kind":"ask","session_id":"s1","question":"Color: pick","questions":[{"question":"Color?","options":[{"label":"Red"}]}]}"#;
        match hook_line_to_event(line, &map) {
            Some(AdapterEvent::AskQuestion { local_id, question, questions, .. }) => {
                assert_eq!(local_id, "s1");
                assert_eq!(question, "Color: pick");
                let qs = questions.expect("structured questions present");
                assert_eq!(qs[0]["question"], "Color?");
                assert_eq!(qs[0]["options"][0]["label"], "Red");
            }
            other => panic!("expected AskQuestion with questions, got {other:?}"),
        }
    }

    #[test]
    fn ask_hook_line_carries_preamble() {
        // CCT-213: the hook forwards the assistant prose preceding the question
        // (read from the transcript) so the live card isn't answered blind.
        let map: SessionMap = Arc::default();
        let line = r#"{"kind":"ask","session_id":"s1","question":"Pick","preamble":"Here is the analysis."}"#;
        match hook_line_to_event(line, &map) {
            Some(AdapterEvent::AskQuestion { preamble, .. }) => {
                assert_eq!(preamble.as_deref(), Some("Here is the analysis."));
            }
            other => panic!("expected AskQuestion with preamble, got {other:?}"),
        }
        // Blank/absent preamble → None so clients render the question alone.
        let blank = r#"{"kind":"ask","session_id":"s1","question":"Pick","preamble":"   "}"#;
        match hook_line_to_event(blank, &map) {
            Some(AdapterEvent::AskQuestion { preamble, .. }) => assert!(preamble.is_none()),
            other => panic!("expected AskQuestion, got {other:?}"),
        }
    }

    #[test]
    fn ask_hook_line_without_questions_is_none() {
        // A legacy/text-only delivery (no `questions`) still yields an event,
        // with `questions: None` so clients fall back to the text form.
        let map: SessionMap = Arc::default();
        let line = r#"{"kind":"ask","session_id":"s1","question":"hi"}"#;
        match hook_line_to_event(line, &map) {
            Some(AdapterEvent::AskQuestion { questions, preamble, .. }) => {
                assert!(questions.is_none());
                assert!(preamble.is_none());
            }
            other => panic!("expected AskQuestion, got {other:?}"),
        }
    }
}
