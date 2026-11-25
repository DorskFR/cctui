//! In memory only: a restart inside the window drops the teardown, and the
//! daemon's reconcile removes the job instead.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use dashmap::DashMap;

/// Longer than the webui's 7 s undo toast.
pub const UNDO_WINDOW: Duration = Duration::from_secs(10);

#[derive(Clone, Default)]
pub struct PendingTeardowns {
    pending: Arc<DashMap<String, Pending>>,
    next: Arc<AtomicU64>,
}

struct Pending {
    generation: u64,
    descendants: Vec<String>,
}

impl PendingTeardowns {
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<PendingTeardowns> = OnceLock::new();
        GLOBAL.get_or_init(Self::default)
    }

    /// A new schedule for the same id replaces the old one.
    pub fn schedule<F>(
        &self,
        session_id: &str,
        descendants: Vec<String>,
        window: Duration,
        teardown: F,
    ) where
        F: Future<Output = ()> + Send + 'static,
    {
        let generation = self.next.fetch_add(1, Ordering::Relaxed);
        self.pending.insert(session_id.to_owned(), Pending { generation, descendants });
        let pending = Arc::clone(&self.pending);
        let id = session_id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(window).await;
            if pending.remove_if(&id, |_, p| p.generation == generation).is_some() {
                teardown.await;
            }
        });
    }

    /// Drop the pending teardown of `session_id`; the descendants it would
    /// have torn down along with it, or `None` when nothing was pending.
    #[must_use]
    pub fn cancel(&self, session_id: &str) -> Option<Vec<String>> {
        self.pending.remove(session_id).map(|(_, p)| p.descendants)
    }

    #[must_use]
    pub fn is_pending(&self, session_id: &str) -> bool {
        self.pending.contains_key(session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn schedule_send(p: &PendingTeardowns, id: &str, tx: &mpsc::UnboundedSender<String>) {
        let tx = tx.clone();
        let sent = id.to_owned();
        p.schedule(id, vec!["child".into()], UNDO_WINDOW, async move {
            let _ = tx.send(sent);
        });
    }

    #[tokio::test(start_paused = true)]
    async fn an_unarchive_inside_the_window_cancels_the_remove() {
        let p = PendingTeardowns::default();
        let (tx, mut rx) = mpsc::unbounded_channel();
        schedule_send(&p, "s1", &tx);
        tokio::time::sleep(UNDO_WINDOW / 2).await;
        assert_eq!(p.cancel("s1"), Some(vec!["child".to_owned()]));
        tokio::time::sleep(UNDO_WINDOW * 2).await;
        assert!(rx.try_recv().is_err(), "a cancelled teardown must not run");
        assert!(!p.is_pending("s1"));
    }

    #[tokio::test(start_paused = true)]
    async fn the_remove_runs_once_after_the_window() {
        let p = PendingTeardowns::default();
        let (tx, mut rx) = mpsc::unbounded_channel();
        schedule_send(&p, "s1", &tx);
        tokio::time::sleep(UNDO_WINDOW.saturating_sub(Duration::from_millis(1))).await;
        assert!(rx.try_recv().is_err(), "nothing runs inside the window");
        assert!(p.is_pending("s1"));
        tokio::time::sleep(Duration::from_millis(2)).await;
        assert_eq!(rx.recv().await.as_deref(), Some("s1"));
        tokio::time::sleep(UNDO_WINDOW * 2).await;
        assert!(rx.try_recv().is_err(), "the teardown runs exactly once");
        assert!(!p.is_pending("s1"));
        assert_eq!(p.cancel("s1"), None, "too late to undo");
    }

    #[tokio::test(start_paused = true)]
    async fn a_rearchive_restarts_the_window() {
        let p = PendingTeardowns::default();
        let (tx, mut rx) = mpsc::unbounded_channel();
        schedule_send(&p, "s1", &tx);
        tokio::time::sleep(UNDO_WINDOW / 2).await;
        schedule_send(&p, "s1", &tx);
        tokio::time::sleep(UNDO_WINDOW / 2 + Duration::from_millis(1)).await;
        assert!(rx.try_recv().is_err(), "the first schedule was replaced");
        tokio::time::sleep(UNDO_WINDOW / 2).await;
        assert_eq!(rx.recv().await.as_deref(), Some("s1"));
        assert!(rx.try_recv().is_err());
    }
}
