//! Presence, preview and plugin types shared with the dispatch and session domains.
use cctui_proto::ws::DaemonFrameUp;
use ring::rand::SecureRandom;

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

pub const MAX_SETTING_VALUE_CHARS: usize = 512;

#[must_use]
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    ring::rand::SystemRandom::new().fill(&mut b).expect("system rng");
    b
}

#[must_use]
pub const fn is_preview_frame(frame: &DaemonFrameUp) -> bool {
    matches!(
        frame,
        DaemonFrameUp::PreviewOpen { .. }
            | DaemonFrameUp::PreviewClose { .. }
            | DaemonFrameUp::PreviewResponse { .. }
            | DaemonFrameUp::PreviewChunk(_)
            | DaemonFrameUp::PreviewError { .. }
    )
}
