use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::RwLock;

use crate::state::AppState;

// --- Permission store (in-memory) ---

#[derive(Debug, Clone)]
pub struct PendingPermission {
    pub session_id: String,
    pub request_id: String,
    pub tool_name: String,
    pub description: String,
    pub input_preview: String,
    pub received_at: DateTime<Utc>,
}

pub struct PermissionStore {
    /// Pending requests waiting for TUI decision: `request_id` → entry.
    /// Populated by the daemon-WS path when an adapter forwards a
    /// `PermissionRequest`; consumed by `record_decision` when a client
    /// responds via the TUI WS.
    pending: HashMap<String, PendingPermission>,
    /// Decisions recorded by TUI clients: `request_id` → (`session_id`,
    /// behavior, `decided_at`). Currently used only for the audit window
    /// covered by `reap_stale`.
    decisions: HashMap<String, (String, String, DateTime<Utc>)>,
    /// Sessions with auto-approve enabled (CCT-151). A cctui-layer
    /// convenience: incoming `PermissionRequest`s for these sessions are
    /// answered `allow` immediately instead of prompting the user. Distinct
    /// from the agent's own `auto` permission mode — this works even when the
    /// agent is launched in `ask` mode. In-memory and reset on restart.
    auto_approve: std::collections::HashSet<String>,
}

impl PermissionStore {
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
            decisions: HashMap::new(),
            auto_approve: std::collections::HashSet::new(),
        }
    }

    pub fn shared() -> SharedPermissionStore {
        Arc::new(RwLock::new(Self::new()))
    }

    /// Insert a fresh pending request. Called by the daemon-WS path when an
    /// adapter forwards an `AdapterEvent::PermissionRequest`.
    pub fn insert_request(&mut self, req: PendingPermission) {
        self.pending.insert(req.request_id.clone(), req);
    }

    /// Record a decision and return the `session_id` it belonged to (empty
    /// string if the `request_id` was unknown — already consumed or never
    /// submitted). Callers use the `session_id` to broadcast a resolution event.
    pub fn record_decision(&mut self, request_id: &str, behavior: String) -> String {
        let session_id = self.pending.remove(request_id).map(|p| p.session_id).unwrap_or_default();
        self.decisions.insert(request_id.to_string(), (session_id.clone(), behavior, Utc::now()));
        session_id
    }

    pub fn list_pending(&self) -> Vec<PendingPermission> {
        self.pending.values().cloned().collect()
    }

    /// Enable/disable auto-approve for a session (CCT-151).
    pub fn set_auto_approve(&mut self, session_id: &str, enabled: bool) {
        if enabled {
            self.auto_approve.insert(session_id.to_string());
        } else {
            self.auto_approve.remove(session_id);
        }
    }

    #[must_use]
    pub fn is_auto_approve(&self, session_id: &str) -> bool {
        self.auto_approve.contains(session_id)
    }

    /// Remove stale entries older than `max_age_secs`
    pub fn reap_stale(&mut self, max_age_secs: i64) {
        let cutoff = Utc::now() - chrono::Duration::seconds(max_age_secs);
        self.pending.retain(|_, p| p.received_at > cutoff);
        self.decisions.retain(|_, (_, _, decided_at)| *decided_at > cutoff);
    }
}

pub type SharedPermissionStore = Arc<RwLock<PermissionStore>>;

#[derive(Debug, Serialize)]
pub struct PendingPermissionView {
    pub session_id: String,
    pub request_id: String,
    pub tool_name: String,
    pub description: String,
    pub input_preview: String,
}

/// List all currently-pending permission requests (for web client
/// reconciliation on (re)connect).
pub async fn list_pending(State(state): State<AppState>) -> Json<Vec<PendingPermissionView>> {
    let store = state.permission_store.read().await;
    Json(
        store
            .list_pending()
            .into_iter()
            .map(|p| PendingPermissionView {
                session_id: p.session_id,
                request_id: p.request_id,
                tool_name: p.tool_name,
                description: p.description,
                input_preview: p.input_preview,
            })
            .collect(),
    )
}

// --- Handlers ---

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_decision_returns_session_id() {
        let mut store = PermissionStore::new();
        let req = PendingPermission {
            session_id: "s1".into(),
            request_id: "r1".into(),
            tool_name: "Bash".into(),
            description: "run ls".into(),
            input_preview: "ls".into(),
            received_at: Utc::now(),
        };
        store.insert_request(req);
        assert_eq!(store.record_decision("r1", "allow".into()), "s1");
        // Unknown request_id returns empty.
        assert_eq!(store.record_decision("nope", "allow".into()), "");
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

    #[test]
    fn reap_stale_removes_old_decisions() {
        let mut store = PermissionStore::new();
        let req = PendingPermission {
            session_id: "s1".into(),
            request_id: "r1".into(),
            tool_name: "Bash".into(),
            description: "run ls".into(),
            input_preview: "ls".into(),
            received_at: Utc::now(),
        };
        store.insert_request(req);
        store.record_decision("r1", "allow".into());
        // Backdate the decision
        store.decisions.get_mut("r1").unwrap().2 = Utc::now() - chrono::Duration::seconds(120);
        assert_eq!(store.decisions.len(), 1);
        store.reap_stale(60);
        assert_eq!(store.decisions.len(), 0);
    }
}
