use cctui_proto::api::SessionListItem;
use cctui_proto::models::SessionStatus;
use crossterm::event::KeyEvent;

use super::state::{ConversationLine, PendingPermission};
use super::toast::Level;

/// Everything that can change the app. Key handlers, the websocket and
/// completed effects all funnel through this one vocabulary.
pub(crate) enum Action {
    Quit,

    SelectNext,
    SelectPrev,
    SelectFirst,
    SelectLast,
    SelectIndex(usize),

    ToggleShowAllSessions,
    ToggleTimestamps,

    OpenHelp,
    CloseHelp,
    OpenSelectedConversation,
    LeaveConversation,

    /// `release_follow` mirrors the per-binding behaviour: scrolling up always
    /// detaches from the tail, scrolling down only does for line-wise keys.
    Scroll {
        lines: i32,
        release_follow: bool,
    },
    ScrollToTop,
    ScrollToBottom,

    ActivateInput,
    /// A key that no navigation binding claimed: it both opens the composer
    /// and types its first character.
    ActivateInputWith(KeyEvent),
    CancelInput,
    InputKey(KeyEvent),
    InputNewline,
    SubmitInput,

    InterruptSelected,
    ToggleAutoApproveSelected,
    AutoApproveSet {
        session_id: String,
        enabled: bool,
    },

    ResolvePermission {
        allow: bool,
    },
    PermissionRequested(PendingPermission),
    PermissionResolved {
        session_id: String,
        request_id: String,
    },

    RefreshSessions,
    SessionsLoaded(Vec<SessionListItem>),
    ConversationLoaded {
        session_id: String,
        lines: Vec<ConversationLine>,
    },

    StreamLine {
        session_id: String,
        line: ConversationLine,
        usage: Option<HeartbeatUsage>,
    },
    SessionStatusChanged {
        session_id: String,
        status: SessionStatus,
    },
    SessionRegistered(Box<cctui_proto::models::Session>),
    SessionDeregistered(String),

    Reconnected,
    Toast(Level, String),
    /// A websocket frame the TUI could not deserialize.
    UndecodableWsMessage(String),
    /// Persisted agent events the TUI could not deserialize.
    UndecodableAgentEvents(usize),
}

/// Token/cost figures a heartbeat carries for the session row.
pub(crate) struct HeartbeatUsage {
    pub(crate) tokens_in: u64,
    pub(crate) tokens_out: u64,
    pub(crate) cost_usd: f64,
}

/// The only way the reducer reaches the network. Nothing here runs on the
/// key-handling path; the effects runner owns them.
pub(crate) enum Effect {
    RefreshSessions,
    /// `fetch` is false when the conversation is already buffered; the
    /// subscribe still goes out either way.
    LoadConversation {
        session_id: String,
        fetch: bool,
    },
    Subscribe {
        session_id: String,
    },
    SendMessage {
        session_id: String,
        content: String,
    },
    Interrupt {
        session_id: String,
    },
    SetAutoApprove {
        session_id: String,
        enabled: bool,
    },
    RespondPermission {
        session_id: String,
        request_id: String,
        behavior: &'static str,
    },
}
