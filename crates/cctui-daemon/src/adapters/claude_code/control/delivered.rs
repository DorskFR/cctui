//! Text cctui itself delivered into a session is human by construction, so the
//! body-only meta heuristic must not be consulted for it: a human prompt that
//! pastes a transcript or an export carries harness wrappers and would be filed
//! as harness-injected.
//!
//! A composer reply carries a turn id, which is proof of delivery on its own.
//! The spawn prompt has none — it rides the launch argv, before the session
//! exists — so its body is fingerprinted here and matched when the transcript
//! tail first reads it back.

use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};

use cctui_proto::adapter::AdapterEvent;

use super::Driver;

/// Fingerprints kept per session. Enough for a spawn prompt plus a few replies;
/// the oldest is dropped past that. Never expired by time: a match can only
/// force a turn to `meta:false`, and the only text that matches is text a human
/// sent, so a late or repeated hit is still the right answer.
const MAX_FINGERPRINTS: usize = 8;

/// Prefix length used for the second, tolerant fingerprint. A 350 KB pasted
/// prompt can come back from the transcript with its tail re-encoded (block
/// joining, attachment expansion), which the full-body hash misses.
const HEAD_CHARS: usize = 256;

/// Full-body and head-prefix hashes of one delivered message. The head hash is
/// absent for bodies shorter than [`HEAD_CHARS`], where it would be the full
/// body and match far too much.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Fingerprint {
    full: u64,
    head: Option<u64>,
}

#[derive(Default)]
pub(super) struct Delivered(std::sync::Mutex<HashMap<String, Vec<Fingerprint>>>);

/// Line endings and surrounding blank space are not content: the composer, the
/// launch argv and the transcript disagree about them.
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").trim().to_owned()
}

/// `DefaultHasher::new` is seeded identically on every call, unlike
/// `RandomState`: two fingerprints of the same body must be equal.
fn hash(text: &str) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn fingerprint(text: &str) -> Option<Fingerprint> {
    let normalized = normalize(text);
    if normalized.is_empty() {
        return None;
    }
    let head = normalized.char_indices().nth(HEAD_CHARS).map(|(at, _)| hash(&normalized[..at]));
    Some(Fingerprint { full: hash(&normalized), head })
}

impl Fingerprint {
    const fn matches(self, other: Self) -> bool {
        self.full == other.full || matches!((self.head, other.head), (Some(a), Some(b)) if a == b)
    }
}

impl Driver {
    /// Record text this daemon is about to deliver to `local_id`.
    pub(super) fn note_delivered(&self, local_id: &str, text: &str) {
        let Some(fp) = fingerprint(text) else { return };
        let Ok(mut map) = self.delivered.0.lock() else { return };
        let seen = map.entry(local_id.to_owned()).or_default();
        if seen.contains(&fp) {
            return;
        }
        seen.push(fp);
        if seen.len() > MAX_FINGERPRINTS {
            seen.remove(0);
        }
    }

    fn was_delivered(&self, local_id: &str, text: &str) -> bool {
        let Some(fp) = fingerprint(text) else { return false };
        let Ok(map) = self.delivered.0.lock() else { return false };
        map.get(local_id).is_some_and(|seen| seen.iter().any(|&s| s.matches(fp)))
    }

    /// Clear `meta` on a user event cctui delivered itself. Applied only to
    /// events parsed out of transcript bytes read for the first time, so a
    /// replay window cannot re-hash a row the server already stored.
    pub(super) fn unmask_delivered(&self, mut evt: AdapterEvent) -> AdapterEvent {
        if let AdapterEvent::Message { local_id, payload, turn_id } = &mut evt
            && payload.get("role").and_then(serde_json::Value::as_str) == Some("user")
            && payload.get("meta").and_then(serde_json::Value::as_bool) == Some(true)
        {
            let text = payload.get("text").and_then(serde_json::Value::as_str).unwrap_or_default();
            let ours = turn_id.is_some() || self.was_delivered(local_id.as_str(), text);
            if ours && let Some(obj) = payload.as_object_mut() {
                obj.insert("meta".to_owned(), serde_json::Value::Bool(false));
            }
        }
        evt
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::driver;
    use super::*;
    use serde_json::json;

    fn meta_of(evt: &AdapterEvent) -> Option<bool> {
        let AdapterEvent::Message { payload, .. } = evt else { return None };
        payload.get("meta").and_then(serde_json::Value::as_bool)
    }

    fn user(text: &str, turn_id: Option<uuid::Uuid>) -> AdapterEvent {
        AdapterEvent::Message {
            local_id: "sess-1".into(),
            payload: json!({"role": "user", "text": text, "meta": true}),
            turn_id,
        }
    }

    #[test]
    fn a_delivered_spawn_prompt_is_human_even_without_a_turn_id() {
        let (d, _rx) = driver();
        let quoted = "line\n".repeat(200);
        let pasted = format!("**User:** hello\n{quoted}<task-notification>x</task-notification>");
        d.note_delivered("sess-1", &pasted);
        assert_eq!(meta_of(&d.unmask_delivered(user(&pasted, None))), Some(false));
        // Trailing-whitespace / CRLF drift must not break the match.
        let drifted = format!("{}\r\n\r\n", pasted.replace('\n', "\r\n"));
        assert_eq!(meta_of(&d.unmask_delivered(user(&drifted, None))), Some(false));
    }

    #[test]
    fn a_long_delivered_prompt_matches_on_its_head_when_the_tail_drifts() {
        let (d, _rx) = driver();
        let body = "x".repeat(HEAD_CHARS * 2);
        d.note_delivered("sess-1", &body);
        let re_encoded = format!("{body}\n[Image #1]");
        assert_eq!(meta_of(&d.unmask_delivered(user(&re_encoded, None))), Some(false));
    }

    #[test]
    fn text_cctui_never_delivered_keeps_its_meta_flag() {
        let (d, _rx) = driver();
        d.note_delivered("sess-1", "take over please");
        // Another session's delivery does not vouch for this one.
        d.note_delivered("sess-2", "<task-notification>x</task-notification>");
        let harness = user("<task-notification>x</task-notification>", None);
        assert_eq!(meta_of(&d.unmask_delivered(harness)), Some(true));
        // Short bodies must not match on a head prefix they do not have.
        assert_eq!(meta_of(&d.unmask_delivered(user("take over", None))), Some(true));
    }

    #[test]
    fn a_turn_id_alone_proves_the_reply_was_ours() {
        let (d, _rx) = driver();
        let quoted = "<task-notification>quoted by a human</task-notification>";
        let evt = user(quoted, Some(uuid::Uuid::new_v4()));
        assert_eq!(meta_of(&d.unmask_delivered(evt)), Some(false));
    }

    #[test]
    fn non_user_payloads_are_left_alone() {
        let (d, _rx) = driver();
        d.note_delivered("sess-1", "hi");
        let marker = AdapterEvent::Message {
            local_id: "sess-1".into(),
            payload: json!({"role": "system_marker", "marker": "interrupted", "meta": true}),
            turn_id: None,
        };
        assert_eq!(meta_of(&d.unmask_delivered(marker)), Some(true));
    }
}
