use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    pub(crate) const fn marker(self) -> &'static str {
        match self {
            Self::Info => "·",
            Self::Warn => "!",
            Self::Error => "✖",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Toast {
    pub(crate) level: Level,
    pub(crate) text: String,
    pub(crate) expires_ms: i64,
}

/// Transient messages for the status line. Time is passed in rather than read:
/// the reducer that pushes these must stay free of clock reads.
#[derive(Debug, Default, Clone)]
pub(crate) struct Toasts {
    items: VecDeque<Toast>,
}

impl Toasts {
    pub(crate) const TTL_MS: i64 = 6_000;
    const MAX: usize = 3;

    pub(crate) fn push(&mut self, level: Level, text: impl Into<String>, now_ms: i64) {
        self.items.push_back(Toast {
            level,
            text: text.into(),
            expires_ms: now_ms.saturating_add(Self::TTL_MS),
        });
        while self.items.len() > Self::MAX {
            self.items.pop_front();
        }
    }

    pub(crate) fn prune(&mut self, now_ms: i64) {
        self.items.retain(|t| t.expires_ms > now_ms);
    }

    /// The newest toast: the status line has room for exactly one.
    pub(crate) fn latest(&self) -> Option<&Toast> {
        self.items.back()
    }

    pub(crate) fn queued(&self) -> usize {
        self.items.len()
    }
}

/// Counts of things the TUI could not act on. Surfaced in the status line so a
/// silent drop is never invisible.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StatusCounters {
    pub(crate) undecodable_ws_messages: u64,
    pub(crate) undecodable_agent_events: u64,
}

impl StatusCounters {
    pub(crate) const fn total(self) -> u64 {
        self.undecodable_ws_messages + self.undecodable_agent_events
    }

    pub(crate) const fn is_clean(self) -> bool {
        self.total() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::{Level, StatusCounters, Toasts};

    #[test]
    fn expired_toasts_are_pruned() {
        let mut toasts = Toasts::default();
        toasts.push(Level::Info, "first", 0);
        toasts.push(Level::Warn, "second", 5_000);
        toasts.prune(Toasts::TTL_MS + 1);
        assert_eq!(toasts.queued(), 1);
        assert_eq!(toasts.latest().map(|t| t.text.as_str()), Some("second"));
    }

    #[test]
    fn the_queue_is_capped_and_keeps_the_newest() {
        let mut toasts = Toasts::default();
        for i in 0..6 {
            toasts.push(Level::Info, format!("t{i}"), 0);
        }
        assert_eq!(toasts.queued(), 3);
        assert_eq!(toasts.latest().map(|t| t.text.as_str()), Some("t5"));
    }

    #[test]
    fn counters_start_clean() {
        let mut counters = StatusCounters::default();
        assert!(counters.is_clean());
        counters.undecodable_ws_messages += 2;
        counters.undecodable_agent_events += 1;
        assert_eq!(counters.total(), 3);
        assert!(!counters.is_clean());
    }
}
