//! A suspended Job keeps its server registration alive while it is queued.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use uuid::Uuid;

/// Well under the server's registration TTL, so a held Job never lapses.
const HOLD_EVERY: Duration = Duration::from_hours(1);
/// A failed hold is retried well before the worker key it protects can expire.
const HOLD_RETRY: Duration = Duration::from_mins(2);

/// When each queued dispatch is next due a hold.
#[derive(Default)]
pub struct QueueHolds {
    next: BTreeMap<Uuid, Instant>,
}

impl QueueHolds {
    /// The `queued` dispatches due a hold at `now`.
    ///
    /// A dispatch no longer queued is forgotten; one seen for the first time is
    /// due at once.
    pub fn due(&mut self, queued: impl IntoIterator<Item = Uuid>, now: Instant) -> Vec<Uuid> {
        let queued: BTreeSet<Uuid> = queued.into_iter().collect();
        self.next.retain(|id, _| queued.contains(id));
        queued.into_iter().filter(|id| self.next.get(id).is_none_or(|at| now >= *at)).collect()
    }

    pub fn held(&mut self, id: Uuid, now: Instant) {
        self.next.insert(id, now + HOLD_EVERY);
    }

    pub fn failed(&mut self, id: Uuid, now: Instant) {
        self.next.insert(id, now + HOLD_RETRY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newly_queued_dispatch_is_held_at_once_then_hourly() {
        let (a, now) = (Uuid::new_v4(), Instant::now());
        let mut holds = QueueHolds::default();
        assert_eq!(holds.due([a], now), [a]);
        holds.held(a, now);
        assert!(holds.due([a], now + Duration::from_mins(59)).is_empty());
        assert_eq!(holds.due([a], now + HOLD_EVERY), [a]);
    }

    #[test]
    fn a_failed_hold_is_retried_soon() {
        let (a, now) = (Uuid::new_v4(), Instant::now());
        let mut holds = QueueHolds::default();
        holds.failed(a, now);
        assert!(holds.due([a], now + Duration::from_mins(1)).is_empty());
        assert_eq!(holds.due([a], now + HOLD_RETRY), [a]);
    }

    #[test]
    fn a_dispatch_that_left_the_queue_is_forgotten() {
        let (a, b, now) = (Uuid::new_v4(), Uuid::new_v4(), Instant::now());
        let mut holds = QueueHolds::default();
        holds.held(a, now);
        holds.held(b, now);
        assert!(holds.due([b], now).is_empty());
        assert_eq!(holds.due([a], now), [a], "a resumed then requeued dispatch is due again");
    }
}
