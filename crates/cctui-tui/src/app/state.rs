use std::collections::HashMap;

use cctui_proto::api::SessionListItem;
use ratatui::style::{Color, Style};
use ratatui_textarea::TextArea;

use super::conversation_store::ConversationStore;
use super::router::Router;
use super::toast::{Level, StatusCounters, Toasts};

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

/// Display ordering for the classifier buckets in the session list:
/// sessions that want the user's eyes float to the top.
pub const fn bucket_rank(bucket: cctui_proto::classifier::Bucket) -> u8 {
    use cctui_proto::classifier::Bucket;
    match bucket {
        Bucket::Blocked => 0,
        Bucket::Review => 1,
        Bucket::Working => 2,
        Bucket::Done => 3,
    }
}

/// Uptime derived from `registered_at`; 0 when unset.
pub fn uptime_secs(s: &SessionListItem) -> i64 {
    s.registered_at.map_or(0, |r| (chrono::Utc::now() - r).num_seconds())
}

#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub router: Router,
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
    pub show_all_sessions: bool,
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
    /// Refreshed once per loop iteration; the reducer reads this instead of the
    /// clock so it stays pure and testable.
    pub clock_ms: i64,
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
            show_all_sessions: false,
            viewport_height: 0,
            total_display_lines: 0,
            render_cache: Vec::new(),
            render_cache_session: String::new(),
            render_cache_entries: 0,
            render_cache_epoch: 0,
            pending_prepend: false,
            toasts: Toasts::default(),
            status: StatusCounters::default(),
            clock_ms: 0,
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

    pub fn conversation(&self, session_id: &str) -> Option<&ConversationStore> {
        self.conversations.get(session_id)
    }

    pub fn conversation_mut(&mut self, session_id: &str) -> &mut ConversationStore {
        self.conversations.entry(session_id.to_owned()).or_default()
    }

    /// Session list ordered for display, with Task-tool subagents
    /// placed immediately after their parent. Top-level sessions (those with
    /// no in-list parent) are ordered by uptime; each parent's children
    /// follow it, also uptime-ordered.
    pub fn flattened_sessions(&self) -> Vec<&SessionListItem> {
        use std::collections::{HashMap, HashSet};
        let ids: HashSet<&str> = self.sessions.iter().map(|s| s.id.as_str()).collect();
        let mut kids: HashMap<&str, Vec<&SessionListItem>> = HashMap::new();
        for s in &self.sessions {
            if let Some(p) = s.parent_id.as_deref().filter(|p| ids.contains(p)) {
                kids.entry(p).or_default().push(s);
            }
        }
        let mut tops: Vec<&SessionListItem> = self
            .sessions
            .iter()
            .filter(|s| s.parent_id.as_deref().is_none_or(|p| !ids.contains(p)))
            .collect();
        // Group by classifier bucket (Needs input → Ready for review →
        // Working → Completed); within a bucket, oldest first.
        tops.sort_by_key(|s| (bucket_rank(s.bucket), uptime_secs(s)));
        let mut out: Vec<&SessionListItem> = Vec::new();
        for t in tops {
            out.push(t);
            if let Some(cs) = kids.get_mut(t.id.as_str()) {
                cs.sort_by_key(|s| uptime_secs(s));
                out.extend(cs.iter().copied());
            }
        }
        if !self.show_all_sessions {
            out.truncate(5);
        }
        out
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
