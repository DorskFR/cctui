//! Per-session plugin data slots: `sessions.metadata.plugins.<plugin_id>`.
//!
//! The allow-list is a built-in registry extended by installed plugin ids, not
//! the `plugins` table alone: first-party connectors (`YouTrack`, Slack, GitHub)
//! have no bundle and would never be writable, and uninstalling a bundle would
//! make a session's stored slot unwritable while it is still rendered.

/// Session metadata is read on every list refresh; a slot is a chip's worth of
/// data, not a document.
pub const MAX_SLOT_BYTES: usize = 4096;

pub const BUILTIN_SLOTS: &[&str] = &["youtrack", "slack", "github"];

/// Whether `plugin_id` may own a slot. `installed` answers whether a plugin
/// bundle with that id exists on this instance.
pub fn slot_allowed(plugin_id: &str, installed: bool) -> bool {
    crate::plugins::valid_id(plugin_id) && (installed || BUILTIN_SLOTS.contains(&plugin_id))
}

/// Serialized size of a slot payload, for the cap check.
pub fn slot_size(data: &serde_json::Value) -> usize {
    serde_json::to_vec(data).map_or(usize::MAX, |v| v.len())
}

/// Write or clear one slot in `metadata`, creating `plugins` if needed and
/// dropping it again once empty so an untouched session keeps a bare object.
pub fn apply_slot(
    metadata: &mut serde_json::Value,
    plugin_id: &str,
    data: Option<serde_json::Value>,
) {
    if !metadata.is_object() {
        *metadata = serde_json::Value::Object(serde_json::Map::new());
    }
    let Some(root) = metadata.as_object_mut() else { return };
    if let Some(value) = data {
        let slots = root
            .entry("plugins".to_owned())
            .and_modify(|v| {
                if !v.is_object() {
                    *v = serde_json::Value::Object(serde_json::Map::new());
                }
            })
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(map) = slots.as_object_mut() {
            map.insert(plugin_id.to_owned(), value);
        }
    } else {
        let empty = root
            .get_mut("plugins")
            .and_then(serde_json::Value::as_object_mut)
            .is_some_and(|slots| {
                slots.remove(plugin_id);
                slots.is_empty()
            });
        if empty {
            root.remove("plugins");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builtin_slots_are_allowed_without_an_installed_bundle() {
        assert!(slot_allowed("youtrack", false));
        assert!(slot_allowed("slack", false));
    }

    #[test]
    fn an_installed_bundle_extends_the_allow_list() {
        assert!(!slot_allowed("yubisashi", false));
        assert!(slot_allowed("yubisashi", true));
    }

    #[test]
    fn a_malformed_id_is_refused_even_when_installed() {
        assert!(!slot_allowed("Not An Id", true));
        assert!(!slot_allowed("", true));
        assert!(!slot_allowed("../etc", true));
    }

    #[test]
    fn slot_size_counts_serialized_bytes() {
        assert_eq!(slot_size(&json!({})), 2);
        assert!(slot_size(&json!({ "issue": "CCT-910" })) < MAX_SLOT_BYTES);
        let big = json!({ "issue": "x".repeat(MAX_SLOT_BYTES) });
        assert!(slot_size(&big) > MAX_SLOT_BYTES);
    }

    #[test]
    fn apply_creates_the_plugins_object_and_keeps_siblings() {
        let mut meta = json!({ "draft": { "prompt": "hi" } });
        apply_slot(&mut meta, "youtrack", Some(json!({ "issue": "CCT-910" })));
        assert_eq!(meta["plugins"]["youtrack"]["issue"], "CCT-910");
        assert_eq!(meta["draft"]["prompt"], "hi");
    }

    #[test]
    fn apply_replaces_one_slot_without_touching_another() {
        let mut meta =
            json!({ "plugins": { "youtrack": { "issue": "CCT-1" }, "slack": { "ts": "1" } } });
        apply_slot(&mut meta, "youtrack", Some(json!({ "issue": "CCT-2" })));
        assert_eq!(meta["plugins"]["youtrack"]["issue"], "CCT-2");
        assert_eq!(meta["plugins"]["slack"]["ts"], "1");
    }

    #[test]
    fn clearing_the_last_slot_drops_the_plugins_object() {
        let mut meta = json!({ "plugins": { "youtrack": { "issue": "CCT-1" } } });
        apply_slot(&mut meta, "youtrack", None);
        assert!(meta.get("plugins").is_none());
    }

    #[test]
    fn clearing_one_of_two_keeps_the_object() {
        let mut meta = json!({ "plugins": { "youtrack": {}, "slack": {} } });
        apply_slot(&mut meta, "youtrack", None);
        assert!(meta["plugins"].get("youtrack").is_none());
        assert!(meta["plugins"].get("slack").is_some());
    }

    #[test]
    fn clearing_an_absent_slot_is_a_no_op() {
        let mut meta = json!({ "draft": 1 });
        apply_slot(&mut meta, "youtrack", None);
        assert_eq!(meta, json!({ "draft": 1 }));
    }

    #[test]
    fn a_non_object_metadata_or_plugins_value_is_replaced() {
        let mut meta = json!("junk");
        apply_slot(&mut meta, "youtrack", Some(json!({ "issue": "CCT-1" })));
        assert_eq!(meta["plugins"]["youtrack"]["issue"], "CCT-1");

        let mut meta = json!({ "plugins": 7 });
        apply_slot(&mut meta, "youtrack", Some(json!({ "issue": "CCT-1" })));
        assert_eq!(meta["plugins"]["youtrack"]["issue"], "CCT-1");
    }
}
