use cctui_proto::api::SessionListItem;
use cctui_proto::models::SessionStatus;
use crossterm::event::KeyEvent;

use super::conversation::ConversationAction;
use super::conversation_store::{PageKind, PageRequest};
use super::identity::AuthAction;
use super::state::{ConversationLine, PendingPermission};
use super::toast::Level;

/// Everything that can change the app. Key handlers, the websocket and
/// completed effects all funnel through this one vocabulary.
pub enum Action {
    Quit,

    SelectNext,
    SelectPrev,
    SelectFirst,
    SelectLast,
    SelectIndex(usize),

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
    Conversation(ConversationAction),

    StreamLine {
        session_id: String,
        seq: Option<i64>,
        line: ConversationLine,
        usage: Option<HeartbeatUsage>,
    },
    SessionStatusChanged {
        session_id: String,
        status: SessionStatus,
    },
    SessionRegistered(Box<cctui_proto::models::Session>),
    SessionDeregistered(String),

    Auth(AuthAction),

    Reconnected,
    Toast(Level, String),
    /// A websocket frame the TUI could not deserialize.
    UndecodableWsMessage(String),
    /// Persisted agent events the TUI could not deserialize.
    UndecodableAgentEvents(usize),
}

/// Token/cost figures a heartbeat carries for the session row.
pub struct HeartbeatUsage {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
}

/// The only way the reducer reaches the network. Nothing here runs on the
/// key-handling path; the effects runner owns them.
pub enum Effect {
    RefreshSessions,
    /// `GET /me`: resolve the identity behind the configured key.
    FetchIdentity,
    LoadConversationPage {
        session_id: String,
        kind: PageKind,
        page: PageRequest,
        etag: Option<String>,
    },
    MarkSeen {
        session_id: String,
    },
    Subscribe {
        session_id: String,
    },
    Unsubscribe {
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
