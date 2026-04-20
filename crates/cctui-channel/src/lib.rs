//! MCP channel — the `cctui channel` subcommand.
//!
//! Invoked by Claude Code as an MCP stdio subprocess. Bridges Claude ↔ the
//! cctui-server REST API. Port of `channel/src/index.ts`.
//!
//! IMPORTANT: stdout is the MCP wire. All logs go to stderr.

#![allow(clippy::too_many_lines)]

pub mod archive;
pub mod bridge;
pub mod config;
pub mod manifest;
pub mod mcp;
pub mod skills;
pub mod transcript;
pub mod types;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bridge::Bridge;
use cctui_proto::api::RegisterRequest;
use mcp::{McpEvent, Pusher};
use tokio::sync::{RwLock, watch};
use types::{PermissionRequest, SessionPollResponse, SessionState, StreamerEvent};

/// Run the channel until stdin closes or a signal fires.
pub async fn run() -> anyhow::Result<()> {
    init_tracing();
    let cfg = config::load();
    let bridge = Bridge::new(cfg.server_url.clone(), cfg.agent_token.clone());

    // Start MCP stdio first — Claude blocks on the initialize handshake.
    let mut mcp = mcp::serve();
    tracing::info!("connected to Claude Code");

    let (cancel_tx, cancel_rx) = watch::channel(false);
    let session: Arc<RwLock<Option<SessionState>>> = Arc::new(RwLock::new(None));

    // Skills sync — independent of session match.
    {
        let bridge_s = bridge.clone();
        tokio::spawn(async move { skills::sync(&bridge_s).await });
    }

    // Session registration + transcript + pending-message polling.
    let reg = tokio::spawn(run_session(
        bridge.clone(),
        mcp.pusher.clone(),
        session.clone(),
        cancel_rx.clone(),
    ));

    // Main event loop: pump MCP events, honour signals, wait for reg to finish.
    let mut sigterm = signal_term();
    let mut sigint = signal_int();

    loop {
        tokio::select! {
            Some(ev) = mcp.events.recv() => {
                handle_mcp_event(ev, &bridge, &mcp.pusher, &session).await;
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM");
                break;
            }
            _ = sigint.recv() => {
                tracing::info!("SIGINT");
                break;
            }
            else => break,
        }
    }

    let _ = cancel_tx.send(true);
    reg.abort();
    Ok(())
}

async fn handle_mcp_event(
    ev: McpEvent,
    bridge: &Bridge,
    pusher: &Pusher,
    session: &Arc<RwLock<Option<SessionState>>>,
) {
    let snapshot = session.read().await.clone();
    match ev {
        McpEvent::Reply(text) => {
            let Some(sess) = snapshot else {
                tracing::error!("reply received before session match — dropping");
                return;
            };
            let event = StreamerEvent {
                session_id: sess.session_id.clone(),
                ty: "assistant_message".to_string(),
                content: Some(format!("[Reply to TUI] {text}")),
                tool: None,
                input: None,
                tool_use_id: None,
                ts: chrono::Utc::now().timestamp(),
                tokens_in: None,
                tokens_out: None,
                cost_usd: None,
            };
            bridge.post_event(&sess.session_id, &event).await;
        }
        McpEvent::PermissionRequest { request_id, tool_name, description, input_preview } => {
            let Some(sess) = snapshot else {
                tracing::error!(%request_id, "permission_request without session — allowing");
                pusher.send_permission_response(&request_id, "allow").await;
                return;
            };
            tracing::info!(%request_id, tool = %tool_name, "forwarding permission_request");
            let req = PermissionRequest {
                request_id: request_id.clone(),
                tool_name,
                description,
                input_preview,
            };
            if let Err(err) = bridge.submit_permission_request(&sess.session_id, &req).await {
                tracing::error!(%request_id, error = %err, "submit permission failed — allowing");
                pusher.send_permission_response(&request_id, "allow").await;
                return;
            }
            let behavior = bridge
                .poll_permission_decision(
                    &sess.session_id,
                    &request_id,
                    Duration::from_secs(30),
                    Duration::from_millis(500),
                )
                .await;
            let decision = match behavior.as_deref() {
                Some("allow" | "deny") => behavior.unwrap(),
                _ => {
                    tracing::error!(%request_id, "permission timed out — allowing");
                    "allow".to_string()
                }
            };
            pusher.send_permission_response(&request_id, &decision).await;
        }
    }
}

async fn run_session(
    bridge: Bridge,
    pusher: Pusher,
    session: Arc<RwLock<Option<SessionState>>>,
    mut cancel: watch::Receiver<bool>,
) {
    let machine_id = cctui_proto::util::hostname();
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let ppid = parent_pid();

    // Register channel — retry up to 60 × 2s.
    let mut channel_id: Option<String> = None;
    for attempt in 1..=60 {
        if *cancel.borrow() {
            return;
        }
        match bridge.register_channel(&machine_id, ppid, &cwd).await {
            Ok(res) => {
                tracing::info!(channel_id = %res.channel_id, %machine_id, ppid, "channel registered");
                channel_id = Some(res.channel_id);
                break;
            }
            Err(err) => {
                tracing::error!(attempt, error = %err, "channel registration failed");
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_millis(2000)) => {},
                    _ = cancel.changed() => return,
                }
            }
        }
    }
    let Some(channel_id) = channel_id else {
        tracing::error!("channel register exhausted retries");
        return;
    };

    // Poll for session assignment.
    tracing::info!("waiting for SessionStart hook");
    let mut attempt: u32 = 0;
    let matched = loop {
        attempt = attempt.saturating_add(1);
        if *cancel.borrow() {
            return;
        }
        if let Ok(SessionPollResponse::Matched { session_id, transcript_path, model }) =
            bridge.poll_session(&channel_id).await
        {
            tracing::info!(%session_id, "session matched");
            break SessionState {
                session_id,
                transcript_path,
                cwd: cwd.clone(),
                machine_id: machine_id.clone(),
                model: model.unwrap_or_default(),
            };
        }
        if attempt % 150 == 0 {
            tracing::info!(elapsed_s = attempt * 2, "still waiting for session match");
        }
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(2000)) => {},
            _ = cancel.changed() => return,
        }
    };

    // Upsert the session server-side.
    let req = RegisterRequest {
        machine_id: matched.machine_id.clone(),
        working_dir: matched.cwd.clone(),
        claude_session_id: Some(matched.session_id.clone()),
        parent_session_id: None,
        metadata: Some(serde_json::json!({
            "project_name": project_name(&matched.cwd),
            "model": matched.model,
            "transcript_path": matched.transcript_path.clone().unwrap_or_default(),
        })),
    };
    if let Err(err) = bridge.register_session(&req).await {
        tracing::error!(error = %err, "register_session failed");
    } else {
        tracing::info!(session_id = %matched.session_id, "session registered with server");
    }

    // Publish the session to the event handler.
    *session.write().await = Some(matched.clone());

    // Pending-message poller.
    spawn_pending_poller(
        bridge.clone(),
        pusher.clone(),
        matched.session_id.clone(),
        cancel.clone(),
    );

    // Transcript tailer.
    if let Some(path) = matched.transcript_path.clone() {
        spawn_transcript_tailer(
            bridge.clone(),
            matched.session_id.clone(),
            PathBuf::from(path),
            cancel.clone(),
        );
    }

    // Archive pipeline.
    spawn_archive_pipeline(bridge.clone(), matched.clone(), cancel.clone());

    // Hold until cancelled.
    let _ = cancel.changed().await;
}

fn spawn_pending_poller(
    bridge: Bridge,
    pusher: Pusher,
    session_id: String,
    mut cancel: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        loop {
            if *cancel.borrow() {
                return;
            }
            let msgs = bridge.fetch_pending_messages(&session_id).await;
            if !msgs.is_empty() {
                tracing::info!(n = msgs.len(), "polled pending message(s)");
            }
            for msg in msgs {
                let mut meta = HashMap::new();
                meta.insert("message_id".to_string(), msg.id.clone());
                pusher.push_message(&msg.content, meta).await;
            }
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(1000)) => {},
                _ = cancel.changed() => return,
            }
        }
    });
}

fn spawn_transcript_tailer(
    bridge: Bridge,
    session_id: String,
    path: PathBuf,
    cancel: watch::Receiver<bool>,
) {
    let offset_path = transcript_offset_path(&session_id);
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(256);
        tokio::spawn(transcript::tail(path, offset_path, tx, cancel));
        while let Some(line) = rx.recv().await {
            bridge.post_transcript_line(&session_id, &line).await;
        }
    });
}

/// `~/.cctui/offsets/{session_id}.offset` — persistent byte offset so a
/// channel restart doesn't replay the whole transcript. Returns `None` only
/// when `$HOME` is unset, in which case we fall back to in-memory tracking
/// (replay-on-restart is still better than no tailing at all).
fn transcript_offset_path(session_id: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    // Strip path separators from the session id for safety — Claude session
    // ids are UUIDs so this is defensive.
    let sanitized: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if sanitized.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".cctui").join("offsets").join(format!("{sanitized}.offset")))
}

fn spawn_archive_pipeline(
    bridge: Bridge,
    session: SessionState,
    mut cancel: watch::Receiver<bool>,
) {
    let projects_root = std::env::var("CLAUDE_PROJECTS_DIR").ok().map_or_else(
        || {
            let home =
                std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            home.join(".claude").join("projects")
        },
        PathBuf::from,
    );
    let current_transcript_abs = session.transcript_path.clone().map(PathBuf::from);
    let cache = Arc::new(archive::ArchiveCache::new());

    // Startup: manifest first (so the server knows what to expect), then walk.
    {
        let bridge_a = bridge.clone();
        let cache_a = cache.clone();
        let projects_root_a = projects_root.clone();
        let current = current_transcript_abs.clone();
        tokio::spawn(async move {
            let entries = manifest::build(&projects_root_a);
            tracing::info!(count = entries.len(), "posting archive manifest");
            if let Err(err) = bridge_a.post_manifest(&entries).await {
                tracing::warn!(%err, "archive manifest post failed");
            }
            let files = archive::walk_project_dirs(&projects_root_a);
            for f in files {
                if Some(&f.abs_path) == current.as_ref() {
                    continue;
                }
                let _ = archive::upload_if_changed(&bridge_a, &cache_a, &f).await;
            }
        });
    }

    // Periodic flush for the live session.
    let archive_interval_min: u64 = std::env::var("CCTUI_ARCHIVE_INTERVAL_MINUTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15)
        .max(1);
    let interval = Duration::from_secs(archive_interval_min * 60);
    if let Some(current) = current_transcript_abs {
        let project_dir = current
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let current_file = archive::ProjectFile {
            abs_path: current,
            project_dir,
            session_id: session.session_id.clone(),
        };
        let projects_root_t = projects_root.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = tokio::time::sleep(interval) => {},
                    _ = cancel.changed() => return,
                }
                if *cancel.borrow() {
                    return;
                }
                let entries = manifest::build(&projects_root_t);
                if let Err(err) = bridge.post_manifest(&entries).await {
                    tracing::warn!(%err, "periodic archive manifest post failed");
                }
                let _ = archive::upload_if_changed(&bridge, &cache, &current_file).await;
            }
        });
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cctui_channel=info".into()),
        )
        .try_init();
}

fn parent_pid() -> u32 {
    #[cfg(unix)]
    unsafe extern "C" {
        fn getppid() -> i32;
    }
    #[cfg(unix)]
    {
        // SAFETY: getppid is defined by POSIX and always safe.
        unsafe { getppid() as u32 }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn signal_term() -> tokio::signal::unix::Signal {
    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler")
}
fn signal_int() -> tokio::signal::unix::Signal {
    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("install SIGINT handler")
}

fn project_name(cwd: &str) -> String {
    std::path::Path::new(cwd)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}
