//! Per-subsystem daemon bandwidth accounting (CCT-744). Every field is
//! serde-defaulted so an older daemon (missing field) and an older server
//! (unknown field) keep interoperating.

use serde::{Deserialize, Serialize};

/// Cumulative post-compression bytes sent per subsystem since process start.
/// Backfill replays ride [`Self::forward`] — same event-stream path, not
/// separable at the wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BandwidthSummary {
    /// Live tail / event-stream frames written to the WS (post-compression).
    #[serde(default)]
    pub forward: u64,
    /// Chunk bytes re-sent for a resumed transfer after a disconnect.
    #[serde(default)]
    pub retransmit: u64,
    /// Reserved for transcript backfill; currently folded into `forward`.
    #[serde(default)]
    pub backfill: u64,
    /// Self-update download bytes (manifest binary + SHA256SUMS).
    #[serde(default)]
    pub self_update: u64,
    /// Content-addressed blob / image HTTP PUT bodies.
    #[serde(default)]
    pub blob_put: u64,
    /// Heartbeat frames written to the WS.
    #[serde(default)]
    pub heartbeat: u64,
}

impl BandwidthSummary {
    /// Total bytes across every subsystem.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.forward
            .saturating_add(self.retransmit)
            .saturating_add(self.backfill)
            .saturating_add(self.self_update)
            .saturating_add(self.blob_put)
            .saturating_add(self.heartbeat)
    }

    /// Bytes that are expected to produce persisted `stream_events` rows —
    /// the divergence-detection signal. Heartbeat/self-update/blob traffic
    /// legitimately uploads without inserts and must not count.
    #[must_use]
    pub const fn event_bytes(&self) -> u64 {
        self.forward.saturating_add(self.retransmit).saturating_add(self.backfill)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_sums_every_subsystem() {
        let s = BandwidthSummary {
            forward: 1,
            retransmit: 2,
            backfill: 4,
            self_update: 8,
            blob_put: 16,
            heartbeat: 32,
        };
        assert_eq!(s.total(), 63);
    }

    #[test]
    fn legacy_payload_without_fields_defaults_to_zero() {
        let s: BandwidthSummary = serde_json::from_str("{}").unwrap();
        assert_eq!(s, BandwidthSummary::default());
        assert_eq!(s.total(), 0);
    }

    #[test]
    fn roundtrips_over_json() {
        let s = BandwidthSummary { forward: 100, blob_put: 5, ..Default::default() };
        let back: BandwidthSummary =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
    }
}
