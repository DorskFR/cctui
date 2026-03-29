use std::collections::HashMap;

use cctui_proto::api::SessionListItem;
use ratatui_textarea::TextArea;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    SessionList,
    Conversation,
    Help,
}

/// Conversation line with metadata for rendering.
pub struct ConversationLine {
    pub timestamp: i64,
    pub kind: LineKind,
    pub text: String,
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
    pub scroll_offset: usize,
    pub follow_tail: bool,
    pub active_count: usize,
    pub show_sidebar: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            view: View::SessionList,
            sessions: Vec::new(),
            selected_index: 0,
            stream_buffer: HashMap::new(),
            message_input: TextArea::default(),
            input_active: false,
            should_quit: false,
            scroll_offset: 0,
            follow_tail: true,
            active_count: 0,
            show_sidebar: true,
        }
    }

    pub fn selected_session(&self) -> Option<&SessionListItem> {
        let flat = self.flattened_sessions();
        flat.get(self.selected_index).copied()
    }

    /// Flat list of sessions ordered by machine, then parents before children.
    pub fn flattened_sessions(&self) -> Vec<&SessionListItem> {
        let mut by_machine: HashMap<&str, Vec<&SessionListItem>> = HashMap::new();
        for s in &self.sessions {
            by_machine.entry(s.machine_id.as_str()).or_default().push(s);
        }

        let mut machines: Vec<&str> = by_machine.keys().copied().collect();
        machines.sort_unstable();

        let mut result: Vec<&SessionListItem> = Vec::new();
        for machine in machines {
            let group = &by_machine[machine];
            // roots first, then children
            for s in group.iter().filter(|s| s.parent_id.is_none()) {
                result.push(s);
                Self::append_children(&s.id, group, &mut result);
            }
        }
        result
    }

    fn append_children<'a>(
        parent_id: &str,
        group: &[&'a SessionListItem],
        result: &mut Vec<&'a SessionListItem>,
    ) {
        for s in group.iter().filter(|s| s.parent_id.as_deref() == Some(parent_id)) {
            result.push(s);
            Self::append_children(&s.id, group, result);
        }
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
            .filter(|s| {
                s.status == cctui_proto::models::SessionStatus::Active
                    || s.status == cctui_proto::models::SessionStatus::Idle
            })
            .collect()
    }

    /// Get the `machine_id` for the session at index, or None if same as previous.
    /// Used to render machine group headers.
    pub fn machine_header_at(&self, index: usize) -> Option<&str> {
        let flat = self.flattened_sessions();
        let current = flat.get(index)?;
        if index == 0 {
            return Some(&current.machine_id);
        }
        let prev = flat.get(index - 1)?;
        if current.machine_id == prev.machine_id { None } else { Some(&current.machine_id) }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
