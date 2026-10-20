//! Which sessions have a text-delta watcher right now. Adapters ask before
//! forwarding partial assistant text, so an unwatched session costs nothing.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};

use cctui_proto::adapter::AdapterEvent;

static WATCHED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn watched() -> &'static Mutex<HashSet<String>> {
    WATCHED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// `WatchText` for `local_id`: on adds a watcher, off removes it.
pub fn set(local_id: &str, watch: bool) {
    let mut set = watched().lock().unwrap_or_else(PoisonError::into_inner);
    if watch {
        set.insert(local_id.to_owned());
    } else {
        set.remove(local_id);
    }
}

#[must_use]
pub fn wanted(local_id: &str) -> bool {
    watched().lock().unwrap_or_else(PoisonError::into_inner).contains(local_id)
}

/// The event carrying `delta`, or `None` when nobody watches `local_id` or
/// the delta is empty.
#[must_use]
pub fn forward(local_id: &str, item_id: &str, delta: &str) -> Option<AdapterEvent> {
    if delta.is_empty() || !wanted(local_id) {
        return None;
    }
    Some(AdapterEvent::TextDelta {
        local_id: local_id.to_owned(),
        item_id: item_id.to_owned(),
        delta: delta.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delta_is_forwarded_only_while_watched() {
        assert!(forward("gate-a", "m:0", "Hel").is_none());
        set("gate-a", true);
        match forward("gate-a", "m:0", "Hel") {
            Some(AdapterEvent::TextDelta { local_id, item_id, delta }) => {
                assert_eq!(local_id, "gate-a");
                assert_eq!(item_id, "m:0");
                assert_eq!(delta, "Hel");
            }
            other => panic!("expected TextDelta, got {other:?}"),
        }
        assert!(forward("gate-a", "m:0", "").is_none(), "an empty delta is noise");
        assert!(forward("gate-other", "m:0", "Hel").is_none(), "the gate is per session");
        set("gate-a", false);
        assert!(!wanted("gate-a"));
        assert!(forward("gate-a", "m:0", "lo").is_none());
    }
}
