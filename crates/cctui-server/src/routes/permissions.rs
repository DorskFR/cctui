use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use cctui_proto::ws::ServerEvent;

use crate::state::AppState;

// --- Permission store (in-memory) ---

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PendingPermission {
    pub session_id: String,
    pub request_id: String,
    pub tool_name: String,
    pub description: String,
    pub input_preview: String,
    pub received_at: DateTime<Utc>,
}

pub struct PermissionStore {
    /// Pending requests waiting for TUI decision: `request_id` → entry
    pending: HashMap<String, PendingPermission>,
    /// Decisions recorded by TUI: `request_id` → behavior ("allow" | "deny")
    decisions: HashMap<String, String>,
}

impl PermissionStore {
    pub fn new() -> Self {
        Self { pending: HashMap::new(), decisions: HashMap::new() }
    }

    pub fn shared() -> SharedPermissionStore {
        Arc::new(RwLock::new(Self::new()))
    }

    pub fn insert_request(&mut self, req: PendingPermission) {
        self.pending.insert(req.request_id.clone(), req);
    }

    pub fn record_decision(&mut self, request_id: &str, behavior: String) {
        self.pending.remove(request_id);
        self.decisions.insert(request_id.to_string(), behavior);
    }

    pub fn take_decision(&mut self, request_id: &str) -> Option<String> {
        self.decisions.remove(request_id)
    }

    /// Remove stale entries older than `max_age_secs`
    pub fn reap_stale(&mut self, max_age_secs: i64) {
        let cutoff = Utc::now() - chrono::Duration::seconds(max_age_secs);
        self.pending.retain(|_, p| p.received_at > cutoff);
    }
}

pub type SharedPermissionStore = Arc<RwLock<PermissionStore>>;

// --- HTTP types ---

#[derive(Debug, Deserialize)]
pub struct PermissionRequestPayload {
    pub request_id: String,
    pub tool_name: String,
    pub description: String,
    pub input_preview: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PermissionDecisionResponse {
    Pending,
    Decided { behavior: String },
}

// --- Handlers ---

/// Channel submits a permission request for a session.
/// Stores it and broadcasts to TUI.
pub async fn submit_request(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<PermissionRequestPayload>,
) -> StatusCode {
    tracing::info!(
        session_id = %session_id,
        request_id = %req.request_id,
        tool_name = %req.tool_name,
        "permission request received from channel"
    );

    let entry = PendingPermission {
        session_id: session_id.clone(),
        request_id: req.request_id.clone(),
        tool_name: req.tool_name.clone(),
        description: req.description.clone(),
        input_preview: req.input_preview.clone(),
        received_at: Utc::now(),
    };

    state.permission_store.write().await.insert_request(entry);

    // Broadcast to TUI clients
    let event = ServerEvent::PermissionRequest {
        session_id,
        request_id: req.request_id,
        tool_name: req.tool_name,
        description: req.description,
        input_preview: req.input_preview,
    };
    let _ = state.tui_tx.send(event);

    StatusCode::ACCEPTED
}

/// Channel polls for a permission decision.
pub async fn poll_decision(
    State(state): State<AppState>,
    Path((session_id, request_id)): Path<(String, String)>,
) -> Json<PermissionDecisionResponse> {
    let mut store = state.permission_store.write().await;
    #[allow(clippy::option_if_let_else)]
    match store.take_decision(&request_id) {
        Some(behavior) => {
            tracing::info!(
                session_id = %session_id,
                request_id = %request_id,
                behavior = %behavior,
                "permission decision retrieved by channel"
            );
            Json(PermissionDecisionResponse::Decided { behavior })
        }
        None => Json(PermissionDecisionResponse::Pending),
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_retrieve_decision() {
        let mut store = PermissionStore::new();
        let req = PendingPermission {
            session_id: "s1".into(),
            request_id: "r1".into(),
            tool_name: "Bash".into(),
            description: "run ls".into(),
            input_preview: "ls -la".into(),
            received_at: Utc::now(),
        };
        store.insert_request(req);
        assert!(store.take_decision("r1").is_none()); // not decided yet
        store.record_decision("r1", "allow".into());
        assert_eq!(store.take_decision("r1"), Some("allow".into()));
        assert!(store.take_decision("r1").is_none()); // consumed
    }

    #[test]
    fn reap_stale_removes_old_pending() {
        let mut store = PermissionStore::new();
        let mut old_req = PendingPermission {
            session_id: "s1".into(),
            request_id: "r1".into(),
            tool_name: "Bash".into(),
            description: "run ls".into(),
            input_preview: "ls".into(),
            received_at: Utc::now(),
        };
        // Backdating to make it stale
        old_req.received_at = Utc::now() - chrono::Duration::seconds(120);
        store.insert_request(old_req);
        assert_eq!(store.pending.len(), 1);
        store.reap_stale(60); // 60s max age
        assert_eq!(store.pending.len(), 0);
    }
}
