//! Reading a draft session's stored payload, shared with the web UI's
//! `sessions.logic.ts`.
//!
//! A draft keeps its spawn payload under `metadata.draft`. Values the user
//! must re-enter — the env — are never in there; only the names are.

use serde_json::Value;

/// The stored payload of a draft row, or an empty object for a live session.
#[must_use]
pub fn draft_payload(metadata: &Value) -> &Value {
    const EMPTY: &Value = &Value::Null;
    metadata.get("draft").filter(|d| d.is_object()).unwrap_or(EMPTY)
}

/// The prompt a draft row previews.
#[must_use]
pub fn draft_prompt(metadata: &Value) -> String {
    draft_payload(metadata).get("prompt").and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// The env var names a launch asks for. Values are never stored, so a launch
/// always re-enters them.
#[must_use]
pub fn draft_env_keys(metadata: &Value) -> Vec<String> {
    draft_payload(metadata)
        .get("env_keys")
        .and_then(Value::as_array)
        .map(|keys| {
            keys.iter()
                .filter_map(Value::as_str)
                .filter(|k| !k.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The labels a draft launches with: its payload's, then any added to the row
/// since it was saved.
#[must_use]
pub fn draft_label_ids(metadata: &Value, row_labels: &[String]) -> Vec<String> {
    let stored: Vec<String> = draft_payload(metadata)
        .get("label_ids")
        .and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let mut out = stored;
    for id in row_labels {
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{draft_env_keys, draft_label_ids, draft_payload, draft_prompt};

    #[test]
    fn a_live_session_has_no_draft_payload() {
        for metadata in [json!({}), json!({ "draft": "not an object" }), json!(null)] {
            assert!(draft_payload(&metadata).get("prompt").is_none());
            assert_eq!(draft_prompt(&metadata), "");
            assert!(draft_env_keys(&metadata).is_empty());
        }
    }

    #[test]
    fn a_draft_carries_its_prompt_and_env_names_but_no_values() {
        let metadata = json!({
            "draft": {
                "prompt": "do the thing",
                "env_keys": ["TOKEN", "", "HOST"],
                "machine_id": "m-1",
            }
        });
        assert_eq!(draft_prompt(&metadata), "do the thing");
        assert_eq!(draft_env_keys(&metadata), vec!["TOKEN".to_owned(), "HOST".to_owned()]);
        assert!(
            draft_payload(&metadata).get("env").is_none(),
            "a stored draft never holds env values"
        );
    }

    #[test]
    fn a_launch_keeps_the_payloads_labels_and_the_rows_own() {
        let metadata = json!({ "draft": { "label_ids": ["l1", "l2"] } });
        assert_eq!(
            draft_label_ids(&metadata, &["l2".to_owned(), "l3".to_owned()]),
            vec!["l1".to_owned(), "l2".to_owned(), "l3".to_owned()],
            "stored first, then what the row gained, each once"
        );
        assert_eq!(draft_label_ids(&json!({}), &["l1".to_owned()]), vec!["l1".to_owned()]);
    }
}
