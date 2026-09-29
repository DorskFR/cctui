//! The shared `local_id → worker short` roster.
//!
//! Built from `list` snapshots by the control driver and read concurrently by
//! the pty-watch pump, which must resolve a short without waiting for the
//! serial command loop. `changed` lets a watch for a not-yet-rostered session
//! park until the next snapshot lists it.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use tokio::sync::Notify;

#[derive(Clone, Default)]
pub(super) struct SessionRoster {
    map: Arc<RwLock<HashMap<String, String>>>,
    changed: Arc<Notify>,
}

impl SessionRoster {
    pub(super) fn insert(&self, local_id: String, short: String) {
        if let Ok(mut map) = self.map.write() {
            map.insert(local_id, short);
        }
        // `notify_one` (not `notify_waiters`): it stores a permit, so an insert
        // racing just ahead of the pump's `changed().await` still wakes it.
        self.changed.notify_one();
    }

    pub(super) fn remove(&self, local_id: &str) {
        if let Ok(mut map) = self.map.write() {
            map.remove(local_id);
        }
    }

    pub(super) fn get(&self, local_id: &str) -> Option<String> {
        self.map.read().ok()?.get(local_id).cloned()
    }

    pub(super) fn entries(&self) -> Vec<(String, String)> {
        self.map.read().map_or_else(
            |_| Vec::new(),
            |map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        )
    }

    pub(super) async fn changed(&self) {
        self.changed.notified().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn insert_is_visible_to_every_clone_and_remove_clears_it() {
        let roster = SessionRoster::default();
        let mirror = roster.clone();
        roster.insert("sess-1".to_owned(), "deadbeef".to_owned());
        assert_eq!(mirror.get("sess-1").as_deref(), Some("deadbeef"));
        assert_eq!(mirror.entries(), vec![("sess-1".to_owned(), "deadbeef".to_owned())]);
        roster.remove("sess-1");
        assert!(mirror.get("sess-1").is_none());
    }

    /// An insert that lands before anyone awaits `changed` must still wake the
    /// next waiter — otherwise a pending watch parks forever.
    #[tokio::test]
    async fn an_insert_before_the_await_still_wakes_the_waiter() {
        let roster = SessionRoster::default();
        roster.insert("sess-1".to_owned(), "deadbeef".to_owned());
        tokio::time::timeout(std::time::Duration::from_millis(500), roster.changed())
            .await
            .expect("a stored permit must wake the first waiter");
    }
}
