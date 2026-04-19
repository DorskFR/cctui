use std::collections::HashMap;

use cctui_proto::api::SessionListItem;
use ratatui::style::{Color, Style};
use ratatui_textarea::TextArea;

#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Tool name for ToolCall/ToolResult lines (for context-aware rendering).
    pub tool: Option<String>,
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
    pub view: View,
    pub sessions: Vec<SessionListItem>,
    pub selected_index: usize,
    pub stream_buffer: HashMap<String, Vec<ConversationLine>>,
    pub message_input: TextArea<'static>,
    pub input_active: bool,
    pub should_quit: bool,
    /// Queue of pending permission requests; first is shown as dialog.
    pub permission_queue: std::collections::VecDeque<PendingPermission>,
    /// View to return to after dismissing a permission dialog.
    pub pre_permission_view: View,
    pub scroll_offset: usize,
    pub follow_tail: bool,
    pub active_count: usize,
    pub show_timestamps: bool,
    pub show_all_sessions: bool,
    /// Last known content area height (set during render, used for scroll math).
    pub viewport_height: usize,
    /// Total display lines in current conversation (set during render).
    pub total_display_lines: usize,
    /// Cached rendered display lines for the current conversation.
    /// Invalidated when `render_cache_len` != `stream_buffer` length for the session.
    pub render_cache: Vec<ratatui::text::Line<'static>>,
    pub render_cache_session: String,
    pub render_cache_len: usize,
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
            view: View::SessionList,
            sessions: Vec::new(),
            selected_index: 0,
            stream_buffer: HashMap::new(),
            message_input: Self::new_input_textarea(),
            input_active: false,
            should_quit: false,
            permission_queue: std::collections::VecDeque::new(),
            pre_permission_view: View::SessionList,
            scroll_offset: 0,
            follow_tail: true,
            active_count: 0,
            show_timestamps: false,
            show_all_sessions: false,
            viewport_height: 0,
            total_display_lines: 0,
            render_cache: Vec::new(),
            render_cache_session: String::new(),
            render_cache_len: 0,
        }
    }

    pub fn selected_session(&self) -> Option<&SessionListItem> {
        let flat = self.flattened_sessions();
        flat.get(self.selected_index).copied()
    }

    pub fn flattened_sessions(&self) -> Vec<&SessionListItem> {
        let mut sorted: Vec<&SessionListItem> = self.sessions.iter().collect();
        sorted.sort_by_key(|s| s.uptime_secs);
        if !self.show_all_sessions {
            sorted.truncate(5);
        }
        sorted
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

    pub fn scroll_to_bottom(&mut self) {
        if let Some(session) = self.selected_session() {
            let line_count = self.stream_buffer.get(&session.id).map_or(0, Vec::len);
            self.scroll_offset = line_count.saturating_sub(1);
        }
    }

    pub fn update_aggregates(&mut self) {
        self.active_count = self
            .sessions
            .iter()
            .filter(|s| s.status == cctui_proto::models::SessionStatus::Active)
            .count();
    }

    pub fn active_sessions(&self) -> Vec<&SessionListItem> {
        self.sessions
            .iter()
            .filter(|s| s.status == cctui_proto::models::SessionStatus::Active)
            .collect()
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
