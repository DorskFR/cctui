use chrono::{DateTime, Utc};
use serde_json::json;

use super::{CreateEvent, DEFAULT_TOPIC, Target, validate};
use crate::inbox::Source;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-16T10:00:00Z").unwrap().with_timezone(&Utc)
}

fn req(value: serde_json::Value) -> CreateEvent {
    serde_json::from_value(value).expect("valid request shape")
}

fn rejection(value: serde_json::Value) -> String {
    validate(req(value), now()).expect_err("rejected")
}

#[test]
fn a_session_event_validates() {
    let valid = validate(
        req(json!({
            "session_id": " s1 ",
            "source": "webhook",
            "source_id": "gh-delivery-1",
            "body": "push",
            "payload": {"ref": "main"},
            "not_before": "2026-11-16T11:00:00Z",
            "expires_at": "2026-11-16T13:00:00+01:00",
        })),
        now(),
    )
    .unwrap();
    assert_eq!(valid.target, Target::Session("s1".into()));
    assert_eq!(valid.source, Source::Webhook);
    assert_eq!(valid.source_id.as_deref(), Some("gh-delivery-1"));
    assert_eq!(valid.payload, Some(json!({"ref": "main"})));
    assert_eq!(valid.not_before, Some(now() + chrono::Duration::hours(1)));
    assert_eq!(valid.expires_at, Some(now() + chrono::Duration::hours(2)));
}

#[test]
fn external_sources_need_a_source_id_and_internal_ones_do_not() {
    for source in ["n8n", "webhook"] {
        let err = rejection(json!({"session_id": "s1", "source": source, "body": "x"}));
        assert!(err.contains("source_id is required"), "{source}: {err}");
        let blank = json!({"session_id": "s1", "source": source, "source_id": "  ", "body": "x"});
        let err = rejection(blank);
        assert!(err.contains("source_id is required"), "{source}: blank is missing");
    }
    let human = validate(req(json!({"session_id": "s1", "source": "human", "body": "x"})), now());
    assert_eq!(human.unwrap().source_id, None);
}

#[test]
fn a_target_is_a_session_or_a_preset_with_a_topic() {
    let preset = |extra: serde_json::Value| {
        let mut v = json!({"preset": " nightly ", "source": "n8n", "source_id": "a", "body": "x"});
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        validate(req(v), now()).map(|valid| valid.target)
    };
    assert_eq!(
        preset(json!({})),
        Ok(Target::Preset { preset: "nightly".into(), topic: DEFAULT_TOPIC.into() })
    );
    assert_eq!(
        preset(json!({"topic": " ci "})),
        Ok(Target::Preset { preset: "nightly".into(), topic: "ci".into() })
    );
    assert!(preset(json!({"topic": "t".repeat(121)})).unwrap_err().contains("topic"));
    assert!(preset(json!({"session_id": "s1"})).unwrap_err().contains("not both"));
    let err = rejection(json!({"source": "n8n", "source_id": "a", "body": "x"}));
    assert!(err.contains("session_id or preset is required"), "{err}");
    let err = rejection(
        json!({"session_id": "s1", "topic": "t", "source": "n8n", "source_id": "a", "body": "x"}),
    );
    assert!(err.contains("topic"), "{err}");
}

#[test]
fn malformed_events_are_refused() {
    let base = |extra: &serde_json::Value| {
        let mut v = json!({"session_id": "s1", "source": "timer", "body": "wake"});
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    };
    for (extra, needle) in [
        (json!({"source": "github"}), "unknown source"),
        (json!({"body": "  "}), "body must not be empty"),
        (json!({"body": "x".repeat(64 * 1024 + 1)}), "body must be at most"),
        (json!({"source_id": "x".repeat(257)}), "source_id must be at most"),
        (json!({"payload": [1, 2]}), "payload must be a JSON object"),
        (json!({"payload": {"blob": "x".repeat(9000)}}), "payload must be at most"),
        (json!({"payload": {"env": {"A": "b"}}}), "drop 'env'"),
        (json!({"payload": {"nested": [{"GitHub_Token": "t"}]}}), "drop 'GitHub_Token'"),
        (json!({"not_before": "tomorrow"}), "not_before must be an RFC3339"),
        (json!({"not_before": "2026-12-16T10:00:01Z"}), "at most 30 days"),
        (json!({"expires_at": "2026-11-16T09:00:00Z"}), "expires_at must be after"),
        (
            json!({"not_before": "2026-11-17T10:00:00Z", "expires_at": "2026-11-17T09:00:00Z"}),
            "expires_at must be after",
        ),
    ] {
        let err = rejection(base(&extra));
        assert!(err.contains(needle), "{extra}: {err}");
    }
    let smuggled = json!({"session_id": "s1", "source": "n8n", "body": "x", "reply_url": "u"});
    let unknown: Result<CreateEvent, _> = serde_json::from_value(smuggled);
    assert!(unknown.is_err(), "an unknown field is refused, not silently stored");
}
