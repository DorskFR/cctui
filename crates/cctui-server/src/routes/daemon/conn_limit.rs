use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use uuid::Uuid;

/// A connection this old may be displaced by a newer one from the same key.
/// Under the daemon read timeout, so a reconnecting daemon never waits on
/// liveness to reap its own half-open socket; above any legitimate reconnect
/// overlap, so a client opening connections in a tight loop — whose sockets are
/// all young — is refused rather than served.
const EVICTABLE_AFTER: Duration = Duration::from_secs(30);

struct Slot {
    seq: u64,
    since: Instant,
    evict: Arc<Notify>,
}

/// Per-machine-key live connection counts for the daemon WS.
#[derive(Default)]
pub(super) struct DaemonConnLimit {
    keys: Mutex<HashMap<Uuid, Vec<Slot>>>,
    seq: AtomicU64,
}

/// Holds one key's admission for as long as it lives; the count is released on
/// drop, so every disconnect path — close frame, read timeout, task abort,
/// failed handshake — releases it.
pub(super) struct ConnGuard {
    limit: Arc<DaemonConnLimit>,
    key_id: Uuid,
    seq: u64,
    evict: Arc<Notify>,
}

impl ConnGuard {
    /// A guard that counts against nothing and is never displaced.
    pub(super) fn exempt() -> Self {
        Self {
            limit: Arc::new(DaemonConnLimit::default()),
            key_id: Uuid::nil(),
            seq: 0,
            evict: Arc::new(Notify::new()),
        }
    }

    /// Resolves once this connection has been displaced by a newer one from the
    /// same key.
    pub(super) async fn evicted(&self) {
        self.evict.notified().await;
    }
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        if let Ok(mut keys) = self.limit.keys.lock()
            && let Some(slots) = keys.get_mut(&self.key_id)
        {
            slots.retain(|s| s.seq != self.seq);
            if slots.is_empty() {
                keys.remove(&self.key_id);
            }
        }
    }
}

impl DaemonConnLimit {
    /// Admit one connection for `key_id`. At the cap the oldest connection is
    /// displaced if it is old enough to be a leftover; when every live one is
    /// young, the upgrade is refused.
    pub(super) fn acquire(
        self: &Arc<Self>,
        key_id: Uuid,
        cap: usize,
        now: Instant,
    ) -> Option<ConnGuard> {
        let cap = cap.max(1);
        let mut keys = self.keys.lock().ok()?;
        let slots = keys.entry(key_id).or_default();
        if slots.len() >= cap {
            let oldest = slots
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.since)
                .filter(|(_, s)| now.saturating_duration_since(s.since) >= EVICTABLE_AFTER)
                .map(|(i, _)| i)?;
            slots.remove(oldest).evict.notify_one();
        }
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let evict = Arc::new(Notify::new());
        slots.push(Slot { seq, since: now, evict: Arc::clone(&evict) });
        drop(keys);
        Some(ConnGuard { limit: Arc::clone(self), key_id, seq, evict })
    }

    #[cfg(test)]
    fn live(&self, key_id: Uuid) -> usize {
        self.keys.lock().unwrap().get(&key_id).map_or(0, Vec::len)
    }

    #[cfg(test)]
    fn keys(&self) -> usize {
        self.keys.lock().unwrap().len()
    }
}

pub(super) static DAEMON_CONNS: LazyLock<Arc<DaemonConnLimit>> =
    LazyLock::new(|| Arc::new(DaemonConnLimit::default()));

#[cfg(test)]
mod tests {
    use super::*;

    fn limit() -> Arc<DaemonConnLimit> {
        Arc::new(DaemonConnLimit::default())
    }

    #[test]
    fn admits_up_to_the_cap() {
        let (limit, key, now) = (limit(), Uuid::new_v4(), Instant::now());
        let held =
            [limit.acquire(key, 3, now), limit.acquire(key, 3, now), limit.acquire(key, 3, now)];
        assert!(held.iter().all(Option::is_some));
        assert_eq!(limit.live(key), 3);
    }

    #[test]
    fn refuses_over_the_cap_while_every_connection_is_young() {
        let (limit, key, now) = (limit(), Uuid::new_v4(), Instant::now());
        let _held: Vec<ConnGuard> = (0..2).filter_map(|_| limit.acquire(key, 2, now)).collect();
        assert!(limit.acquire(key, 2, now).is_none());
        assert_eq!(limit.live(key), 2);
    }

    #[test]
    fn a_dropped_guard_releases_its_slot() {
        let (limit, key, now) = (limit(), Uuid::new_v4(), Instant::now());
        let first = limit.acquire(key, 1, now).unwrap();
        assert!(limit.acquire(key, 1, now).is_none());
        drop(first);
        assert_eq!(limit.live(key), 0);
        assert_eq!(limit.keys(), 0);
        assert!(limit.acquire(key, 1, now).is_some());
    }

    #[test]
    fn keys_are_counted_separately() {
        let (limit, now) = (limit(), Instant::now());
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let _first = limit.acquire(a, 1, now).unwrap();
        assert!(limit.acquire(b, 1, now).is_some());
        assert_eq!(limit.live(a), 1);
        assert_eq!(limit.live(b), 1);
    }

    #[tokio::test]
    async fn a_reconnect_displaces_a_leftover_connection() {
        let (limit, key, start) = (limit(), Uuid::new_v4(), Instant::now());
        let stale = limit.acquire(key, 1, start).unwrap();
        let later = start + EVICTABLE_AFTER;
        let fresh = limit.acquire(key, 1, later);
        assert!(fresh.is_some(), "a reconnecting daemon is never locked out");
        assert_eq!(limit.live(key), 1);
        stale.evicted().await;
        drop(stale);
        assert_eq!(limit.live(key), 1, "the displaced guard releases nothing it no longer owns");
    }
}
