//! Presence types shared with the dispatch and session domains.

/// What kind of WS a presence row describes. Stored as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Daemon,
    Dispatcher,
    /// One announced session, so session-scoped frames reach the pod holding
    /// the WS that announced it rather than whichever pod owns the (possibly
    /// shared) machine identity.
    Session,
}

impl Kind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Dispatcher => "dispatcher",
            Self::Session => "session",
        }
    }
}

/// A row's heartbeat must be at most this old to be trusted. Heartbeats are
/// written every `HEARTBEAT_SECS`, so 3× distinguishes a crashed pod from a
/// slow tick (mirrors the WS read-timeout discipline).
pub const LIVE_WITHIN_SECS: i32 = 45;
