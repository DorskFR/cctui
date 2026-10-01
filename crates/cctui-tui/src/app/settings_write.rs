//! The one path that writes `/settings`.
//!
//! `PUT /settings` replaces the row and the route has no merge or `If-Match`,
//! so a writer that sends a blob it read at startup silently reverts whatever
//! the web UI saved since — and a writer that never read one at all drops every
//! key it has not heard of. Both writes therefore carry only the keys they own,
//! and the body is built by merging that patch into a read taken immediately
//! before the write. A read that fails writes nothing.

use serde_json::Value;

use super::action::Effect;

/// The body a settings write should send.
#[derive(Debug, Clone, PartialEq)]
pub enum WritePlan {
    Put(Value),
    /// The pre-write read failed, so the merge base is unknown. Replacing the
    /// row from a patch alone would wipe every key the patch does not name.
    Refuse,
}

/// Merge `patch` into `base`: objects merge key by key, anything else replaces.
///
/// Recursive so a patch of one nested key (`sessionList.sort`) keeps its
/// siblings (`sessionList.someWebUiOnlyKey`), which a shallow insert would drop.
pub fn deep_merge(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            for (key, value) in patch {
                match base.get_mut(&key) {
                    Some(slot) => deep_merge(slot, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, patch) => *base = patch,
    }
}

/// `fresh` is the body of the read taken just before the write, `None` when it
/// failed.
#[must_use]
pub fn plan(fresh: Option<Value>, patch: Value) -> WritePlan {
    let Some(mut body) = fresh else { return WritePlan::Refuse };
    if !body.is_object() {
        body = Value::Object(serde_json::Map::new());
    }
    deep_merge(&mut body, patch);
    WritePlan::Put(body)
}

/// The one way to persist a settings change: name the keys you own, nothing else.
#[must_use]
pub fn save(patch: Value) -> Effect {
    Effect::SaveSettings { patch }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{WritePlan, deep_merge, plan};

    /// Everything the TUI has never heard of, as the web UI stores it.
    fn server_blob() -> serde_json::Value {
        json!({
            "theme": "dark",
            "macros": [{"name": "ship", "prompt": "ship it"}],
            "secretScrubPatterns": ["sk-[a-z]+"],
            "whipStopPhrases": ["stop"],
            "plugins": {"yubisashi": {"enabled": true}},
            "spawnMemory": {"recentDirs": ["/src"]},
            "sessionList": {"sort": "name", "colorBy": "machine", "webUiOnlyKey": 7},
            "harnessMode": "bg"
        })
    }

    #[test]
    fn a_failed_pre_write_read_writes_nothing() {
        assert_eq!(plan(None, json!({"harnessMode": "sdk"})), WritePlan::Refuse);
    }

    #[test]
    fn a_write_keeps_every_key_it_does_not_own() {
        let WritePlan::Put(body) = plan(Some(server_blob()), json!({"harnessMode": "sdk"})) else {
            panic!("a readable settings row is writable");
        };
        assert_eq!(body["harnessMode"], json!("sdk"), "the owned key moved");
        assert_eq!(body["theme"], json!("dark"));
        assert_eq!(body["macros"], server_blob()["macros"]);
        assert_eq!(body["secretScrubPatterns"], server_blob()["secretScrubPatterns"]);
        assert_eq!(body["whipStopPhrases"], server_blob()["whipStopPhrases"]);
        assert_eq!(body["plugins"], server_blob()["plugins"]);
        assert_eq!(body["spawnMemory"], server_blob()["spawnMemory"]);
    }

    #[test]
    fn a_nested_patch_keeps_its_siblings() {
        let WritePlan::Put(body) =
            plan(Some(server_blob()), json!({"sessionList": {"sort": "activity"}}))
        else {
            panic!("writable");
        };
        assert_eq!(body["sessionList"]["sort"], json!("activity"));
        assert_eq!(body["sessionList"]["colorBy"], json!("machine"), "untouched sibling survives");
        assert_eq!(body["sessionList"]["webUiOnlyKey"], json!(7), "a key we never parse survives");
    }

    /// The merge base is whatever the server holds now, so a change the web UI
    /// made while the TUI was open is not reverted by the TUI's next write.
    #[test]
    fn the_merge_base_is_the_fresh_read_not_what_the_tui_remembers() {
        let mut later = server_blob();
        later["theme"] = json!("light");
        later["sessionList"]["colorBy"] = json!("label");

        let WritePlan::Put(body) = plan(Some(later), json!({"sessionList": {"sort": "activity"}}))
        else {
            panic!("writable");
        };
        assert_eq!(body["theme"], json!("light"), "the web UI's newer value stands");
        assert_eq!(body["sessionList"]["colorBy"], json!("label"));
        assert_eq!(body["sessionList"]["sort"], json!("activity"));
    }

    #[test]
    fn a_row_that_is_not_an_object_is_written_as_one() {
        let WritePlan::Put(body) = plan(Some(json!(null)), json!({"harnessMode": "sdk"})) else {
            panic!("an empty settings row is still writable");
        };
        assert_eq!(body, json!({"harnessMode": "sdk"}));
    }

    #[test]
    fn merging_replaces_scalars_and_arrays_but_recurses_into_objects() {
        let mut base = json!({"a": 1, "arr": [1, 2], "o": {"x": 1, "y": 2}});
        deep_merge(&mut base, json!({"a": 2, "arr": [9], "o": {"y": 3, "z": 4}, "new": true}));
        assert_eq!(base, json!({"a": 2, "arr": [9], "o": {"x": 1, "y": 3, "z": 4}, "new": true}));
    }
}
