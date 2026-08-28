//! Server-side bandwidth anomaly detection.
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
pub const EVICTION_WINDOW: Duration = Duration::from_mins(15);

/// A single chunked transfer inserts nothing until it completes, and the
/// daemon's `SendGuard` caps any transfer at 32 MiB — so more unexplained
/// growth than one maximal in-flight transfer cannot be legitimate.
pub const DIVERGENCE_MIN_BYTES: u64 = 33 * 1024 * 1024;

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

/// A crashlooping daemon reconnects this many times within
/// [`CONNECT_FLAP_WINDOW`]; last-seen liveness stays "online" through the
/// whole loop, so connect churn is the only signal.
pub const CONNECT_FLAP_THRESHOLD: usize = 5;
pub const CONNECT_FLAP_WINDOW: Duration = Duration::from_mins(10);
pub const CONNECT_NOTIFY_COOLDOWN: Duration = Duration::from_hours(1);

#[derive(Default)]
struct ConnectState {
    connects: VecDeque<Instant>,
    notified_at: Option<Instant>,
}

pub struct ConnectFlap {
    /// Connects within the window, threshold included.
    pub connects: usize,
    /// Whether the caller should push a notification (at most one per machine
    /// per [`CONNECT_NOTIFY_COOLDOWN`]).
    pub notify: bool,
}

/// Rolling per-machine daemon-WS connect timestamps — the crashloop detector.
#[derive(Default)]
pub struct ConnectTracker {
    inner: Mutex<HashMap<Uuid, ConnectState>>,
}

impl ConnectTracker {
    /// Record a daemon WS connect now; `Some` once the machine is flapping.
    pub fn record(&self, machine_id: Uuid) -> Option<ConnectFlap> {
        self.record_at(machine_id, Instant::now())
    }

    fn record_at(&self, machine_id: Uuid, now: Instant) -> Option<ConnectFlap> {
        let mut map = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let st = map.entry(machine_id).or_default();
        st.connects.push_back(now);
        while st.connects.front().is_some_and(|&t| now.duration_since(t) > CONNECT_FLAP_WINDOW) {
            st.connects.pop_front();
        }
        let connects = st.connects.len();
        let out = if connects < CONNECT_FLAP_THRESHOLD {
            None
        } else {
            let notify =
                st.notified_at.is_none_or(|t| now.duration_since(t) >= CONNECT_NOTIFY_COOLDOWN);
            if notify {
                st.notified_at = Some(now);
            }
            Some(ConnectFlap { connects, notify })
        };
        drop(map);
        out
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
    /// Record this heartbeat's cumulative event-carrying `upload_bytes` and the
    /// machine's persisted `insert_count`. The baseline only advances when
    /// inserts advance (or uploads reset lower, i.e. daemon restart), so slow
    /// leaks accumulate against it; `Some` requires `DIVERGENCE_MIN_BYTES` of
    /// growth with zero new inserts.
    pub fn observe(
        &self,
        machine_id: Uuid,
        upload_bytes: u64,
        insert_count: u64,
    ) -> Option<Divergence> {
        let mut map = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let out = match map.get_mut(&machine_id) {
            None => {
                map.insert(machine_id, Observation { upload_bytes, insert_count });
                None
            }
            Some(p) if insert_count > p.insert_count || upload_bytes < p.upload_bytes => {
                *p = Observation { upload_bytes, insert_count };
                None
            }
            Some(p) if upload_bytes.saturating_sub(p.upload_bytes) < DIVERGENCE_MIN_BYTES => None,
            Some(p) => {
                let d =
                    Divergence { prev_upload_bytes: p.upload_bytes, upload_bytes, insert_count };
                *p = Observation { upload_bytes, insert_count };
                Some(d)
            }
        };
        drop(map);
        out
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
        let n = t.record_at(m, base + EVICTION_WINDOW + Duration::from_mins(1));
        assert_eq!(n, 1);
    }

    #[test]
    fn connect_flap_fires_at_the_threshold_and_notifies_once() {
        let t = ConnectTracker::default();
        let m = Uuid::new_v4();
        let base = Instant::now();
        for i in 0..CONNECT_FLAP_THRESHOLD - 1 {
            assert!(t.record_at(m, base + Duration::from_secs(i as u64)).is_none());
        }
        let flap = t
            .record_at(m, base + Duration::from_secs(CONNECT_FLAP_THRESHOLD as u64))
            .expect("threshold connect flags the flap");
        assert_eq!(flap.connects, CONNECT_FLAP_THRESHOLD);
        assert!(flap.notify, "first flap notifies");
        let again = t
            .record_at(m, base + Duration::from_secs(CONNECT_FLAP_THRESHOLD as u64 + 5))
            .expect("still flapping");
        assert!(!again.notify, "within the cooldown no second notification");
    }

    #[test]
    fn connect_flap_notifies_again_after_the_cooldown() {
        let t = ConnectTracker::default();
        let m = Uuid::new_v4();
        let base = Instant::now();
        for i in 0..CONNECT_FLAP_THRESHOLD {
            t.record_at(m, base + Duration::from_secs(i as u64));
        }
        let later = base + CONNECT_NOTIFY_COOLDOWN + Duration::from_secs(1);
        for i in 0..CONNECT_FLAP_THRESHOLD - 1 {
            t.record_at(m, later + Duration::from_secs(i as u64));
        }
        let flap = t
            .record_at(m, later + Duration::from_secs(CONNECT_FLAP_THRESHOLD as u64))
            .expect("flapping again");
        assert!(flap.notify, "cooldown elapsed → notify again");
    }

    #[test]
    fn connects_outside_the_window_are_pruned() {
        let t = ConnectTracker::default();
        let m = Uuid::new_v4();
        let base = Instant::now();
        for i in 0..CONNECT_FLAP_THRESHOLD - 1 {
            t.record_at(m, base + Duration::from_secs(i as u64));
        }
        assert!(
            t.record_at(m, base + CONNECT_FLAP_WINDOW + Duration::from_mins(1)).is_none(),
            "old connects aged out — a lone reconnect is not a flap"
        );
    }

    #[test]
    fn divergence_fires_when_bytes_grow_without_inserts() {
        let t = DivergenceTracker::default();
        let m = Uuid::new_v4();
        assert!(t.observe(m, 1_000, 10).is_none(), "first observation cannot diverge");
        let grown = 1_000 + DIVERGENCE_MIN_BYTES;
        let d = t.observe(m, grown, 10).expect("uploads grew past floor, inserts flat");
        assert_eq!(d.prev_upload_bytes, 1_000);
        assert_eq!(d.upload_bytes, grown);
    }

    #[test]
    fn divergence_quiet_below_floor_but_accumulates_against_baseline() {
        let t = DivergenceTracker::default();
        let m = Uuid::new_v4();
        assert!(t.observe(m, 1_000, 10).is_none());
        for i in 1..=10 {
            assert!(t.observe(m, 1_000 + i * 200, 10).is_none(), "heartbeat-sized growth is quiet");
        }
        let d = t.observe(m, 1_000 + DIVERGENCE_MIN_BYTES, 10).expect("slow leak accumulates");
        assert_eq!(d.prev_upload_bytes, 1_000, "baseline held until the floor was crossed");
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
