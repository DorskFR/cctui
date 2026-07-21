//! Shared per-subsystem bandwidth counters (CCT-744).
//!
//! Cheap atomics threaded to the send paths so a runaway upload loop becomes
//! attributable. A [`BandwidthCounters`] handle is cloned (Arc) into the WS
//! send loop and the HTTP client; the running daemon persists a
//! [`BandwidthSnapshot`] to a state file so a separate `cctui-daemon status`
//! process can read it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cctui_proto::bandwidth::BandwidthSummary;
use serde::{Deserialize, Serialize};

use crate::runtime;

const FILE_NAME: &str = "daemon-bandwidth.json";
const WINDOW_MINUTES: usize = 60;

/// Which send path a byte count belongs to.
#[derive(Debug, Clone, Copy)]
pub enum Subsystem {
    Forward,
    Retransmit,
    Backfill,
    SelfUpdate,
    BlobPut,
    Heartbeat,
}

#[derive(Debug, Default)]
struct Totals {
    forward: AtomicU64,
    retransmit: AtomicU64,
    backfill: AtomicU64,
    self_update: AtomicU64,
    blob_put: AtomicU64,
    heartbeat: AtomicU64,
}

/// A fixed-size ring of one-minute buckets giving a rolling last-hour total at
/// O(1) per add — no unbounded queue.
#[derive(Debug)]
struct HourWindow {
    start: Instant,
    buckets: [u64; WINDOW_MINUTES],
    head_minute: u64,
}

impl Default for HourWindow {
    fn default() -> Self {
        Self { start: Instant::now(), buckets: [0; WINDOW_MINUTES], head_minute: 0 }
    }
}

impl HourWindow {
    fn minute(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.start).as_secs() / 60
    }

    fn advance(&mut self, minute: u64) {
        if minute <= self.head_minute {
            return;
        }
        let gap = (minute - self.head_minute).min(WINDOW_MINUTES as u64);
        for i in 1..=gap {
            let idx = ((self.head_minute + i) % WINDOW_MINUTES as u64) as usize;
            self.buckets[idx] = 0;
        }
        self.head_minute = minute;
    }

    fn add(&mut self, now: Instant, bytes: u64) {
        let m = self.minute(now);
        self.advance(m);
        let idx = (m % WINDOW_MINUTES as u64) as usize;
        self.buckets[idx] = self.buckets[idx].saturating_add(bytes);
    }

    fn total(&mut self, now: Instant) -> u64 {
        self.advance(self.minute(now));
        self.buckets.iter().sum()
    }
}

#[derive(Debug, Default)]
struct Inner {
    totals: Totals,
    window: Mutex<HourWindow>,
}

/// Cloneable handle over the shared counters.
#[derive(Debug, Clone, Default)]
pub struct BandwidthCounters {
    inner: Arc<Inner>,
}

impl BandwidthCounters {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `bytes` written by `subsystem`.
    pub fn add(&self, subsystem: Subsystem, bytes: u64) {
        let slot = match subsystem {
            Subsystem::Forward => &self.inner.totals.forward,
            Subsystem::Retransmit => &self.inner.totals.retransmit,
            Subsystem::Backfill => &self.inner.totals.backfill,
            Subsystem::SelfUpdate => &self.inner.totals.self_update,
            Subsystem::BlobPut => &self.inner.totals.blob_put,
            Subsystem::Heartbeat => &self.inner.totals.heartbeat,
        };
        slot.fetch_add(bytes, Ordering::Relaxed);
        self.inner
            .window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .add(Instant::now(), bytes);
    }

    /// Since-start totals per subsystem.
    #[must_use]
    pub fn summary(&self) -> BandwidthSummary {
        let t = &self.inner.totals;
        BandwidthSummary {
            forward: t.forward.load(Ordering::Relaxed),
            retransmit: t.retransmit.load(Ordering::Relaxed),
            backfill: t.backfill.load(Ordering::Relaxed),
            self_update: t.self_update.load(Ordering::Relaxed),
            blob_put: t.blob_put.load(Ordering::Relaxed),
            heartbeat: t.heartbeat.load(Ordering::Relaxed),
        }
    }

    /// Rolling total over the last hour.
    #[must_use]
    pub fn last_hour(&self) -> u64 {
        self.inner
            .window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .total(Instant::now())
    }

    #[must_use]
    pub fn snapshot(&self) -> BandwidthSnapshot {
        BandwidthSnapshot {
            summary: self.summary(),
            last_hour_bytes: self.last_hour(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Best-effort write of the current snapshot to the state file so a separate
    /// `status` process can read it.
    pub fn persist(&self) {
        let snap = self.snapshot();
        let Ok(json) = serde_json::to_string_pretty(&snap) else { return };
        if runtime::record_at(&runtime::state_candidates(FILE_NAME), &json).is_none() {
            tracing::debug!("failed to persist bandwidth snapshot to any candidate location");
        }
    }
}

/// The persisted form the `status` command renders.
#[derive(Debug, Serialize, Deserialize)]
pub struct BandwidthSnapshot {
    pub summary: BandwidthSummary,
    pub last_hour_bytes: u64,
    pub updated_at: String,
}

impl BandwidthSnapshot {
    /// Human-readable status lines (one per subsystem plus totals).
    #[must_use]
    pub fn render(&self) -> String {
        let s = &self.summary;
        format!(
            "bandwidth (since start): forward={} retransmit={} backfill={} \
             self_update={} blob_put={} heartbeat={} total={}\n\
             bandwidth (last hour): {}\nbandwidth updated_at: {}",
            s.forward,
            s.retransmit,
            s.backfill,
            s.self_update,
            s.blob_put,
            s.heartbeat,
            s.total(),
            self.last_hour_bytes,
            self.updated_at,
        )
    }
}

/// Read the persisted snapshot, if the daemon has written one.
#[must_use]
pub fn read_snapshot() -> Option<BandwidthSnapshot> {
    runtime::state_candidates(FILE_NAME)
        .iter()
        .find_map(|p| serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_accumulates_per_subsystem_and_total() {
        let c = BandwidthCounters::new();
        c.add(Subsystem::Forward, 100);
        c.add(Subsystem::Forward, 50);
        c.add(Subsystem::Retransmit, 10);
        c.add(Subsystem::BlobPut, 7);
        let s = c.summary();
        assert_eq!(s.forward, 150);
        assert_eq!(s.retransmit, 10);
        assert_eq!(s.blob_put, 7);
        assert_eq!(s.total(), 167);
    }

    #[test]
    fn hour_window_expires_stale_buckets() {
        let mut w = HourWindow::default();
        let base = w.start;
        w.add(base, 100);
        assert_eq!(w.total(base), 100);
        let t45 = base + std::time::Duration::from_secs(45 * 60);
        w.add(t45, 50);
        assert_eq!(w.total(t45), 150);
        // 90 minutes on: the minute-0 bucket has rolled off, minute-45 survives.
        let t90 = base + std::time::Duration::from_secs(90 * 60);
        assert_eq!(w.total(t90), 50);
    }

    #[test]
    fn snapshot_render_includes_every_subsystem() {
        let c = BandwidthCounters::new();
        c.add(Subsystem::Forward, 900);
        c.add(Subsystem::BlobPut, 42);
        let rendered = c.snapshot().render();
        assert!(rendered.contains("forward=900"), "{rendered}");
        assert!(rendered.contains("blob_put=42"), "{rendered}");
        assert!(rendered.contains("total=942"), "{rendered}");
    }
}
