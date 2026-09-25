use super::test_support::*;
use super::*;

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
        d.emit(AdapterEvent::Message {
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

    d.emit(AdapterEvent::Message {
        local_id: "sess-1".into(),
        payload: json!({"role": "assistant", "text": "on it"}),
        turn_id: None,
    })
    .await;
    let AdapterEvent::Message { turn_id, .. } = rx.recv().await.unwrap() else {
        panic!("want Message")
    };
    assert_eq!(turn_id, None, "assistant text must not inherit the turn id");

    d.emit(AdapterEvent::Message {
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
    d.emit(AdapterEvent::Message {
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
