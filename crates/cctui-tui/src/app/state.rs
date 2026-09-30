use std::collections::HashMap;

use cctui_proto::api::SessionListItem;
use ratatui::style::{Color, Style};
use ratatui_textarea::TextArea;

use super::conversation_store::ConversationStore;
use super::identity::AuthState;
use super::router::Router;
use super::session_live::RefreshCounters;
pub use super::session_list::uptime_secs;
use super::toast::{Level, StatusCounters, Toasts};
pub use crate::config::uistate::UiState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    SessionList,
    Conversation,
    Help,
    PermissionDialog,
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

/// Conversation line with metadata for rendering.
pub struct ConversationLine {
    pub timestamp: i64,
    pub kind: LineKind,
    pub text: String,
    /// Raw tool input JSON (kept for Edit/Write to generate diffs).
    pub tool_input: Option<serde_json::Value>,
}

#[derive(Clone, PartialEq, Eq)]
pub enum LineKind {
    User,
    Assistant,
    ToolCall,
    ToolResult,
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
    /// Queue of pending permission requests; first is shown as dialog.
    pub permission_queue: std::collections::VecDeque<PendingPermission>,
    pub scroll_offset: usize,
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
    /// An older page landed: the next render re-anchors the viewport onto the
    /// lines the user was reading instead of letting them slide down.
    pub pending_prepend: bool,
    pub toasts: Toasts,
    pub status: StatusCounters,
    pub auth: AuthState,
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
            permission_queue: std::collections::VecDeque::new(),
            scroll_offset: 0,
            follow_tail: true,
            active_count: 0,
            show_timestamps: false,
            viewport_height: 0,
            total_display_lines: 0,
            render_cache: Vec::new(),
            render_cache_session: String::new(),
            render_cache_entries: 0,
            render_cache_epoch: 0,
            pending_prepend: false,
            toasts: Toasts::default(),
            status: StatusCounters::default(),
            auth: AuthState::Unknown,
            clock_ms: 0,
            machine_liveness: HashMap::new(),
            ws_healthy: false,
            last_refresh_ms: 0,
            refresh: RefreshCounters::default(),
            ui: UiState::default(),
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

    /// Requests waiting on this session, the count lane E's list-level
    /// indicator reads too.
    pub fn pending_permissions(&self, session_id: &str) -> usize {
        self.permission_queue.iter().filter(|p| p.session_id == session_id).count()
    }

    pub fn selected_session_id(&self) -> Option<String> {
        self.selected_session().map(|s| s.id.clone())
    }

    #[cfg(test)]
    pub fn conversation(&self, session_id: &str) -> Option<&ConversationStore> {
        self.conversations.get(session_id)
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
