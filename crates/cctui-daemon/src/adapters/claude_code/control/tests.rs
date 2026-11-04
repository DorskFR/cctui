use super::test_support::*;
use super::*;

#[tokio::test]
async fn fork_dispatch_preserves_the_child_gateway_hint_when_the_pull_is_unavailable() {
    let (d, _events) = driver();
    let mut spec = launch_argv_spec();
    spec.working_dir = Some(d.cfg.jobs_root.parent().unwrap().to_string_lossy().into_owned());
    spec.env = env_of(&[
        ("ANTHROPIC_BASE_URL", "https://gateway.invalid/anthropic"),
        ("ANTHROPIC_AUTH_TOKEN", "child-token"),
    ]);
    let child = uuid::Uuid::new_v4().to_string();
    let dispatch = d
        .prepare_fork(Path::new("/unused.sock"), "parent", &spec, Some(&child), None)
        .await
        .unwrap();
    assert_eq!(dispatch.req["d"]["env"]["ANTHROPIC_AUTH_TOKEN"], "child-token");
    assert_eq!(dispatch.req["d"]["reattachEnv"]["ANTHROPIC_AUTH_TOKEN"], "child-token");
    assert_eq!(dispatch.session_id, child);
    std::fs::remove_dir_all(d.cfg.jobs_root.parent().unwrap()).unwrap();
}

fn refused_for_hook_settings<T>(launched: anyhow::Result<T>) {
    let Err(err) = launched else { panic!("the launch was not refused") };
    assert!(err.downcast_ref::<HookSettingsError>().is_some(), "{err:#}");
    assert!(format!("{err:#}").contains("refusing to launch without cctui's hooks"), "{err:#}");
}

fn stop_policy() -> cctui_proto::hook_policy::HookPolicy {
    use cctui_proto::hook_policy::{HookEvent, HookPolicy, HookRule, ToolClass};
    HookPolicy {
        rules: vec![HookRule {
            event: HookEvent::Stop,
            tool: ToolClass::Any,
            command: "p-stop".into(),
            timeout_secs: None,
        }],
    }
}

fn workdir_spec(d: &Driver) -> cctui_proto::adapter::SessionSpec {
    let mut spec = launch_argv_spec();
    spec.working_dir = Some(d.cfg.jobs_root.parent().unwrap().to_string_lossy().into_owned());
    spec
}

#[tokio::test]
async fn a_spawn_with_or_without_a_policy_is_refused_when_its_settings_cannot_be_written() {
    let (d, mut events) = driver();
    let tmp = tempfile::tempdir().unwrap();
    let _root = ConfigRootOverride::unwritable(tmp.path());
    for policy in [Some(stop_policy()), None] {
        let mut spec = workdir_spec(&d);
        spec.hook_policy = policy;
        let id = uuid::Uuid::new_v4().to_string();
        refused_for_hook_settings(
            d.prepare_spawn(Path::new("/unused.sock"), &spec, Some(id)).await,
        );
    }
    assert!(events.try_recv().is_err(), "a refused spawn reports through its command result");
    std::fs::remove_dir_all(d.cfg.jobs_root.parent().unwrap()).unwrap();
}

#[tokio::test]
async fn a_fork_is_refused_when_its_settings_cannot_be_written() {
    let (d, _events) = driver();
    let tmp = tempfile::tempdir().unwrap();
    let _root = ConfigRootOverride::unwritable(tmp.path());
    let child = uuid::Uuid::new_v4().to_string();
    refused_for_hook_settings(
        d.prepare_fork(Path::new("/unused.sock"), "parent", &workdir_spec(&d), Some(&child), None)
            .await,
    );
    assert!(d.fork_parent_by_short.lock().unwrap().is_empty(), "the fork never linked its parent");
    std::fs::remove_dir_all(d.cfg.jobs_root.parent().unwrap()).unwrap();
}

#[tokio::test]
async fn a_dispatched_worker_spec_is_refused_when_its_settings_cannot_be_written() {
    use cctui_proto::hook_policy::DISPATCH_POLICY_FIELD;
    let (d, _events) = driver();
    let tmp = tempfile::tempdir().unwrap();
    let _root = ConfigRootOverride::unwritable(tmp.path());
    let payload = json!({ DISPATCH_POLICY_FIELD: stop_policy(), "prompt": "go" });
    let mut spec = Driver::build_dispatch_spec(&payload).unwrap();
    assert!(spec.hook_policy.is_some());
    spec.working_dir = workdir_spec(&d).working_dir;
    let id = uuid::Uuid::new_v4().to_string();
    refused_for_hook_settings(d.prepare_spawn(Path::new("/unused.sock"), &spec, Some(id)).await);
    std::fs::remove_dir_all(d.cfg.jobs_root.parent().unwrap()).unwrap();
}

#[tokio::test]
async fn a_dispatched_pod_reports_a_refused_launch_and_sends_nothing() {
    use cctui_proto::hook_policy::DISPATCH_POLICY_FIELD;
    let (d, _events) = driver();
    let tmp = tempfile::tempdir().unwrap();
    let sock = tmp.path().join("claude.sock");
    let ops = dead_worker_socket(&sock);
    let _root = ConfigRootOverride::unwritable(tmp.path());
    let payload = json!({ DISPATCH_POLICY_FIELD: stop_policy(), "prompt": "go" });
    let mut spec = Driver::build_dispatch_spec(&payload).unwrap();
    spec.working_dir = workdir_spec(&d).working_dir;
    let id = uuid::Uuid::new_v4().to_string();
    let mut reported = Vec::new();
    let dispatched = d
        .dispatch_started_session(&sock, &spec, &id, |err| reported.push(format!("{err:#}")))
        .await;
    refused_for_hook_settings(dispatched);
    assert_eq!(reported.len(), 1, "the refusal is reported exactly once");
    assert!(reported[0].starts_with("refusing to launch"), "{}", reported[0]);
    assert!(ops.lock().unwrap().is_empty(), "nothing reached the claude daemon");
    std::fs::remove_dir_all(d.cfg.jobs_root.parent().unwrap()).unwrap();
}

#[tokio::test]
async fn a_policy_spawn_without_a_machine_key_is_refused_before_any_settings_exist() {
    let (d, _events) = driver();
    assert!(d.machine_key.is_none());
    let tmp = tempfile::tempdir().unwrap();
    let _root = ConfigRootOverride::set(tmp.path());
    let mut spec = workdir_spec(&d);
    spec.hook_policy = Some(stop_policy());
    let id = uuid::Uuid::new_v4().to_string();
    let Err(err) = d.prepare_spawn(Path::new("/unused.sock"), &spec, Some(id)).await else {
        panic!("a policy launch without a seal key was not refused");
    };
    assert!(
        matches!(
            err.downcast_ref::<HookSettingsError>(),
            Some(HookSettingsError::PolicyNotKept { .. })
        ),
        "{err:#}"
    );
    let settings = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("hook-settings-"))
        .count();
    assert_eq!(settings, 0, "no --settings file was written for the refused launch");
    std::fs::remove_dir_all(d.cfg.jobs_root.parent().unwrap()).unwrap();
}

/// Answers every control-socket request with a dead worker and records its op.
fn dead_worker_socket(sock: &Path) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::UnixListener::bind(sock).unwrap();
    let ops = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = ops.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (read_half, mut write_half) = stream.into_split();
            let mut line = String::new();
            let mut reader = tokio::io::BufReader::new(read_half);
            if reader.read_line(&mut line).await.is_err() {
                continue;
            }
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
            seen.lock().unwrap().push(req["op"].as_str().unwrap_or("?").to_owned());
            let _ = write_half.write_all(b"{\"ok\":true,\"alive\":false}\n").await;
        }
    });
    ops
}

#[tokio::test]
async fn a_cold_resume_is_refused_and_dispatches_nothing_when_its_settings_cannot_be_written() {
    let (d, _events) = driver();
    let tmp = tempfile::tempdir().unwrap();
    let short = "c01dfee0";
    let root = tmp.path().join("cfg");
    std::fs::create_dir_all(root.join(format!("hook-settings-{short}.json"))).unwrap();
    let _root = ConfigRootOverride::set(&root);
    let sock = tmp.path().join("claude.sock");
    let ops = dead_worker_socket(&sock);
    let cwd = workdir_spec(&d).working_dir.unwrap();
    let resumed = d
        .resume_worker(
            &sock,
            short,
            "local",
            Some("c01dfee0-0000-4000-8000-000000000000"),
            Some(&cwd),
            &std::collections::BTreeMap::new(),
        )
        .await;
    refused_for_hook_settings(resumed);
    assert_eq!(*ops.lock().unwrap(), ["has"], "no dispatch reached the claude daemon");
    std::fs::remove_dir_all(d.cfg.jobs_root.parent().unwrap()).unwrap();
}

#[test]
fn supervise_daemon_defaults_on_and_is_disablable_by_config() {
    assert!(DriverConfig::default().supervise_daemon);
    assert!(DriverConfig::from_value(&serde_json::json!({})).supervise_daemon);
    assert!(
        !DriverConfig::from_value(&serde_json::json!({"supervise_daemon": false})).supervise_daemon
    );
    assert!(!driver().0.cfg.supervise_daemon, "the test driver must never supervise");
}

#[test]
fn a_machine_local_env_override_wins_over_the_server_side_config() {
    assert_eq!(supervise_env_override(None), None);
    assert_eq!(supervise_env_override(Some("")), None);
    assert_eq!(supervise_env_override(Some("  ")), None);
    assert_eq!(supervise_env_override(Some("maybe")), None);
    for off in ["0", "false", "no", "OFF", " false "] {
        assert_eq!(supervise_env_override(Some(off)), Some(false), "{off}");
    }
    for on in ["1", "true", "YES", "on"] {
        assert_eq!(supervise_env_override(Some(on)), Some(true), "{on}");
    }
}

#[test]
fn reseed_interval_defaults_and_honors_override() {
    assert_eq!(reseed_interval_from(None), Duration::from_hours(1));
    assert_eq!(reseed_interval_from(Some("120".into())), Duration::from_mins(2));
    // Zero / garbage fall back to the hourly default rather than a hot loop.
    assert_eq!(reseed_interval_from(Some("0".into())), Duration::from_hours(1));
    assert_eq!(reseed_interval_from(Some("nope".into())), Duration::from_hours(1));
}

#[test]
fn reseed_runs_on_first_pass_reattach_and_after_interval() {
    let interval = Duration::from_hours(1);
    // First pass (never re-seeded) always runs.
    assert!(reseed_due(None, interval, false));
    // A fresh pass is not due again until the interval elapses...
    assert!(!reseed_due(Some(Instant::now()), interval, false));
    // ...unless the daemon just (re)attached to the claude socket.
    assert!(reseed_due(Some(Instant::now()), interval, true));
    // Past the interval, the periodic renewal fires.
    let stale = Instant::now().checked_sub(Duration::from_secs(3601)).unwrap();
    assert!(reseed_due(Some(stale), interval, false));
}

#[test]
fn kill_signal_name_maps_to_claude_enum() {
    // The interrupt route sends 15; kill_session sends None (handled at the
    // call site). Anything that is not SIGKILL must map to SIGTERM so it
    // satisfies claude's `["SIGTERM","SIGKILL"]` enum; a numeric signal is
    // rejected outright.
    assert_eq!(kill_signal_name(15), "SIGTERM");
    assert_eq!(kill_signal_name(9), "SIGKILL");
    assert_eq!(kill_signal_name(2), "SIGTERM");
}

#[test]
fn only_spares_and_dying_jobs_are_hidden() {
    let fleet = snap("aaaaaaaa", "working", None);
    assert!(fleet.is_user_visible());
    assert!(!fleet.is_foreign());

    // A human's own job is visible — it is just never touched automatically.
    for foreign in ["shell", "cli", "bg", "interactive"] {
        let mut s = snap("bbbbbbbb", "working", None);
        s.source = Some(foreign.into());
        assert!(s.is_user_visible(), "{foreign} must stay visible");
        assert!(s.is_foreign());
    }

    let mut spare = snap("eeeeeeee", "working", None);
    spare.source = Some(SPARE_SOURCE.into());
    assert!(!spare.is_user_visible());
    assert!(spare.is_foreign());

    // Older claude builds omit `source`; those are ours.
    let mut no_source = snap("cccccccc", "working", None);
    no_source.source = None;
    assert!(no_source.is_user_visible());
    assert!(!no_source.is_foreign());

    let mut dying = snap("dddddddd", "working", None);
    dying.dying = true;
    assert!(!dying.is_user_visible());
}

#[test]
fn resume_guard_bounds_supervisor_revives_but_yields_to_the_caller() {
    let guarded = with_resume_guard(std::collections::BTreeMap::new());
    assert_eq!(guarded.get("CLAUDE_CODE_RESUME_INTERRUPTED_TURN").unwrap(), "0");
    assert_eq!(
        guarded.get("CLAUDE_CODE_RESUME_INTERRUPTED_TURN_MAX_AGE_MS").unwrap(),
        RESUME_INTERRUPTED_TURN_MAX_AGE_MS
    );
    // A bound of 0 would mean "no bound" — never emit that as the default.
    assert_ne!(RESUME_INTERRUPTED_TURN_MAX_AGE_MS, "0");

    let explicit = with_resume_guard(env_of(&[
        ("CLAUDE_CODE_RESUME_INTERRUPTED_TURN", "1"),
        ("ANTHROPIC_BASE_URL", "http://gw"),
    ]));
    assert_eq!(explicit.get("CLAUDE_CODE_RESUME_INTERRUPTED_TURN").unwrap(), "1");
    assert_eq!(explicit.get("ANTHROPIC_BASE_URL").unwrap(), "http://gw");
    assert!(explicit.contains_key("CLAUDE_CODE_RESUME_INTERRUPTED_TURN_MAX_AGE_MS"));
}

#[tokio::test]
async fn injected_turn_stamps_every_encoding_with_one_id() {
    let (d, mut rx) = driver();
    let id = uuid::Uuid::new_v4();
    d.note_turn("sess-1", Some(id));
    // The three shapes Claude stores one attachment-carrying turn in.
    for text in [
        "look at this\nAttached file:\n- /tmp/cctui-uploads/sess-1/shot.png",
        "[Image #1][shot.png]look at this",
        "[Image: source: /tmp/cctui-uploads/sess-1/shot.png]",
    ] {
        d.emit_fresh(AdapterEvent::Message {
            local_id: "sess-1".into(),
            payload: json!({"role": "user", "text": text, "meta": false}),
            turn_id: None,
        })
        .await;
    }
    for _ in 0..3 {
        let AdapterEvent::Message { turn_id, .. } = rx.recv().await.unwrap() else {
            panic!("want Message")
        };
        assert_eq!(turn_id, Some(id));
    }

    d.emit_fresh(AdapterEvent::Message {
        local_id: "sess-1".into(),
        payload: json!({"role": "assistant", "text": "on it"}),
        turn_id: None,
    })
    .await;
    let AdapterEvent::Message { turn_id, .. } = rx.recv().await.unwrap() else {
        panic!("want Message")
    };
    assert_eq!(turn_id, None, "assistant text must not inherit the turn id");

    d.emit_fresh(AdapterEvent::Message {
        local_id: "sess-2".into(),
        payload: json!({"role": "user", "text": "hi"}),
        turn_id: None,
    })
    .await;
    let AdapterEvent::Message { turn_id, .. } = rx.recv().await.unwrap() else {
        panic!("want Message")
    };
    assert_eq!(turn_id, None, "another session's turn must not inherit it");
}

#[tokio::test]
async fn a_reply_without_a_turn_id_clears_the_previous_one() {
    let (d, mut rx) = driver();
    d.note_turn("sess-1", Some(uuid::Uuid::new_v4()));
    d.note_turn("sess-1", None);
    d.emit_fresh(AdapterEvent::Message {
        local_id: "sess-1".into(),
        payload: json!({"role": "user", "text": "typed in the TUI"}),
        turn_id: None,
    })
    .await;
    let AdapterEvent::Message { turn_id, .. } = rx.recv().await.unwrap() else {
        panic!("want Message")
    };
    assert_eq!(turn_id, None);
}

#[test]
fn a_turn_id_older_than_the_window_is_not_reused() {
    let (d, _rx) = driver();
    let id = uuid::Uuid::new_v4();
    let stale = Instant::now()
        .checked_sub(TURN_ID_WINDOW + Duration::from_secs(1))
        .expect("monotonic clock must already be older than the window");
    d.pending_turns.lock().unwrap().insert("sess-1".into(), PendingTurn { id, at: stale });
    assert_eq!(d.turn_for("sess-1"), None);
    assert!(!d.pending_turns.lock().unwrap().contains_key("sess-1"));
}

#[test]
fn is_dead_parses_defensive_shapes() {
    // no live known-dead sample, so several plausible shapes.
    let mut s = snap("abcd1234", "working", None);
    assert!(!s.is_dead(), "live working session is not dead");

    s.gone = true;
    assert!(s.is_dead(), "gone flag → dead");
    s.gone = false;

    s.dead = true;
    assert!(s.is_dead(), "dead flag → dead");
    s.dead = false;

    s.alive = Some(false);
    assert!(s.is_dead(), "alive:false → dead");
    s.alive = Some(true);
    assert!(!s.is_dead(), "alive:true → not dead");
    s.alive = None;

    s.status = Some("Exited".into());
    assert!(s.is_dead(), "status:exited (case-insensitive) → dead");
    s.status = Some("process gone".into());
    assert!(s.is_dead(), "status:'process gone' → dead");
    s.status = Some("running".into());
    assert!(!s.is_dead(), "status:running → not dead");
    s.status = None;

    s.state = Some("gone".into());
    assert!(s.is_dead(), "state:gone → dead");
    s.state = Some("working".into());
    assert!(!s.is_dead(), "state:working → not dead");
    s.state = None;

    s.tempo = Some("dead".into());
    assert!(s.is_dead(), "tempo:dead → dead");
    s.tempo = None;

    // the observed live shape — state:"failed", tempo:"idle",
    // detail:"process gone while supervisor was down". The phrase is
    // embedded in a sentence in `detail`, so it must match as a substring.
    s.state = Some("failed".into());
    s.tempo = Some("idle".into());
    s.detail = Some("process gone while supervisor was down".into());
    assert!(s.is_dead(), "detail containing 'process gone' → dead");
    s.detail = Some("Working on the fix".into());
    assert!(!s.is_dead(), "ordinary detail → not dead");
}

#[tokio::test]
async fn resolve_short_for_removal_uses_live_map_then_derives() {
    // removal targets completed sessions, which have already
    // dropped out of the live roster. Prefer the live reverse map, but
    // fall back to the session UUID's first group (== the short).
    let (mut d, _rx) = driver();
    let mut s = snap("deadbeef", "working", None);
    s.session_id = Some("deadbeef-1111-2222-3333-444455556666".into());
    d.apply_snapshot(vec![s]).await;
    // Live: resolved from the map.
    assert_eq!(
        d.resolve_short_for_removal("deadbeef-1111-2222-3333-444455556666").unwrap(),
        "deadbeef"
    );
    // Exited (not in the map): derived from the UUID's first group.
    assert_eq!(
        d.resolve_short_for_removal("c0ffee00-9999-8888-7777-666655554444").unwrap(),
        "c0ffee00"
    );
    // Non-hex / malformed first group: refuse rather than guess.
    assert!(d.resolve_short_for_removal("zzzzzzzz-0000").is_err());
}

#[tokio::test]
async fn slow_discovery_keeps_the_roster_and_does_not_kickstart() {
    let (mut d, mut events) = driver();
    let tmp = tempfile::tempdir().unwrap();
    let hash = tmp.path().join("hash");
    std::fs::create_dir(&hash).unwrap();
    let sock = hash.join("control.sock");
    let _listener = tokio::net::UnixListener::bind(&sock).unwrap();
    d.cfg.discovery = Discovery::with_base(tmp.path().to_path_buf());
    d.roster.insert("aabbccdd".into());
    d.short_by_session.insert("session".into(), "aabbccdd".into());
    d.poll_once().await.unwrap();
    assert!(d.roster.contains("aabbccdd"));
    assert_eq!(d.short_by_session.get("session").as_deref(), Some("aabbccdd"));
    assert!(!d.churned, "a slow probe must not trigger reattachment churn");
    assert!(events.try_recv().is_err(), "no false session end may be emitted");
    assert_eq!(
        d.ensure_socket().await.unwrap(),
        sock,
        "a listening daemon needs no duplicate supervisor"
    );
}

#[test]
fn a_peer_turn_waits_for_an_open_ask_form_and_a_human_reply_does_not() {
    let peer = "<cross-session-message from=\"remote:x\" origin=\"remote\">\nyes\n\
                </cross-session-message>";
    assert!(super::defers_for_ask(true, peer));
    assert!(super::defers_for_ask(true, "<cctui-room name=\"ops\" from=\"a\">\nhi\n</cctui-room>"));
    assert!(!super::defers_for_ask(false, peer), "no form up: delivered at once");
    assert!(!super::defers_for_ask(true, "go with option 2"), "the human still answers the form");
}

#[test]
fn an_ask_that_opens_mid_flush_is_recognised_so_the_turn_is_requeued() {
    let err: anyhow::Error = super::reply::AskOpened.into();
    assert!(err.is::<super::reply::AskOpened>());
    assert!(!anyhow::anyhow!("socket gone").is::<super::reply::AskOpened>());
}

#[tokio::test]
async fn a_dead_worker_drops_its_stale_ask_and_an_ended_one_its_deferred_peer_turns() {
    let peer = "<cctui-room name=\"ops\" from=\"a\">\nhi\n</cctui-room>";
    let (mut d, mut rx) = driver();
    for id in ["hib", "gone"] {
        d.pending_asks.lock().unwrap().insert(id.to_owned(), None);
        assert!(matches!(d.send_message(id.to_owned(), peer.to_owned()).await, Ok(Handled::Done)));
    }
    assert_eq!(d.deferred_peer.len(), 2);

    d.mark_dead("f00dc1a1", "hib", true).await;
    d.mark_dead("f00dc1a2", "gone", false).await;

    assert!(d.pending_asks.lock().unwrap().is_empty(), "no ask outlives its worker");
    assert!(d.deferred_peer.contains_key("hib"), "a hibernated worker still gets its turns");
    assert!(!d.deferred_peer.contains_key("gone"), "an ended session's turns are dropped");
    let mut resolved = Vec::new();
    while let Ok(evt) = rx.try_recv() {
        if let AdapterEvent::AskResolved { local_id } = evt {
            resolved.push(local_id);
        }
    }
    assert_eq!(resolved, ["hib", "gone"]);
}

#[tokio::test]
async fn the_interrupter_drops_a_stale_ask_only_when_one_is_up() {
    let (d, mut rx) = driver();
    d.pending_asks.lock().unwrap().insert("s".to_owned(), None);
    super::drop_stale_ask(&d.pending_asks, &d.events, "s").await;
    super::drop_stale_ask(&d.pending_asks, &d.events, "s").await;
    assert!(d.pending_asks.lock().unwrap().is_empty());
    assert!(matches!(rx.try_recv(), Ok(AdapterEvent::AskResolved { .. })));
    assert!(matches!(rx.try_recv(), Ok(AdapterEvent::PlanResolved { .. })));
    assert!(rx.try_recv().is_err(), "a second drop emits nothing");
}
