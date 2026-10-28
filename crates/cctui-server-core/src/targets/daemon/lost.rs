//! Reconnect grace and per-connection session-loss scheduling.
use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use uuid::Uuid;

/// One closed connection's deferred mark: the sessions it announced that no
/// connection has announced since.
struct PendingMark {
    machine_id: Uuid,
    sessions: HashSet<String>,
    task: tokio::task::AbortHandle,
}

/// Deferred `daemon_lost` marks, one per closed connection.
///
/// Every dispatched
/// worker pod shares one machine id, so a mark is only ever cancelled for the
/// sessions a later connection announces, or for the whole machine when it is
/// not shared.
#[derive(Default)]
pub struct PendingDaemonLost {
    marks: dashmap::DashMap<Uuid, PendingMark>,
}

impl PendingDaemonLost {
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    pub fn schedule<F, Fut>(
        self: &Arc<Self>,
        conn_id: Uuid,
        machine_id: Uuid,
        sessions: Vec<String>,
        delay: Duration,
        mark: F,
    ) where
        F: FnOnce(Vec<String>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let this = Arc::clone(self);
        let task = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let Some((_, pending)) = this.marks.remove(&conn_id) else { return };
            mark(pending.sessions.into_iter().collect()).await;
        });
        let sessions = sessions.into_iter().collect();
        let pending = PendingMark { machine_id, sessions, task: task.abort_handle() };
        if let Some(superseded) = self.marks.insert(conn_id, pending) {
            superseded.task.abort();
        }
    }

    /// A reconnect of a machine only one daemon uses: its marks are moot.
    pub fn cancel_machine(&self, machine_id: Uuid) {
        self.marks.retain(|_, pending| {
            let keep = pending.machine_id != machine_id;
            if !keep {
                pending.task.abort();
            }
            keep
        });
    }

    /// `session_id` was announced again, so it is alive on some connection.
    pub fn announced(&self, session_id: &str) {
        self.marks.retain(|_, pending| {
            pending.sessions.remove(session_id);
            let keep = !pending.sessions.is_empty();
            if !keep {
                pending.task.abort();
            }
            keep
        });
    }
}

pub static PENDING_DAEMON_LOST: LazyLock<Arc<PendingDaemonLost>> =
    LazyLock::new(|| Arc::new(PendingDaemonLost::default()));
