//! Server-side bandwidth anomaly detection (CCT-744).
//!
//! In-memory, alerting-not-audit: an eviction-rate tracker escalates a machine's
//! repeated WS evictions to an ERROR log (glitchtip picks up ERROR level), and a
//! divergence tracker fires when a machine's reported upload bytes grow while its
//! persisted `stream_events` inserts do not — the 2026-07-21 failure signature.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uuid::Uuid;

/// Escalate to ERROR once a machine hits this many evictions within
/// [`EVICTION_WINDOW`].
pub const EVICTION_THRESHOLD: usize = 5;
pub const EVICTION_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Rolling per-machine eviction timestamps within [`EVICTION_WINDOW`].
#[derive(Default)]
pub struct EvictionTracker {
    inner: Mutex<HashMap<Uuid, VecDeque<Instant>>>,
}

impl EvictionTracker {
    /// Record an eviction now; returns the count within the window.
    pub fn record(&self, machine_id: Uuid) -> usize {
        self.record_at(machine_id, Instant::now())
    }

    fn record_at(&self, machine_id: Uuid, now: Instant) -> usize {
        let mut map = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dq = map.entry(machine_id).or_default();
        dq.push_back(now);
        while dq.front().is_some_and(|&t| now.duration_since(t) > EVICTION_WINDOW) {
            dq.pop_front();
        }
        let len = dq.len();
        drop(map);
        len
    }
}

/// The last upload/insert observation for one machine.
#[derive(Clone, Copy)]
struct Observation {
    upload_bytes: u64,
    insert_count: u64,
}

/// Upload/insert divergence: uploads climbing with no matching inserts.
pub struct Divergence {
    pub prev_upload_bytes: u64,
    pub upload_bytes: u64,
    pub insert_count: u64,
}

/// Per-machine last-seen upload total + persisted insert count, so a heartbeat
/// can flag uploads that grow without matching inserts.
#[derive(Default)]
pub struct DivergenceTracker {
    inner: Mutex<HashMap<Uuid, Observation>>,
}

impl DivergenceTracker {
    /// Record this heartbeat's cumulative `upload_bytes` and the machine's
    /// current persisted `insert_count`. Returns `Some` when uploads grew since
    /// the last observation while inserts did not — a daemon restart (uploads
    /// reset lower) never fires.
    pub fn observe(
        &self,
        machine_id: Uuid,
        upload_bytes: u64,
        insert_count: u64,
    ) -> Option<Divergence> {
        let mut map = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev = map.insert(machine_id, Observation { upload_bytes, insert_count });
        drop(map);
        prev.filter(|p| upload_bytes > p.upload_bytes && insert_count <= p.insert_count)
            .map(|p| Divergence { prev_upload_bytes: p.upload_bytes, upload_bytes, insert_count })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_rate_escalates_at_the_threshold() {
        let t = EvictionTracker::default();
        let m = Uuid::new_v4();
        let base = Instant::now();
        for i in 1..EVICTION_THRESHOLD {
            let n = t.record_at(m, base + Duration::from_secs(i as u64));
            assert!(n < EVICTION_THRESHOLD, "below threshold stays quiet: {n}");
        }
        let n = t.record_at(m, base + Duration::from_secs(EVICTION_THRESHOLD as u64));
        assert_eq!(n, EVICTION_THRESHOLD, "fifth eviction in the window escalates");
    }

    #[test]
    fn evictions_outside_the_window_are_pruned() {
        let t = EvictionTracker::default();
        let m = Uuid::new_v4();
        let base = Instant::now();
        for i in 0..4 {
            t.record_at(m, base + Duration::from_secs(i));
        }
        // Well past the window: the old four have aged out, this is a lone event.
        let n = t.record_at(m, base + EVICTION_WINDOW + Duration::from_secs(60));
        assert_eq!(n, 1);
    }

    #[test]
    fn divergence_fires_when_bytes_grow_without_inserts() {
        let t = DivergenceTracker::default();
        let m = Uuid::new_v4();
        assert!(t.observe(m, 1_000, 10).is_none(), "first observation cannot diverge");
        let d = t.observe(m, 2_000, 10).expect("uploads grew, inserts flat → divergence");
        assert_eq!(d.prev_upload_bytes, 1_000);
        assert_eq!(d.upload_bytes, 2_000);
    }

    #[test]
    fn divergence_quiet_when_inserts_grow_too() {
        let t = DivergenceTracker::default();
        let m = Uuid::new_v4();
        assert!(t.observe(m, 1_000, 10).is_none());
        assert!(t.observe(m, 2_000, 25).is_none(), "inserts advanced → healthy, no alert");
    }

    #[test]
    fn divergence_quiet_after_daemon_restart_resets_bytes() {
        let t = DivergenceTracker::default();
        let m = Uuid::new_v4();
        assert!(t.observe(m, 5_000, 10).is_none());
        assert!(t.observe(m, 100, 10).is_none(), "a lower total is a restart, not a leak");
    }

    #[test]
    fn migration_075_creates_and_drops_the_table() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let up = std::fs::read_to_string(format!("{dir}/075_machine_bandwidth.up.sql")).unwrap();
        let down =
            std::fs::read_to_string(format!("{dir}/075_machine_bandwidth.down.sql")).unwrap();
        assert!(up.contains("CREATE TABLE machine_bandwidth"), "up must create the table");
        assert!(up.contains("REFERENCES machines(id)"), "up must FK to machines");
        assert!(down.contains("DROP TABLE machine_bandwidth"), "down must drop the table");
    }
}
