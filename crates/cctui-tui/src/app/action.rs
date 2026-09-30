use cctui_proto::api::SessionListItem;
use cctui_proto::models::SessionStatus;
use crossterm::event::KeyEvent;

use super::attention::AttentionAction;
use super::conversation::ConversationAction;
use super::conversation_store::{PageKind, PageRequest};
use super::drafts::DraftAction;
use super::identity::AuthAction;
use super::prompt::PromptAction;
use super::send::SendAction;
use super::session_live::SessionLiveAction;
use super::state::ConversationLine;
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

    Attention(AttentionAction),

    RefreshSessions,
    SessionsLoaded(Vec<SessionListItem>),
    Conversation(ConversationAction),
    CmdLine(super::cmdline::CmdAction),
    Prompt(PromptAction),

    StreamLine {
        session_id: String,
        seq: Option<i64>,
        /// `None` for an event with nothing to render; the `usage` beside it may
        /// still move the session row. Boxed: a line is far the largest payload
        /// in this enum.
        line: Option<Box<ConversationLine>>,
        usage: Option<HeartbeatUsage>,
    },
    SessionStatusChanged {
        session_id: String,
        status: SessionStatus,
    },
    SessionRegistered(Box<cctui_proto::models::Session>),
    SessionDeregistered(String),

    Auth(AuthAction),
    Drafts(DraftAction),
    Send(SendAction),
    SessionLive(SessionLiveAction),

    /// A pure clock advance: it moves delivery deadlines, re-evaluates the
    /// clock-derived row signals and polls the session list when one is due.
    Tick,

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
    /// `GET /permissions/pending`: the requests the server still holds, so a
    /// prompt raised before this client connected still gets a card.
    FetchPendingPermissions,
    LoadConversationPage {
        session_id: String,
        kind: PageKind,
        page: PageRequest,
        etag: Option<String>,
    },
    MarkSeen {
        session_id: String,
    },
    /// `GET /drafts`: every unsent draft, pulled once at startup.
    LoadDraftIndex,
    /// Re-read one session's draft and prompt history.
    LoadDrafts {
        session_id: String,
    },
    /// Save a draft, debounced per key. Empty text deletes it.
    SaveDraft {
        key: String,
        text: String,
    },
    /// Drop a draft now, cancelling any debounced save of it.
    DiscardDraft {
        key: String,
    },
    Subscribe {
        session_id: String,
    },
    Unsubscribe {
        session_id: String,
    },
    SendMessage {
        send_id: u64,
        session_id: String,
        content: String,
        /// 0-based option picks per question when the message answers a prompt.
        /// Replayed on every retry, so the daemon can still drive the real
        /// form after a frame the server never received.
        ask_picks: Option<Vec<Vec<usize>>>,
        /// Minted on the first attempt and replayed on every retry.
        turn_id: Option<uuid::Uuid>,
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
    /// Persist the fold state to `tui-state.json`.
    SaveUiState(crate::config::uistate::UiState),
}
