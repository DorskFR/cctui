//! Daemon give-up policy (CCT-742): size + attempts caps so one poison event
//! can't wedge the send pipeline. Counters + tombstones persist so a restart
//! mid-transfer doesn't reset the count.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Post-compression wire payloads above this are dropped unsent.
pub const MAX_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

/// A transfer making no forward progress is abandoned after this many attempts.
pub const MAX_ATTEMPTS: u32 = 3;

/// Per-hash send state: the count of no-progress attempts and the highest
/// forward-progress watermark seen, so an attempt that advances it resets the
/// count instead of counting as a failure (CCT-742 §4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Entry {
    attempts: u32,
    progress: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    attempts: HashMap<String, Entry>,
    tombstones: HashSet<String>,
}

/// Persistent per-content-hash give-up tracker, stored at
/// `$XDG_CONFIG_HOME/cctui/send-attempts.json` (sibling of the transcript
/// offsets). Best-effort: a load failure starts from empty state.
#[derive(Debug, Default)]
pub struct SendGuard {
    path: Option<PathBuf>,
    state: State,
}

impl SendGuard {
    #[must_use]
    pub fn open_default() -> Self {
        let path = dirs::config_dir().map(|d| d.join("cctui").join("send-attempts.json"));
        Self::open(path)
    }

    #[must_use]
    pub fn open(path: Option<PathBuf>) -> Self {
        let state = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, state }
    }

    #[must_use]
    pub fn is_tombstoned(&self, hash: &str) -> bool {
        self.state.tombstones.contains(hash)
    }

    /// Record a failed (no full ack) attempt for `hash`, where `progress` is the
    /// number of chunks acked so far. An attempt that advanced `progress` past
    /// the stored watermark resets the counter and is not counted; otherwise the
    /// counter increments. Returns `true` once the transfer must be given up —
    /// either because it is already tombstoned or it just hit [`MAX_ATTEMPTS`],
    /// in which case it is tombstoned and its counter pruned here.
    pub fn note_failed_attempt(&mut self, hash: &str, progress: u64) -> bool {
        if self.is_tombstoned(hash) {
            return true;
        }
        let entry = self.state.attempts.entry(hash.to_owned()).or_default();
        if progress > entry.progress {
            entry.progress = progress;
            entry.attempts = 0;
            return false;
        }
        entry.attempts += 1;
        if entry.attempts >= MAX_ATTEMPTS {
            self.give_up(hash);
            return true;
        }
        false
    }

    /// Abandon `hash` definitively: drop its counter and tombstone it so any
    /// future re-emission of the same payload is skipped immediately.
    pub fn give_up(&mut self, hash: &str) {
        self.state.attempts.remove(hash);
        self.state.tombstones.insert(hash.to_owned());
    }

    /// A transfer completed successfully — prune its counter (keeps the file
    /// small; a success is never tombstoned).
    pub fn complete(&mut self, hash: &str) {
        self.state.attempts.remove(hash);
    }

    /// Persist to disk. Best-effort; a missed write only risks re-counting on
    /// the next restart.
    pub fn flush(&self) {
        let Some(path) = &self.path else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_vec_pretty(&self.state) {
            Ok(bytes) => {
                if let Err(err) = std::fs::write(path, bytes) {
                    tracing::warn!(%err, ?path, "failed to persist send attempts");
                }
            }
            Err(err) => tracing::warn!(%err, "failed to serialise send attempts"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gives_up_after_exactly_three_stuck_attempts() {
        let mut g = SendGuard::open(None);
        // No forward progress across attempts → count 1, 2, then give up on 3.
        assert!(!g.note_failed_attempt("poison", 0));
        assert!(!g.note_failed_attempt("poison", 0));
        assert!(g.note_failed_attempt("poison", 0), "third stuck attempt gives up");
        assert!(g.is_tombstoned("poison"));
        // Once tombstoned it is never retried.
        assert!(g.note_failed_attempt("poison", 0));
    }

    #[test]
    fn forward_progress_resets_the_counter() {
        let mut g = SendGuard::open(None);
        // Two stuck attempts, then one that advances the acked-chunk watermark.
        assert!(!g.note_failed_attempt("t", 0));
        assert!(!g.note_failed_attempt("t", 0));
        assert!(!g.note_failed_attempt("t", 5), "an attempt that made progress resets");
        // Even many further progress-making attempts never give up.
        for chunk in 6..50 {
            assert!(!g.note_failed_attempt("t", chunk));
        }
        assert!(!g.is_tombstoned("t"));
    }

    #[test]
    fn restart_between_attempts_does_not_reset_the_counter() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("send-attempts.json");

        // Two failed attempts, persisted, then the daemon "restarts".
        let mut g1 = SendGuard::open(Some(path.clone()));
        assert!(!g1.note_failed_attempt("h", 0));
        assert!(!g1.note_failed_attempt("h", 0));
        g1.flush();

        // A fresh guard reloads the count; the third attempt still gives up.
        let mut g2 = SendGuard::open(Some(path.clone()));
        assert!(g2.note_failed_attempt("h", 0), "counter survived the restart");
        assert!(g2.is_tombstoned("h"));
        g2.flush();

        // The tombstone itself round-trips too.
        let g3 = SendGuard::open(Some(path));
        assert!(g3.is_tombstoned("h"));
    }

    #[test]
    fn completion_prunes_the_counter_without_tombstoning() {
        let mut g = SendGuard::open(None);
        assert!(!g.note_failed_attempt("ok", 0));
        g.complete("ok");
        assert!(!g.is_tombstoned("ok"));
        // A brand-new run of the same hash starts counting from zero again.
        assert!(!g.note_failed_attempt("ok", 0));
        assert!(!g.note_failed_attempt("ok", 0));
        assert!(g.note_failed_attempt("ok", 0));
    }
}
