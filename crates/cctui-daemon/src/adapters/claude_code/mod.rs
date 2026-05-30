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

mod backfill;
mod control;
mod discovery;
mod socket;
mod state;
mod transcript;

use std::path::PathBuf;

use cctui_proto::adapter::AdapterEvent;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;

use crate::adapter_runtime::{Adapter, AdapterCtx, AdapterFactory};

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
            let driver = control::Driver::new(cfg, ctx.events, ctx.commands, ctx.shutdown);
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

fn resolve_legacy_socket_path(config: &serde_json::Value) -> PathBuf {
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
}
