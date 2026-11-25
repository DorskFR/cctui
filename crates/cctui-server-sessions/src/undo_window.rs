//! In memory only: a restart inside the window drops the teardown, and the
//! daemon's reconcile removes the job instead.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use dashmap::DashMap;

use crate::context::SessionsContext;
use crate::events::kind;

/// Longer than the webui's 7 s undo toast.
pub const UNDO_WINDOW: Duration = Duration::from_secs(10);

#[derive(Clone, Default)]
pub struct PendingTeardowns {
    pending: Arc<DashMap<String, u64>>,
    next: Arc<AtomicU64>,
}

impl PendingTeardowns {
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<PendingTeardowns> = OnceLock::new();
        GLOBAL.get_or_init(Self::default)
    }

    /// A new schedule for the same id replaces the old one.
    pub fn schedule<F>(&self, session_id: &str, window: Duration, teardown: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let generation = self.next.fetch_add(1, Ordering::Relaxed);
        self.pending.insert(session_id.to_owned(), generation);
        let pending = Arc::clone(&self.pending);
        let id = session_id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(window).await;
            if pending.remove_if(&id, |_, g| *g == generation).is_some() {
                teardown.await;
            }
        });
    }

    /// Whether a teardown was pending here.
    #[must_use]
    pub fn cancel(&self, session_id: &str) -> bool {
        self.pending.remove(session_id).is_some()
    }

    #[must_use]
    pub fn is_pending(&self, session_id: &str) -> bool {
        self.pending.contains_key(session_id)
    }
}

/// Revoke the live tokens of archived sessions past their undo window.
///
/// A restart inside the window loses the teardown. A row is inside the window
/// while a recent `session.archived` event names it, as subject or descendant;
/// an archive with no event counts as old.
pub async fn revoke_orphaned_tokens<S: SessionsContext>(state: &S) -> usize {
    let ids: Vec<String> = match sqlx::query_scalar(
        "SELECT DISTINCT s.id FROM session_tokens t JOIN sessions s ON s.id = t.session_id \
          WHERE t.revoked_at IS NULL AND s.status = 'archived' \
            AND NOT EXISTS (SELECT 1 FROM events e \
                 WHERE e.kind = $1 AND e.occurred_at > now() - make_interval(secs => $2) \
                   AND (e.session_id = s.id OR e.detail -> 'descendants' ? s.id))",
    )
    .bind(kind::SESSION_ARCHIVED)
    .bind(UNDO_WINDOW.as_secs_f64())
    .fetch_all(state.pool())
    .await
    {
        Ok(ids) => ids,
        Err(err) => {
            tracing::warn!(%err, "orphaned archive token sweep failed");
            return 0;
        }
    };
    let pending = PendingTeardowns::global();
    let mut revoked = 0;
    for id in ids.iter().filter(|id| !pending.is_pending(id)) {
        state.revoke_session_tokens(id).await;
        revoked += 1;
    }
    if revoked > 0 {
        tracing::info!(revoked, "revoked tokens of archived sessions past their undo window");
    }
    revoked
}

/// The descendants archived together with `session_id` by its archive still
/// inside the window, whichever replica holds that window.
pub async fn archived_with<S: SessionsContext>(
    state: &S,
    session_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT jsonb_array_elements_text(COALESCE(detail -> 'descendants', '[]'::jsonb)) \
           FROM (SELECT detail FROM events \
                  WHERE kind = $1 AND session_id = $2 \
                    AND occurred_at > now() - make_interval(secs => $3) \
                  ORDER BY occurred_at DESC, id DESC LIMIT 1) last",
    )
    .bind(kind::SESSION_ARCHIVED)
    .bind(session_id)
    .bind(UNDO_WINDOW.as_secs_f64())
    .fetch_all(state.pool())
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn schedule_send(p: &PendingTeardowns, id: &str, tx: &mpsc::UnboundedSender<String>) {
        let tx = tx.clone();
        let sent = id.to_owned();
        p.schedule(id, UNDO_WINDOW, async move {
            let _ = tx.send(sent);
        });
    }

    #[tokio::test(start_paused = true)]
    async fn an_unarchive_inside_the_window_cancels_the_remove() {
        let p = PendingTeardowns::default();
        let (tx, mut rx) = mpsc::unbounded_channel();
        schedule_send(&p, "s1", &tx);
        tokio::time::sleep(UNDO_WINDOW / 2).await;
        assert!(p.cancel("s1"));
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
        assert!(!p.cancel("s1"), "too late to undo");
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
