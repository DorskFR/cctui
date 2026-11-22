use serde_json::json;

use super::*;

#[test]
fn the_setting_defaults_on_with_a_three_minute_grace() {
    let want = Policy { enabled: true, grace_secs: 180, keep_profiles: vec![] };
    assert_eq!(Policy::from_settings(None), want);
    assert_eq!(Policy::from_settings(Some(&json!({}))), want);
    assert_eq!(Policy::from_settings(Some(&json!({ ENABLED_KEY: "no", GRACE_KEY: "x" }))), want);
}

#[test]
fn the_setting_reads_the_owners_choices() {
    let p = Policy::from_settings(Some(&json!({
        ENABLED_KEY: false,
        GRACE_KEY: 0,
        KEEP_PROFILES_KEY: [" review ", "review", "", 3],
    })));
    assert_eq!(p, Policy { enabled: false, grace_secs: 0, keep_profiles: vec!["review".into()] });
    assert_eq!(Policy::from_settings(Some(&json!({ GRACE_KEY: 99_999 }))).grace_secs, 3600);
    assert_eq!(Policy::from_settings(Some(&json!({ GRACE_KEY: -5 }))).grace_secs, 0);
}

#[test]
fn a_kept_profile_or_a_disabled_setting_covers_nothing() {
    let p = Policy { keep_profiles: vec!["review".into()], ..Policy::default() };
    assert!(p.covers(None));
    assert!(p.covers(Some("build")));
    assert!(!p.covers(Some("review")));
    assert!(!Policy { enabled: false, ..Policy::default() }.covers(None));
}

#[test]
fn the_grace_counts_from_the_end() {
    let end = Utc::now();
    let p = Policy::default();
    assert!(!p.due(end, end + chrono::Duration::seconds(179)));
    assert!(p.due(end, end + chrono::Duration::seconds(180)));
    assert!(Policy { grace_secs: 0, ..Policy::default() }.due(end, end));
}

#[test]
fn clamp_drops_wrong_types_and_normalizes_the_rest() {
    let mut data = json!({
        ENABLED_KEY: "yes",
        GRACE_KEY: 7200,
        KEEP_PROFILES_KEY: ["a", " a ", "b"],
        "other": 1,
    });
    clamp(&mut data);
    assert_eq!(data, json!({ GRACE_KEY: 3600, KEEP_PROFILES_KEY: ["a", "b"], "other": 1 }));

    let mut data = json!({ ENABLED_KEY: false, GRACE_KEY: "soon", KEEP_PROFILES_KEY: [] });
    clamp(&mut data);
    assert_eq!(data, json!({ ENABLED_KEY: false }));
}

#[test]
fn a_session_left_on_a_question_waits_on_the_user() {
    assert!(waits_on_user(Some("blocked"), None, None));
    assert!(!waits_on_user(None, Some("stopped"), Some("success")));
    assert!(!waits_on_user(None, Some("done"), Some("failure")));
}
