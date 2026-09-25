use cctui_proto::diagnose::SessionDiagnose;

use super::*;

pub(super) fn env_of(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

pub(super) fn deferred(sock: PathBuf) -> DeferredDispatch {
    DeferredDispatch {
        sock,
        req: serde_json::json!({ "proto": 1, "op": "dispatch" }),
        short: format!("t-{}", uuid::Uuid::new_v4()),
        what: "spawn in /tmp".to_owned(),
        session_id: uuid::Uuid::new_v4().to_string(),
        gate: None,
    }
}

pub(super) fn launch_argv_spec() -> cctui_proto::adapter::SessionSpec {
    cctui_proto::adapter::SessionSpec {
        service_tier: None,
        adapter_id: cctui_proto::adapter::AdapterId::new("claude-code"),
        working_dir: Some("/w".into()),
        prompt: Some("go".into()),
        name: Some("task".into()),
        permission_mode: Some(cctui_proto::adapter::PermissionMode::Auto),
        effort: Some(" high ".into()),
        model: Some("opus".into()),
        env: std::collections::BTreeMap::new(),
        bootstrap: serde_json::Value::Null,
        parent_local_id: None,
    }
}

pub(super) fn dispatched_argv(
    mut launch: LaunchArgs,
    relay: bool,
    prompt: Option<&str>,
) -> (Vec<String>, Vec<String>) {
    if relay {
        launch.mcp_config = Some("/cfg/mcp.json".into());
    }
    launch.settings_path = Some("/cfg/settings.json".into());
    let ids = JobIds {
        session_id: "child".into(),
        short: "c0ffee00".into(),
        nonce: "0123abcd".into(),
        created_at_ms: 1,
    };
    let req = launch::dispatch_request(
        &ids,
        "/w",
        &launch,
        prompt,
        &std::collections::BTreeMap::new(),
        &json!({}),
    );
    let strings = |v: &serde_json::Value| -> Vec<String> {
        v.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_owned()).collect()
    };
    (strings(&req["d"]["launch"]["args"]), strings(&req["d"]["respawnFlags"]))
}

/// The managed settings shape produced by `ensure_hook_settings` for a
/// non-whip session (hooks the ask form + permission flow depend on). Kept
/// in the test so the merge assertions below pin the load-bearing keys
/// without dragging in `current_exe`/socket resolution.
pub(super) fn managed_ask_settings() -> serde_json::Value {
    json!({
        "hooks": {
            "PreToolUse": [
                { "matcher": "AskUserQuestion|ExitPlanMode", "hooks": [{ "type": "command", "command": "cctui ask-hook --event pre" }] },
                { "matcher": "Bash|Edit", "hooks": [{ "type": "command", "command": "cctui ask-hook --event perm" }] },
            ],
            "PostToolUse": [{ "matcher": "AskUserQuestion|ExitPlanMode", "hooks": [{ "type": "command", "command": "cctui ask-hook --event post" }] }],
        },
    })
}

pub(super) fn snap(short: &str, state: &str, name: Option<&str>) -> LiveSnapshot {
    LiveSnapshot {
        short: short.into(),
        session_id: Some(format!("{short}-uuid")),
        session_id_camel: None,
        cwd: Some("/tmp".into()),
        tempo: Some("active".into()),
        state: Some(state.into()),
        detail: None,
        needs: None,
        name: name.map(String::from),
        intent: None,
        source: Some(FLEET_SOURCE.into()),
        dying: false,
        gone: false,
        dead: false,
        alive: None,
        status: None,
        cli_version: Some("2.1.145".into()),
    }
}

pub(super) fn driver() -> (Driver, mpsc::Receiver<AdapterEvent>) {
    let (tx, rx) = mpsc::channel(64);
    let (_cmd_tx, cmd_rx) = mpsc::channel(64);
    let tmp = tempfile::tempdir().unwrap().keep();
    let cfg = DriverConfig {
        poll_interval: Duration::from_millis(50),
        jobs_root: tmp.join("jobs"),
        projects_root: tmp.join("projects"),
        discovery: Discovery::with_base(tmp.join("daemon")),
        offsets_path: Some(tmp.join("offsets.json")),
        backfill_cursor_path: Some(tmp.join("backfill.json")),
        skip_backfill: true,
        claude_bin: "claude".to_string(),
        hook_socket_path: tmp.join("hook.sock"),
    };
    (Driver::new(cfg, tx, cmd_rx, CancellationToken::new()), rx)
}

/// Write a subagent transcript under the parent's `subagents/` dir so a
/// poll discovers it. Returns the agent file path.
pub(super) fn write_subagent(d: &Driver, parent_short: &str, agent_id: &str, lines: &[&str]) {
    use std::io::Write;
    let sess = format!("{parent_short}-uuid");
    let parent_path = transcript::transcript_path(&d.cfg.projects_root, "/tmp", &sess);
    let dir = transcript::subagents_dir(&parent_path);
    std::fs::create_dir_all(&dir).unwrap();
    let mut f = std::fs::File::create(dir.join(format!("agent-{agent_id}.jsonl"))).unwrap();
    for l in lines {
        f.write_all(l.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
    }
}

/// Drain events until the Diagnose reply for `request_id` arrives.
pub(super) async fn recv_diagnose(
    rx: &mut mpsc::Receiver<AdapterEvent>,
    request_id: uuid::Uuid,
) -> SessionDiagnose {
    loop {
        match rx.recv().await.expect("event stream open") {
            AdapterEvent::Diagnose { request_id: rid, report, .. } if rid == request_id => {
                return *report;
            }
            _ => {}
        }
    }
}

/// Write `lines` to a live session's main transcript at the path
/// `apply_snapshot` resolves for `snap(short, …)`, returning its `local_id`.
pub(super) fn write_main_transcript(d: &Driver, short: &str, lines: &[&str]) -> String {
    use std::io::Write;
    let sess = format!("{short}-uuid");
    let path = transcript::transcript_path(&d.cfg.projects_root, "/tmp", &sess);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).unwrap();
    for l in lines {
        f.write_all(l.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
    }
    sess
}

pub(super) fn text_line(t: &str) -> String {
    format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{t}"}}]}}}}"#)
}

pub(super) fn drain_messages(rx: &mut mpsc::Receiver<AdapterEvent>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(evt) = rx.try_recv() {
        if let AdapterEvent::Message { payload, .. } = evt
            && let Some(t) = payload.get("text").and_then(|v| v.as_str())
        {
            out.push(t.to_owned());
        }
    }
    out
}
