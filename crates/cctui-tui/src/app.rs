use std::collections::HashMap;

use cctui_proto::api::SessionListItem;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    Sessions,
    Conversation,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pane {
    Tree,
    Detail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailMode {
    Conversation,
    Log,
}

pub struct App {
    pub view: View,
    pub active_pane: Pane,
    pub detail_mode: DetailMode,
    pub sessions: Vec<SessionListItem>,
    pub selected_index: usize,
    pub stream_buffer: HashMap<Uuid, Vec<String>>,
    pub message_input: Option<String>,
    pub should_quit: bool,
    pub filter: Option<String>,
    pub aggregate_tokens: u64,
    pub aggregate_cost: f64,
    pub active_count: usize,
}

impl App {
    pub fn new() -> Self {
        Self {
            view: View::Sessions,
            active_pane: Pane::Tree,
            detail_mode: DetailMode::Conversation,
            sessions: Vec::new(),
            selected_index: 0,
            stream_buffer: HashMap::new(),
            message_input: None,
            should_quit: false,
            filter: None,
            aggregate_tokens: 0,
            aggregate_cost: 0.0,
            active_count: 0,
        }
    }

    pub fn selected_session(&self) -> Option<&SessionListItem> {
        let flat = self.flattened_sessions();
        flat.get(self.selected_index).copied()
    }

    pub fn flattened_sessions(&self) -> Vec<&SessionListItem> {
        let mut result = Vec::new();
        let roots: Vec<&SessionListItem> =
            self.sessions.iter().filter(|s| s.parent_id.is_none()).collect();
        for root in roots {
            result.push(root);
            self.append_children(root.id, &mut result);
        }
        result
    }

    pub fn append_children<'a>(&'a self, parent_id: Uuid, result: &mut Vec<&'a SessionListItem>) {
        let children: Vec<&SessionListItem> =
            self.sessions.iter().filter(|s| s.parent_id == Some(parent_id)).collect();
        for child in children {
            result.push(child);
            self.append_children(child.id, result);
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

    pub fn update_aggregates(&mut self) {
        use cctui_proto::models::SessionStatus;
        let mut tokens: u64 = 0;
        let mut cost: f64 = 0.0;
        let mut active: usize = 0;
        for session in &self.sessions {
            tokens += session.token_usage.tokens_in + session.token_usage.tokens_out;
            cost += session.token_usage.cost_usd;
            if session.status == SessionStatus::Active {
                active += 1;
            }
        }
        self.aggregate_tokens = tokens;
        self.aggregate_cost = cost;
        self.active_count = active;
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
