use cctui_proto::api::auto_followup::{
    AutoFollowupSetting, AutoFollowupThreshold, ContextWindow, SendFollowup, ThresholdUnit,
    WindowSource,
};
use serde_json::json;

use super::{Hold, Signals, fires, hold, key_for, setting_of, threshold_at, window_of};

const fn tokens(value: u64) -> AutoFollowupThreshold {
    AutoFollowupThreshold { unit: ThresholdUnit::Tokens, value }
}

const fn percent(value: u64) -> AutoFollowupThreshold {
    AutoFollowupThreshold { unit: ThresholdUnit::Percent, value }
}

const ON_150K: AutoFollowupSetting =
    AutoFollowupSetting { enabled: true, threshold: tokens(150_000) };

const IDLE: Signals<'static> = Signals {
    status: "active",
    ended: false,
    pinned: false,
    child: false,
    tempo: None,
    agent_state: Some("idle"),
    activity: Some("success"),
    soft_limit: None,
};

fn on_150k() -> serde_json::Value {
    json!({"enabled": true, "threshold": {"unit": "tokens", "value": 150_000}})
}

#[test]
fn context_tokens_sums_input_and_cache_of_the_newest_turn() {
    let at = chrono::Utc::now();
    let rows = vec![
        ("s".to_owned(), 1_000, 150_000, 9_000, at, 1),
        ("s".to_owned(), 999_999, 0, 0, at - chrono::Duration::minutes(1), 2),
        ("empty".to_owned(), 0, 0, 0, at, 1),
    ];
    let turns = crate::routes::sessions::fold_last_turns(rows);
    assert_eq!(turns["s"].context_tokens(), Some(160_000), "the newest row only");
    assert_eq!(turns["empty"].context_tokens(), None);
}

#[test]
fn a_codex_turn_counts_the_same_prompt_size() {
    let at = chrono::Utc::now();
    let (codex_input, codex_cached) = (11_860_i64, 9_600_i64);
    let rows = vec![
        ("codex".to_owned(), codex_input - codex_cached, codex_cached, 0, at, 1),
        ("claude".to_owned(), 2_260, 9_000, 600, at, 1),
    ];
    let turns = crate::routes::sessions::fold_last_turns(rows);
    assert_eq!(turns["codex"].context_tokens(), Some(11_860), "codex reports input with cache");
    assert_eq!(turns["claude"].context_tokens(), turns["codex"].context_tokens());
}

fn claude(
    reported: Option<&str>,
    launched: Option<&str>,
    observed: Option<u64>,
) -> Option<ContextWindow> {
    window_of(Some("claude-code"), reported, launched, &json!({}), observed)
}

const fn window(tokens: u64, source: WindowSource) -> ContextWindow {
    ContextWindow { tokens, source }
}

#[test]
fn context_window_resolves_1m_claude_and_unknown_models() {
    let none = json!({});
    assert_eq!(claude(Some("opus[1m]"), None, None), Some(window(1_000_000, WindowSource::Known)));
    let meta_model = json!({"model": "claude-opus-5-5[1m]"});
    let from_meta = window_of(Some("claude-code"), None, None, &meta_model, None);
    assert_eq!(from_meta.map(|w| w.tokens), Some(1_000_000));
    let codex = json!({"model_context_window": 400_000});
    let reported = window_of(Some("codex"), Some("gpt-5"), None, &codex, None);
    assert_eq!(reported, Some(window(400_000, WindowSource::Known)));
    let catalog = window_of(Some("codex"), Some("gpt-5"), None, &none, None);
    assert_eq!(catalog.map(|w| w.tokens), Some(cctui_proto::api::auto_followup::CODEX_WINDOW));
    assert_eq!(window_of(Some("opencode"), Some("big"), None, &none, None), None);
    assert_eq!(window_of(None, None, None, &none, None), None);
}

#[test]
fn a_1m_spawn_resolves_a_1m_window_though_the_harness_reports_the_base_model() {
    let resolved = claude(Some("claude-opus-5-5"), Some("claude-opus-5-5[1m]"), Some(10_000));
    assert_eq!(resolved, Some(window(1_000_000, WindowSource::Known)));
    let pct = AutoFollowupSetting { enabled: true, threshold: percent(80) };
    assert_eq!(threshold_at(pct, resolved, None), Some(800_000), "80% of 1M, not of 200k");
}

#[test]
fn observed_context_above_200k_implies_a_1m_window() {
    let prod = claude(Some("claude-opus-5-5"), None, Some(363_044));
    assert_eq!(prod, Some(window(1_000_000, WindowSource::Observed)));
    let launched_base = claude(Some("claude-opus-5-5"), Some("claude-opus-5-5"), Some(426_000));
    let ran_1m = Some(window(1_000_000, WindowSource::Observed));
    assert_eq!(launched_base, ran_1m, "launched plain, ran 1M");
    assert_eq!(claude(None, None, Some(200_000)), Some(window(200_000, WindowSource::Assumed)));
    let pct = AutoFollowupSetting { enabled: true, threshold: percent(50) };
    assert_eq!(threshold_at(pct, prod, None), Some(500_000));
}

#[test]
fn a_plain_claude_session_stays_at_200k() {
    let launched = claude(Some("claude-opus-5-5"), Some("claude-opus-5-5"), Some(150_000));
    let assumed = Some(window(200_000, WindowSource::Assumed));
    assert_eq!(launched, assumed, "a plain launch can run 1M");
    let guessed = claude(Some("claude-opus-5-5"), None, Some(150_000));
    assert_eq!(guessed, Some(window(200_000, WindowSource::Assumed)));
    let pct = AutoFollowupSetting { enabled: true, threshold: percent(80) };
    assert_eq!(threshold_at(pct, launched, None), None, "never a percent of a guessed window");
    assert_eq!(threshold_at(pct, guessed, None), None);
    assert_eq!(threshold_at(ON_150K, guessed, None), Some(150_000), "tokens need no window");
}

#[test]
fn a_malformed_setting_falls_back_to_the_default() {
    let off = AutoFollowupSetting::default();
    assert_eq!(setting_of(&json!({})), off);
    assert_eq!(setting_of(&json!({"autoFollowup": "yes"})), off);
    assert_eq!(setting_of(&json!({"autoFollowup": {"enabled": true}})), off);
    let out_of_range = json!({"autoFollowup": {"enabled": true,
        "threshold": {"unit": "percent", "value": 250}}});
    assert_eq!(setting_of(&out_of_range), off);
    assert_eq!(setting_of(&json!({"autoFollowup": on_150k()})), ON_150K);
}

#[test]
fn under_the_threshold_or_setting_off_is_a_plain_send() {
    let at = threshold_at(ON_150K, None, None);
    assert_eq!(at, Some(150_000));
    assert!(fires(SendFollowup::Auto, "go on", Some(150_000), at));
    assert!(!fires(SendFollowup::Auto, "go on", Some(149_999), at));
    assert!(!fires(SendFollowup::Auto, "go on", None, at));
    assert!(!fires(SendFollowup::Auto, "  ", Some(200_000), at), "nothing to say");
    let off = AutoFollowupSetting { enabled: false, ..ON_150K };
    assert_eq!(threshold_at(off, Some(window(200_000, WindowSource::Known)), None), None);
}

#[test]
fn a_percent_threshold_without_a_known_window_never_fires() {
    let pct = AutoFollowupSetting { enabled: true, threshold: percent(80) };
    assert_eq!(threshold_at(pct, Some(window(200_000, WindowSource::Known)), None), Some(160_000));
    let unknown = window_of(Some("opencode"), Some("big"), None, &json!({}), None);
    assert_eq!(threshold_at(pct, unknown, None), None);
    assert!(!fires(SendFollowup::Auto, "go on", Some(u64::MAX), None));
}

#[test]
fn exclusions_children_pinned_waiting_and_mid_turn_never_fire() {
    assert_eq!(hold(&IDLE), None);
    assert_eq!(hold(&Signals { pinned: true, ..IDLE }), Some(Hold::Pinned));
    assert_eq!(hold(&Signals { child: true, ..IDLE }), Some(Hold::Child));
    assert_eq!(hold(&Signals { status: "archived", ..IDLE }), Some(Hold::Closed));
    assert_eq!(hold(&Signals { ended: true, ..IDLE }), Some(Hold::Closed));
    let working = Signals { agent_state: Some("working"), activity: None, ..IDLE };
    assert_eq!(hold(&working), Some(Hold::Busy));
    let asking = Signals { tempo: Some("blocked"), activity: None, ..IDLE };
    assert_eq!(hold(&asking), Some(Hold::Waiting));
    for held in [Hold::Pinned, Hold::Child, Hold::Closed, Hold::Busy, Hold::Waiting] {
        assert_eq!(threshold_at(ON_150K, None, Some(held)), None, "{held:?}");
    }
}

#[test]
fn a_message_key_names_the_followup_op() {
    assert_eq!(key_for(Some("t1")), "auto:t1");
    assert!(key_for(None).starts_with("auto:"));
    assert_ne!(key_for(Some("  ")), "auto:");
    let long = "x".repeat(200);
    assert!(key_for(Some(&long)).len() <= cctui_proto::api::followup::IDEMPOTENCY_KEY_MAX);
}
