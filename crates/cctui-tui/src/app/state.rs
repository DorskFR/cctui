use std::collections::{HashMap, HashSet};

use cctui_proto::api::SessionListItem;
use ratatui::style::{Color, Style};
use ratatui_textarea::TextArea;

use super::attention::PermissionInbox;
use super::conversation_store::ConversationStore;
use super::diagnose::DiagnosePanel;
use super::identity::AuthState;
use super::prompt::{AskCard, PlanCard};
use super::router::Router;
pub use super::session_list::uptime_secs_at;
use super::session_live::RefreshCounters;
use super::toast::{Level, StatusCounters, Toasts};
pub use crate::config::uistate::UiState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The overlay pager for an agent-linked local file.
    FileViewer,
    SessionList,
    Conversation,
    Help,
    HistoryPicker,
    Pins,
    Macros,
    Diagnose,
    Terminal,
    ModelPicker,
    Sidebar,
}

/// A pending permission request from Claude Code that needs TUI approval.
#[derive(Debug, Clone)]
pub struct PendingPermission {
    pub session_id: String,
    pub request_id: String,
    pub tool_name: String,
    pub description: String,
    pub input_preview: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    Read,
    Write,
    Mcp,
    /// Executed by the provider rather than the harness.
    Server,
    Other,
}

impl ToolCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Mcp => "mcp",
            Self::Server => "server",
            Self::Other => "tool",
        }
    }

    /// `server_tool_use` wins over the name-based bucket.
    #[must_use]
    pub fn of(tool: &str, event_kind: Option<&str>) -> Self {
        if matches!(event_kind, Some("server_tool_use" | "server_tool_result")) {
            return Self::Server;
        }
        match tool {
            "Read" | "Glob" | "Grep" | "WebFetch" | "WebSearch" | "LSP" => Self::Read,
            "Write" | "Edit" | "Bash" | "NotebookEdit" => Self::Write,
            name if name.starts_with("mcp__") => Self::Mcp,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnFooter {
    pub duration_ms: Option<u64>,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    /// The server classified the turn as wanting the operator.
    pub needs_action: bool,
}

impl TurnFooter {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.duration_ms.is_none() && self.tokens_in.is_none() && self.tokens_out.is_none()
    }
}

/// One rendered transcript line.
///
/// Build with [`ConversationLine::new`] so adding a field never touches a call
/// site.
#[derive(Debug, Clone, Default)]
pub struct ConversationLine {
    pub timestamp: i64,
    pub kind: LineKind,
    pub text: String,
    /// Raw tool input JSON (kept for Edit/Write to generate diffs).
    pub tool_input: Option<serde_json::Value>,
    /// Tool name on [`LineKind::Tool`] and [`LineKind::Result`].
    pub tool: Option<String>,
    pub message_id: Option<String>,
    pub turn_id: Option<uuid::Uuid>,
    /// Sender of a peer message: a display name when one was supplied.
    pub peer_from: Option<String>,
    /// Room a peer message came through; absent for a direct one.
    pub peer_room: Option<String>,
    /// Duration and token figures on [`LineKind::Summary`].
    pub footer: Option<TurnFooter>,
    /// Delivery or queue state; `None` is a settled line.
    pub status: Option<LineStatus>,
}

/// What a line is still waiting for: delivery of the user's own send, or the
/// agent taking a queued prompt off its queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineStatus {
    Sending,
    Retrying {
        attempt: u32,
        max: u32,
    },
    Delivered,
    Failed(String),
    /// Waiting behind the running turn on the agent's own queue.
    Queued,
    /// Withdrawn from that queue before the agent ran it.
    Removed,
}

impl ConversationLine {
    #[must_use]
    pub fn new(kind: LineKind, text: impl Into<String>, timestamp: i64) -> Self {
        Self { timestamp, kind, text: text.into(), ..Self::default() }
    }

    #[must_use]
    pub fn with_status(mut self, status: LineStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Whether the line hides body text behind a collapse toggle.
    #[must_use]
    pub const fn collapsible(&self) -> bool {
        matches!(self.kind, LineKind::Thinking { .. } | LineKind::Result { .. } | LineKind::Compact)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LineKind {
    User,
    #[default]
    Assistant,
    /// Extended thinking; `redacted` content has no body to show.
    Thinking {
        redacted: bool,
    },
    Tool {
        category: ToolCategory,
    },
    Result {
        error: bool,
    },
    /// A message relayed from another session or a room.
    Peer,
    /// One line of harness bookkeeping.
    Marker,
    /// A `/clear` boundary.
    Reset,
    /// A `/compact` summary.
    Compact,
    /// The per-turn footer.
    Summary,
    System,
    Reply,
}

#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub router: Router,
    /// Loaded once at startup; `App::new()` is always the built-in defaults so
    /// a test never depends on the user's own `tui.toml`.
    pub config: crate::config::Config,
    /// Injected so views never read the build: a version bump must not churn
    /// every snapshot.
    pub version: &'static str,
    pub sessions: Vec<SessionListItem>,
    pub selected_index: usize,
    pub conversations: HashMap<String, ConversationStore>,
    pub subscribed: Option<String>,
    pub message_input: TextArea<'static>,
    pub input_active: bool,
    pub should_quit: bool,
    /// Pending permission requests for every session, rendered as a card in
    /// the session they belong to.
    pub permissions: PermissionInbox,
    /// Live `AskUserQuestion` per session, cleared on `AskResolved`.
    pub asks: HashMap<String, AskCard>,
    /// Live plan-approval prompt per session, cleared on `PlanResolved`.
    pub plans: HashMap<String, PlanCard>,
    /// The open diagnose/info overlay, `None` when it is closed.
    pub diagnose: Option<DiagnosePanel>,
    /// Sessions parked behind an account soft limit, from the WS frames.
    pub soft_limited: HashSet<String>,
    pub scroll_offset: usize,
    /// First cheat-sheet row shown; clamped by the overlay when it draws.
    pub help_scroll: usize,
    pub follow_tail: bool,
    pub active_count: usize,
    pub show_timestamps: bool,
    /// Last known content area height (set during render, used for scroll math).
    pub viewport_height: usize,
    /// Total display lines in current conversation (set during render).
    pub total_display_lines: usize,
    /// Rendered display lines for the current conversation. Valid only while
    /// the store has done nothing but append: `epoch` changes on any insert
    /// that lands earlier, which no append-only cache can absorb.
    pub render_cache: Vec<ratatui::text::Line<'static>>,
    pub render_cache_session: String,
    pub render_cache_entries: usize,
    pub render_cache_epoch: u64,
    /// `show_timestamps` the cache was built with: the prefix is baked into
    /// every cached row, so toggling it has to rebuild.
    pub render_cache_timestamps: bool,
    pub render_cache_pins: u64,
    /// First display row of each cached entry; the line cursor maps an entry
    /// onto its rows through this.
    pub render_cache_starts: Vec<usize>,
    /// Focused entry in line-select mode; `None` means normal scrolling.
    pub line_cursor: Option<usize>,
    /// Which transcript categories are on screen, restored from disk at startup.
    pub filter: super::transcript_filter::Filter,
    /// Focused row of the `F` menu while it is open.
    pub filter_menu: Option<usize>,
    /// The `/` or `:` prompt at the bottom of the conversation.
    pub cmdline: super::cmdline::CmdLine,
    /// The committed search and its hits.
    pub find: super::cmdline::Find,
    /// `filter.cache_key()` the render cache was built with: a filter change
    /// adds or removes rows, which no append-only cache can absorb.
    pub render_cache_filter: String,
    /// Where the server is, for a copyable session link. Empty in tests.
    pub server_url: String,
    /// Which way the next bulk toggle goes; the per-entry state is the store's.
    pub expand_all: bool,
    /// An older page landed: the next render re-anchors the viewport onto the
    /// lines the user was reading instead of letting them slide down.
    pub pending_prepend: bool,
    pub toasts: Toasts,
    pub status: StatusCounters,
    pub auth: AuthState,
    pub drafts: super::drafts::DraftState,
    pub pins: super::pins::PinState,
    pub mentions: super::mentions::MentionState,
    pub macros: super::macros::MacroState,
    /// Sends that have left the composer but are not confirmed delivered.
    pub outbox: super::send::Outbox,
    /// Files staged for the composer, uploaded on send.
    pub attachments: super::attach::Attachments,
    /// A held lead chord (the `g` of `gf`), cleared by the next key.
    pub pending_chord: Option<crate::config::chord::Chord>,
    /// The file the viewer is showing, while it is open.
    pub file_view: Option<super::fileview::FileView>,
    /// Refreshed once per loop iteration; the reducer reads this instead of the
    /// clock so it stays pure and testable.
    pub clock_ms: i64,
    /// Live machine tiers from `machine_liveness`, keyed by machine id.
    pub machine_liveness: HashMap<String, cctui_proto::models::MachineLiveness>,
    /// The socket is delivering events, so the REST poll can slow down.
    pub ws_healthy: bool,
    pub last_refresh_ms: i64,
    pub refresh: RefreshCounters,
    /// Fold state, loaded at startup and written back on every toggle.
    pub ui: UiState,
    /// Interrupt/fork confirmations and the model picker.
    pub controls: super::controls::Controls,
    /// Cursor state of the todo/subagent sidebar.
    pub sidebar: super::sidebar::Sidebar,
    /// Mark-seen debounce state; the counts themselves live on the rows.
    pub unread: super::unread::Unread,
    /// The watched session's emulated screen, open only while the pane is.
    pub terminal: Option<super::terminal::TerminalPane>,
}

impl App {
    fn new_input_textarea() -> TextArea<'static> {
        let mut ta = TextArea::default();
        ta.set_placeholder_text("Type a message...");
        ta.set_placeholder_style(Style::new().fg(Color::DarkGray));
        ta
    }

    pub fn reset_input(&mut self) {
        self.message_input = Self::new_input_textarea();
    }

    /// Replace the composer's content, leaving the caret after the last
    /// character so typing continues where the text ends.
    pub fn set_input_text(&mut self, text: &str) {
        let mut textarea = Self::new_input_textarea();
        if !text.is_empty() {
            let _ = textarea.insert_str(text);
        }
        self.message_input = textarea;
    }

    /// The same, with the caret at a character offset: a completion inserts
    /// mid-text and the user keeps typing after the token, not at the end.
    pub fn set_input_text_at(&mut self, text: &str, caret: usize) {
        self.set_input_text(text);
        let mut row = 0_usize;
        let mut col = caret;
        for line in text.split('\n') {
            let len = line.chars().count();
            if col <= len {
                break;
            }
            col -= len + 1;
            row += 1;
        }
        let jump = ratatui_textarea::CursorMove::Jump(
            u16::try_from(row).unwrap_or(u16::MAX),
            u16::try_from(col).unwrap_or(u16::MAX),
        );
        self.message_input.move_cursor(jump);
    }

    pub fn new() -> Self {
        Self {
            router: Router::new(View::SessionList),
            config: crate::config::Config::default(),
            version: env!("CARGO_PKG_VERSION"),
            sessions: Vec::new(),
            selected_index: 0,
            conversations: HashMap::new(),
            subscribed: None,
            message_input: Self::new_input_textarea(),
            input_active: false,
            should_quit: false,
            permissions: PermissionInbox::default(),
            asks: HashMap::new(),
            plans: HashMap::new(),
            diagnose: None,
            soft_limited: HashSet::new(),
            scroll_offset: 0,
            help_scroll: 0,
            follow_tail: true,
            active_count: 0,
            show_timestamps: false,
            viewport_height: 0,
            total_display_lines: 0,
            render_cache: Vec::new(),
            render_cache_session: String::new(),
            render_cache_entries: 0,
            render_cache_epoch: 0,
            render_cache_timestamps: false,
            render_cache_pins: 0,
            render_cache_starts: Vec::new(),
            line_cursor: None,
            filter: super::transcript_filter::Filter::default(),
            filter_menu: None,
            cmdline: super::cmdline::CmdLine::default(),
            find: super::cmdline::Find::default(),
            render_cache_filter: String::new(),
            server_url: String::new(),
            expand_all: false,
            pending_prepend: false,
            toasts: Toasts::default(),
            status: StatusCounters::default(),
            auth: AuthState::Unknown,
            drafts: super::drafts::DraftState::default(),
            pins: super::pins::PinState::default(),
            mentions: super::mentions::MentionState::default(),
            macros: super::macros::MacroState::default(),
            outbox: super::send::Outbox::default(),
            attachments: super::attach::Attachments::new(),
            pending_chord: None,
            file_view: None,
            clock_ms: 0,
            machine_liveness: HashMap::new(),
            ws_healthy: false,
            last_refresh_ms: 0,
            refresh: RefreshCounters::default(),
            ui: UiState::default(),
            controls: super::controls::Controls::default(),
            sidebar: super::sidebar::Sidebar::default(),
            unread: super::unread::Unread::default(),
            terminal: None,
        }
    }

    pub fn view(&self) -> View {
        self.router.current()
    }

    pub fn toast(&mut self, level: Level, text: impl Into<String>) {
        self.toasts.push(level, text, self.clock_ms);
    }

    pub fn selected_session(&self) -> Option<&SessionListItem> {
        let flat = self.flattened_sessions();
        flat.get(self.selected_index).copied()
    }

    pub fn selected_session_id(&self) -> Option<String> {
        self.selected_session().map(|s| s.id.clone())
    }

    /// An ended session takes no more input: the composer is closed for it.
    pub fn selected_session_ended(&self) -> bool {
        self.selected_session().is_some_and(|s| s.end_reason.is_some())
    }

    #[cfg(test)]
    pub fn conversation(&self, session_id: &str) -> Option<&ConversationStore> {
        self.conversations.get(session_id)
    }

    /// The modal strip or panel holding the keyboard, if any. A feature with
    /// its own context adds an arm here.
    #[must_use]
    pub const fn key_overlay(&self) -> Option<crate::config::keymap::Context> {
        use crate::config::keymap::Context;
        if self.cmdline.open.is_some() {
            return Some(Context::CmdLine);
        }
        if self.filter_menu.is_some() {
            return Some(Context::FilterMenu);
        }
        None
    }

    /// The line the cursor is on, or `None` outside line-select.
    #[must_use]
    pub fn focused_line(&self) -> Option<&ConversationLine> {
        let cursor = self.line_cursor?;
        let session_id = self.selected_session_id()?;
        let entry = self.conversations.get(&session_id)?.entries().get(cursor)?;
        Some(&entry.line)
    }

    pub fn conversation_mut(&mut self, session_id: &str) -> &mut ConversationStore {
        self.conversations.entry(session_id.to_owned()).or_default()
    }

    pub fn list_rows(&self) -> Vec<super::session_list::Row<'_>> {
        super::session_list::rows(&self.sessions, &self.ui)
    }

    pub fn flattened_sessions(&self) -> Vec<&SessionListItem> {
        super::session_list::sessions_of(&self.list_rows())
    }

    pub fn select_next(&mut self) {
        let len = self.flattened_sessions().len();
        if len > 0 && self.selected_index < len - 1 {
            self.selected_index += 1;
        }
    }

    pub const fn select_prev(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub const fn select_first(&mut self) {
        self.selected_index = 0;
    }

    pub fn select_last(&mut self) {
        let len = self.flattened_sessions().len();
        if len > 0 {
            self.selected_index = len - 1;
        }
    }

    pub fn update_aggregates(&mut self) {
        self.active_count = self
            .sessions
            .iter()
            .filter(|s| s.status == cctui_proto::models::SessionStatus::Active)
            .count();
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
