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

/// A live `AskUserQuestion` the agent is currently blocked on (CCT-277).
/// Held authoritatively per session so it can be replayed to any client that
/// (re)subscribes — unlike the original fire-and-forget broadcast, which was
/// lost forever if no client happened to be listening when it went out.
#[derive(Debug, Clone)]
pub struct PendingAsk {
    pub session_id: String,
    pub question: String,
    pub questions: Option<serde_json::Value>,
    pub preamble: Option<String>,
    pub received_at: DateTime<Utc>,
}

pub struct PermissionStore {
    /// Pending requests waiting for TUI decision: `request_id` → entry.
    /// Populated by the daemon-WS path when an adapter forwards a
    /// `PermissionRequest`; consumed by `record_decision` when a client
    /// responds via the TUI WS.
    pending: HashMap<String, PendingPermission>,
    /// Live `AskUserQuestion` prompts, keyed by `session_id` (a session has at
    /// most one open ask form). Populated on `AskQuestion`, dropped on
    /// `AskResolved`; replayed to (re)subscribing clients so a prompt is never
    /// lost to a momentary unsubscribe (CCT-277).
    asks: HashMap<String, PendingAsk>,
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
            asks: HashMap::new(),
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

    /// Record the live `AskUserQuestion` for a session (CCT-277). A newer ask
    /// for the same session replaces the prior one.
    pub fn insert_ask(&mut self, ask: PendingAsk) {
        self.asks.insert(ask.session_id.clone(), ask);
    }

    /// Drop the live ask for a session once it resolves (answered anywhere, or
    /// dismissed). Idempotent.
    pub fn remove_ask(&mut self, session_id: &str) {
        self.asks.remove(session_id);
    }

    /// The session's currently-open ask, if any — used to replay the prompt to
    /// a (re)subscribing client.
    #[must_use]
    pub fn pending_ask(&self, session_id: &str) -> Option<PendingAsk> {
        self.asks.get(session_id).cloned()
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
        // Asks are normally cleared by `AskResolved`; this is only a leak
        // backstop. A user can deliberate over an `AskUserQuestion` for far
        // longer than a tool-permission prompt, so give it a much wider window
        // (4×) before treating an entry as stale and dropping it from replay.
        let ask_cutoff = Utc::now() - chrono::Duration::seconds(max_age_secs.saturating_mul(4));
        self.asks.retain(|_, a| a.received_at > ask_cutoff);
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
    fn ask_store_insert_get_remove() {
        let mut store = PermissionStore::new();
        assert!(store.pending_ask("s1").is_none());
        store.insert_ask(PendingAsk {
            session_id: "s1".into(),
            question: "pick one".into(),
            questions: Some(serde_json::json!([{ "question": "pick one", "options": [] }])),
            preamble: Some("context".into()),
            received_at: Utc::now(),
        });
        let got = store.pending_ask("s1").expect("ask present");
        assert_eq!(got.question, "pick one");
        assert_eq!(got.preamble.as_deref(), Some("context"));
        // A newer ask for the same session replaces the prior one.
        store.insert_ask(PendingAsk {
            session_id: "s1".into(),
            question: "now this".into(),
            questions: None,
            preamble: None,
            received_at: Utc::now(),
        });
        assert_eq!(store.pending_ask("s1").unwrap().question, "now this");
        store.remove_ask("s1");
        assert!(store.pending_ask("s1").is_none());
        // Removing an absent ask is a harmless no-op.
        store.remove_ask("s1");
    }

    #[test]
    fn reap_stale_keeps_asks_longer_than_permissions() {
        let mut store = PermissionStore::new();
        // An ask older than the permission window but inside the 4× ask window
        // must survive — users deliberate over AskUserQuestion prompts.
        store.insert_ask(PendingAsk {
            session_id: "s1".into(),
            question: "q".into(),
            questions: None,
            preamble: None,
            received_at: Utc::now() - chrono::Duration::seconds(120),
        });
        store.reap_stale(60);
        assert!(store.pending_ask("s1").is_some(), "ask within 4× window survives");
        // Beyond the 4× window it is reaped as a leak backstop.
        store.reap_stale(20);
        assert!(store.pending_ask("s1").is_none(), "ask beyond 4× window reaped");
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
