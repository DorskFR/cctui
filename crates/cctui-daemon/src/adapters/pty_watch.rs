//! The watch bookkeeping shared by the claude-code and codex live views: one
//! cancellable viewer task per key, started and stopped by `WatchPty`.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub(crate) struct PtyWatchSet {
    tasks: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl PtyWatchSet {
    /// Spawn `task` under a child of `shutdown` unless `key` is already
    /// watched. Returns whether a task was started.
    pub(crate) fn watch<F, Fut>(&self, key: String, shutdown: &CancellationToken, task: F) -> bool
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let Ok(mut tasks) = self.tasks.lock() else { return false };
        if tasks.contains_key(&key) {
            return false;
        }
        let cancel = shutdown.child_token();
        tokio::spawn(task(cancel.clone()));
        tasks.insert(key, cancel);
        true
    }

    pub(crate) fn unwatch(&self, key: &str) {
        if let Ok(mut tasks) = self.tasks.lock()
            && let Some(cancel) = tasks.remove(key)
        {
            cancel.cancel();
        }
    }

    pub(crate) fn watching(&self) -> usize {
        self.tasks.lock().map_or(0, |tasks| tasks.len())
    }

    #[cfg(test)]
    pub(crate) fn token(&self, key: &str) -> Option<CancellationToken> {
        self.tasks.lock().ok()?.get(key).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn watch_is_idempotent_per_key_and_unwatch_cancels() {
        let set = PtyWatchSet::default();
        let shutdown = CancellationToken::new();
        assert!(set.watch("a".to_owned(), &shutdown, |cancel| async move {
            cancel.cancelled().await;
        }));
        assert!(!set.watch("a".to_owned(), &shutdown, |_| async {}));
        assert_eq!(set.watching(), 1);

        let token = set.token("a").unwrap();
        set.unwatch("a");
        assert!(token.is_cancelled());
        assert_eq!(set.watching(), 0);
        set.unwatch("a");
    }

    #[tokio::test]
    async fn shutdown_cancels_every_watch() {
        let set = PtyWatchSet::default();
        let shutdown = CancellationToken::new();
        set.watch("a".to_owned(), &shutdown, |_| async {});
        set.watch("b".to_owned(), &shutdown, |_| async {});
        let (a, b) = (set.token("a").unwrap(), set.token("b").unwrap());
        shutdown.cancel();
        assert!(a.is_cancelled() && b.is_cancelled());
    }
}
