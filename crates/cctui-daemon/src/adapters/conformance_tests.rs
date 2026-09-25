use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use cctui_proto::adapter::{AdapterCommand, AdapterEvent, AdapterId, RemoveInitiator, SessionSpec};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::adapter_runtime::{Adapter, AdapterCtx};

const GHOST: &str = "ghost";
const SPAWN: Uuid = Uuid::from_u128(1);
const FORK: Uuid = Uuid::from_u128(2);
const REPLY: Uuid = Uuid::from_u128(3);
const INTERRUPT: Uuid = Uuid::from_u128(4);
const REMOVE: Uuid = Uuid::from_u128(5);
const SET_MODEL: Uuid = Uuid::from_u128(6);
const DIAGNOSE: Uuid = Uuid::from_u128(7);

const FORK_TO_CHANGE_MODEL: &str =
    "in-place model/effort switch is not supported for claude sessions — fork to change model";
const CODEX_LOST: &str = "codex session lost: no live app-server and no resumable thread record";

fn spec_without_dir(adapter: &str) -> SessionSpec {
    SessionSpec {
        adapter_id: AdapterId::new(adapter),
        working_dir: None,
        prompt: None,
        name: None,
        permission_mode: None,
        effort: None,
        model: None,
        service_tier: None,
        env: BTreeMap::new(),
        bootstrap: serde_json::Value::Null,
        parent_local_id: None,
    }
}

fn every_command(adapter: &str) -> Vec<AdapterCommand> {
    vec![
        AdapterCommand::ResumeMarks { marks: vec![] },
        AdapterCommand::SendMessage { local_id: GHOST.into(), text: "hi".into() },
        AdapterCommand::Kill { local_id: GHOST.into(), signal: None },
        AdapterCommand::Spawn {
            spec: spec_without_dir(adapter),
            command_id: Some(SPAWN),
            session_id: None,
        },
        AdapterCommand::Fork {
            parent_local_id: GHOST.into(),
            spec: spec_without_dir(adapter),
            command_id: Some(FORK),
            session_id: None,
            extract: None,
        },
        AdapterCommand::Reply {
            local_id: GHOST.into(),
            text: "hi".into(),
            ask_picks: None,
            env: BTreeMap::new(),
            command_id: Some(REPLY),
            turn_id: None,
        },
        AdapterCommand::Interrupt { local_id: GHOST.into(), command_id: Some(INTERRUPT) },
        AdapterCommand::Resume { local_id: GHOST.into(), working_dir: None, env: BTreeMap::new() },
        AdapterCommand::PermissionResponse {
            local_id: GHOST.into(),
            request_id: "r1".into(),
            allow: true,
        },
        AdapterCommand::Rename { local_id: GHOST.into(), name: "renamed".into() },
        AdapterCommand::Remove {
            local_id: GHOST.into(),
            command_id: Some(REMOVE),
            initiator: RemoveInitiator::User,
        },
        AdapterCommand::SetModel {
            local_id: GHOST.into(),
            model: Some("m".into()),
            effort: None,
            command_id: Some(SET_MODEL),
        },
        AdapterCommand::Diagnose { local_id: GHOST.into(), request_id: DIAGNOSE },
        AdapterCommand::WatchPty { local_id: GHOST.into(), watch: true },
    ]
}

fn tag(id: Uuid) -> &'static str {
    match id {
        SPAWN => "spawn",
        FORK => "fork",
        REPLY => "reply",
        INTERRUPT => "interrupt",
        REMOVE => "remove",
        SET_MODEL => "set_model",
        DIAGNOSE => "diagnose",
        _ => "?",
    }
}

/// `opaque` results drop their error text: it comes from the OS.
fn line(evt: &AdapterEvent, opaque: &[Uuid]) -> Option<String> {
    match evt {
        AdapterEvent::CommandResult { command_id, ok, error } => {
            let error = if opaque.contains(command_id) {
                error.as_ref().map(|_| "<os>".to_owned())
            } else {
                error.clone()
            };
            Some(format!("result {} ok={ok} error={error:?}", tag(*command_id)))
        }
        AdapterEvent::Diagnose { local_id, request_id, .. } if local_id == GHOST => {
            Some(format!("diagnose {}", tag(*request_id)))
        }
        AdapterEvent::SessionEnded { local_id, reason } if local_id == GHOST => {
            Some(format!("ended {reason:?}"))
        }
        AdapterEvent::Status { local_id, state, detail, .. } if local_id == GHOST => {
            Some(format!("status {state:?} {detail:?}"))
        }
        AdapterEvent::SessionStarted { local_id, .. } if local_id == GHOST => {
            Some("started".to_owned())
        }
        _ => None,
    }
}

async fn collect(
    rx: &mut mpsc::Receiver<AdapterEvent>,
    expected: usize,
    opaque: &[Uuid],
) -> Vec<String> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while out.len() < expected {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(evt)) => out.extend(line(&evt, opaque)),
            _ => break,
        }
    }
    while let Ok(Some(evt)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
        out.extend(line(&evt, opaque));
    }
    out.sort();
    out
}

fn sorted(lines: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = lines.iter().map(|s| (*s).to_owned()).collect();
    v.sort();
    v
}

struct Harness {
    events: mpsc::Receiver<AdapterEvent>,
    commands: mpsc::Sender<AdapterCommand>,
    shutdown: CancellationToken,
    _connected: broadcast::Sender<()>,
}

fn ctx(config: serde_json::Value) -> (AdapterCtx, Harness) {
    let (events_tx, events) = mpsc::channel(256);
    let (commands, commands_rx) = mpsc::channel(64);
    let (connected_tx, connected) = broadcast::channel(4);
    let shutdown = CancellationToken::new();
    let ctx = AdapterCtx {
        events: events_tx,
        commands: commands_rx,
        shutdown: shutdown.clone(),
        config,
        server: None,
        machine_key: None,
        connected,
    };
    (ctx, Harness { events, commands, shutdown, _connected: connected_tx })
}

async fn drive(
    adapter: &str,
    harness: &mut Harness,
    expected: usize,
    opaque: &[Uuid],
) -> Vec<String> {
    for cmd in every_command(adapter) {
        harness.commands.send(cmd).await.expect("adapter accepts commands");
    }
    let lines = collect(&mut harness.events, expected, opaque).await;
    harness.shutdown.cancel();
    lines
}

fn fake_control_socket(base: &Path) {
    let dir = base.join("0000aaaa");
    std::fs::create_dir_all(&dir).unwrap();
    let listener = tokio::net::UnixListener::bind(dir.join("control.sock")).unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                while let Ok(Some(_)) = lines.next_line().await {
                    if write.write_all(b"{\"ok\":true,\"jobs\":[]}\n").await.is_err() {
                        return;
                    }
                }
            });
        }
    });
}

fn claude_config(mode: &str, tmp: &Path) -> serde_json::Value {
    serde_json::json!({
        "mode": mode,
        "jobs_root": tmp.join("jobs"),
        "projects_root": tmp.join("projects"),
        "discovery_base": tmp.join("daemon"),
        "offsets_path": tmp.join("offsets.json"),
        "backfill_cursor_path": tmp.join("backfill.json"),
        "skip_backfill": true,
        "claude_bin": tmp.join("no-such-claude"),
        "socket_path": tmp.join("hook.sock"),
    })
}

async fn run_claude(mode: &str, expected: usize) -> Vec<String> {
    let tmp = tempfile::tempdir().unwrap();
    fake_control_socket(&tmp.path().join("daemon"));
    let (ctx, mut harness) = ctx(claude_config(mode, tmp.path()));
    let task = tokio::spawn(async move { super::claude_code::ClaudeCodeAdapter.start(ctx).await });
    let lines = drive("claude-code", &mut harness, expected, &[]).await;
    task.abort();
    lines
}

#[tokio::test]
async fn claude_bg_answers_every_command() {
    let expected = sorted(&[
        "diagnose diagnose",
        "result fork ok=false error=Some(\"fork: working_dir required\")",
        "result interrupt ok=false error=Some(\"unknown session ghost\")",
        "result remove ok=false error=Some(\"cannot resolve short for ghost\")",
        "result reply ok=false error=Some(\"cannot resolve short for ghost\")",
        &format!("result set_model ok=false error=Some({FORK_TO_CHANGE_MODEL:?})"),
        "result spawn ok=false error=Some(\"spawn: working_dir required\")",
    ]);
    assert_eq!(run_claude("bg", expected.len()).await, expected);
}

#[tokio::test]
async fn claude_oneshot_answers_every_command() {
    let expected = sorted(&[
        "ended Killed",
        "result fork ok=false error=Some(\"spawn: working_dir required\")",
        "result interrupt ok=true error=None",
        "result remove ok=true error=None",
        "result reply ok=false error=Some(\"reply: unknown session ghost\")",
        &format!("result set_model ok=false error=Some({FORK_TO_CHANGE_MODEL:?})"),
        "result spawn ok=false error=Some(\"spawn: working_dir required\")",
    ]);
    assert_eq!(run_claude("oneshot", expected.len()).await, expected);
}

#[tokio::test]
async fn claude_sdk_answers_every_command() {
    let expected = sorted(&[
        "ended Killed",
        "result fork ok=false error=Some(\"spawn: working_dir required\")",
        "result interrupt ok=true error=None",
        "result remove ok=true error=None",
        "result reply ok=false error=Some(\"cannot (re)launch ghost: no known working_dir/posture\")",
        "result set_model ok=true error=None",
        "result spawn ok=false error=Some(\"spawn: working_dir required\")",
    ]);
    assert_eq!(run_claude("sdk", expected.len()).await, expected);
}

#[tokio::test]
async fn opencode_answers_every_command() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, mut harness) = ctx(serde_json::json!({
        "bin": tmp.path().join("no-such-opencode"),
        "state_root": tmp.path(),
    }));
    let task = tokio::spawn(async move { super::opencode::OpenCodeAdapter.start(ctx).await });
    let expected = sorted(&[
        "diagnose diagnose",
        "ended Killed",
        "ended Killed",
        "result fork ok=false error=Some(\"opencode fork requires the parent session to be live on this daemon\")",
        "result interrupt ok=false error=Some(\"no live opencode session\")",
        "result reply ok=false error=Some(\"no live opencode session\")",
        "result spawn ok=false error=Some(\"working_dir required\")",
    ]);
    let lines = drive("opencode", &mut harness, expected.len(), &[]).await;
    task.abort();
    assert_eq!(lines, expected);
}

#[tokio::test]
async fn codex_answers_every_command() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, mut harness) = ctx(serde_json::Value::Null);
    let bin = tmp.path().join("no-such-codex").to_string_lossy().into_owned();
    let AdapterCtx { events, commands, shutdown, .. } = ctx;
    let task = tokio::spawn(async move {
        super::codex::run_command_pump_for_test(&bin, events, commands, shutdown).await;
    });
    let lost = format!("status Some(\"failed\") Some({CODEX_LOST:?})");
    let expected = sorted(&[
        "diagnose diagnose",
        "ended Killed",
        "ended Killed",
        "result fork ok=false error=Some(\"<os>\")",
        "result interrupt ok=false error=Some(\"no live codex session to interrupt\")",
        "result reply ok=false error=Some(\"no codex session for command\")",
        "result set_model ok=false error=Some(\"no codex session for command\")",
        "result spawn ok=false error=Some(\"working_dir required\")",
        &lost,
        &lost,
        &lost,
        &lost,
        &lost,
    ]);
    let lines = drive("codex", &mut harness, expected.len(), &[FORK]).await;
    task.abort();
    assert_eq!(lines, expected);
}
