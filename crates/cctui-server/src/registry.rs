use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use cctui_proto::models::{Session, SessionStatus, TokenUsage};
use cctui_proto::ws::AgentEvent;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMessage {
    pub id: Uuid, // internal message ID — not a session ID
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineCommand {
    pub id: Uuid, // internal command ID — not a session ID
    pub command_type: String,
    pub payload: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug)]
pub struct SessionHandle {
    pub session: Session,
    #[allow(dead_code)]
    pub last_heartbeat: Instant,
    #[allow(dead_code)]
    pub token_usage: TokenUsage,
    pub stream_tx: broadcast::Sender<AgentEvent>,
    pub pending_messages: Vec<PendingMessage>,
    pub policy_rules: Vec<crate::policy::PolicyRule>,
}

pub type SharedRegistry = Arc<RwLock<Registry>>;

pub struct Registry {
    sessions: HashMap<String, SessionHandle>,
    machine_commands: HashMap<String, Vec<MachineCommand>>,
}

#[allow(dead_code)]
impl Registry {
    pub fn new() -> Self {
        Self { sessions: HashMap::new(), machine_commands: HashMap::new() }
    }

    pub fn shared() -> SharedRegistry {
        Arc::new(RwLock::new(Self::new()))
    }

    pub fn register(&mut self, session: Session) -> broadcast::Sender<AgentEvent> {
        // Reuse an existing stream_tx on re-registration so current WS
        // subscribers don't see the broadcast channel close and lose their
        // stream until they manually reopen the pane.
        let stream_tx = self
            .sessions
            .get(&session.id)
            .map_or_else(|| broadcast::channel(256).0, |h| h.stream_tx.clone());
        let tx = stream_tx.clone();
        self.sessions.insert(
            session.id.clone(),
            SessionHandle {
                session,
                last_heartbeat: Instant::now(),
                token_usage: TokenUsage::default(),
                stream_tx,
                pending_messages: Vec::new(),
                policy_rules: Vec::new(),
            },
        );
        tx
    }

    pub fn deregister(&mut self, id: &str) -> Option<Session> {
        self.sessions.remove(id).map(|h| h.session)
    }

    pub fn get(&self, id: &str) -> Option<&SessionHandle> {
        self.sessions.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut SessionHandle> {
        self.sessions.get_mut(id)
    }

    pub fn list(&self) -> Vec<&SessionHandle> {
        self.sessions.values().collect()
    }

    pub fn update_heartbeat(&mut self, id: &str, tokens_in: u64, tokens_out: u64, cost_usd: f64) {
        if let Some(handle) = self.sessions.get_mut(id) {
            handle.last_heartbeat = Instant::now();
            handle.session.last_heartbeat = Utc::now();
            handle.session.status = SessionStatus::Active;
            handle.token_usage.tokens_in = tokens_in;
            handle.token_usage.tokens_out = tokens_out;
            handle.token_usage.cost_usd = cost_usd;
        }
    }

    pub fn update_token_usage(&mut self, id: &str, tokens_in: u64, tokens_out: u64, cost_usd: f64) {
        if let Some(handle) = self.sessions.get_mut(id) {
            handle.token_usage.tokens_in += tokens_in;
            handle.token_usage.tokens_out += tokens_out;
            handle.token_usage.cost_usd += cost_usd;
            handle.last_heartbeat = Instant::now();
            handle.session.last_heartbeat = Utc::now();
        }
    }

    /// Record activity for `id` — refresh the heartbeat and flip the session
    /// to `Active` if it was `New` or `Inactive`. Returns the new status if
    /// it changed, so the caller can broadcast a status change. Returns
    /// `None` if the session is unknown or was already `Active`.
    pub fn touch(&mut self, id: &str) -> Option<SessionStatus> {
        let handle = self.sessions.get_mut(id)?;
        handle.last_heartbeat = Instant::now();
        handle.session.last_heartbeat = Utc::now();
        if handle.session.status == SessionStatus::Active {
            None
        } else {
            handle.session.status = SessionStatus::Active;
            Some(SessionStatus::Active)
        }
    }

    /// Demote stale sessions to `Inactive`:
    /// - `Active` when idle longer than `inactive_after_secs`.
    /// - `New` when no turn arrives within `NEW_TTL_SECS` of registration
    ///   (prevents the "new" pill from lingering on sessions that never
    ///   produced activity). `Inactive` stays `Inactive` until revived.
    ///
    /// Returns the list of session ids that were just demoted.
    pub fn mark_stale(&mut self, inactive_after_secs: u64) -> Vec<String> {
        const NEW_TTL_SECS: u64 = 60;
        let now = Instant::now();
        let mut demoted = Vec::new();
        for handle in self.sessions.values_mut() {
            let elapsed = now.duration_since(handle.last_heartbeat).as_secs();
            let should_demote = match handle.session.status {
                SessionStatus::Active => elapsed > inactive_after_secs,
                SessionStatus::New => elapsed > NEW_TTL_SECS,
                SessionStatus::Inactive => false,
            };
            if should_demote {
                handle.session.status = SessionStatus::Inactive;
                demoted.push(handle.session.id.clone());
            }
        }
        demoted
    }

    pub fn subscribe(&self, id: &str) -> Option<broadcast::Receiver<AgentEvent>> {
        self.sessions.get(id).map(|h| h.stream_tx.subscribe())
    }

    pub fn queue_message(&mut self, session_id: &str, content: String) -> Option<Uuid> {
        let handle = self.sessions.get_mut(session_id)?;
        let msg = PendingMessage { id: Uuid::new_v4(), content, created_at: Utc::now() };
        let id = msg.id;
        handle.pending_messages.push(msg);
        Some(id)
    }

    pub fn take_pending_messages(&mut self, session_id: &str) -> Vec<PendingMessage> {
        self.sessions
            .get_mut(session_id)
            .map(|h| std::mem::take(&mut h.pending_messages))
            .unwrap_or_default()
    }

    pub fn set_policy(&mut self, session_id: &str, rules: Vec<crate::policy::PolicyRule>) {
        if let Some(handle) = self.sessions.get_mut(session_id) {
            handle.policy_rules = rules;
        }
    }

    pub fn queue_machine_command(
        &mut self,
        machine_id: &str,
        cmd_type: &str,
        payload: serde_json::Value,
    ) -> Uuid {
        let cmd = MachineCommand {
            id: Uuid::new_v4(),
            command_type: cmd_type.to_string(),
            payload,
            created_at: chrono::Utc::now(),
        };
        let id = cmd.id;
        self.machine_commands.entry(machine_id.to_string()).or_default().push(cmd);
        id
    }

    pub fn take_machine_commands(&mut self, machine_id: &str) -> Vec<MachineCommand> {
        self.machine_commands.remove(machine_id).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session(id: &str, parent: Option<String>) -> Session {
        Session {
            id: id.to_string(),
            parent_id: parent,
            account_id: None,
            machine_id: "test".into(),
            working_dir: "/tmp".into(),
            status: SessionStatus::Active,
            registered_at: Utc::now(),
            last_heartbeat: Utc::now(),
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn register_and_list() {
        let mut reg = Registry::new();
        let id = "claude-session-abc";
        reg.register(make_session(id, None));
        assert_eq!(reg.list().len(), 1);
        assert!(reg.get(id).is_some());
    }

    #[test]
    fn deregister_removes_session() {
        let mut reg = Registry::new();
        let id = "claude-session-abc";
        reg.register(make_session(id, None));
        reg.deregister(id);
        assert!(reg.get(id).is_none());
        assert_eq!(reg.list().len(), 0);
    }

    #[test]
    fn subscribe_gets_broadcast_receiver() {
        let mut reg = Registry::new();
        let id = "claude-session-abc";
        let tx = reg.register(make_session(id, None));
        let mut rx = reg.subscribe(id).unwrap();

        tx.send(AgentEvent::Text { content: "hello".into(), ts: 0 }).unwrap();

        let event = rx.try_recv().unwrap();
        match event {
            AgentEvent::Text { content, .. } => assert_eq!(content, "hello"),
            _ => panic!("unexpected event"),
        }
    }

    #[test]
    fn queue_and_take_pending_messages() {
        let mut reg = Registry::new();
        let id = "claude-session-abc";
        reg.register(make_session(id, None));

        let msg_id = reg.queue_message(id, "hello".into());
        assert!(msg_id.is_some());

        let msg_id2 = reg.queue_message(id, "world".into());
        assert!(msg_id2.is_some());

        let messages = reg.take_pending_messages(id);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "hello");
        assert_eq!(messages[1].content, "world");

        // Second take returns empty
        let messages2 = reg.take_pending_messages(id);
        assert!(messages2.is_empty());
    }

    #[test]
    fn queue_message_for_nonexistent_session_returns_none() {
        let mut reg = Registry::new();
        let result = reg.queue_message("nonexistent", "orphan".into());
        assert!(result.is_none());
    }

    #[test]
    fn take_pending_from_nonexistent_session_returns_empty() {
        let mut reg = Registry::new();
        let messages = reg.take_pending_messages("nonexistent");
        assert!(messages.is_empty());
    }

    #[test]
    fn mark_stale_demotes_active_to_inactive() {
        let mut reg = Registry::new();
        let id = "claude-session-abc";
        reg.register(make_session(id, None));

        // Fresh session — stays Active.
        let demoted = reg.mark_stale(90);
        assert!(demoted.is_empty());
        assert_eq!(reg.get(id).unwrap().session.status, SessionStatus::Active);

        // Backdate the heartbeat past the inactive window.
        if let Some(handle) = reg.get_mut(id) {
            handle.last_heartbeat =
                std::time::Instant::now().checked_sub(std::time::Duration::from_secs(100)).unwrap();
        }
        let demoted = reg.mark_stale(90);
        assert_eq!(demoted, vec![id.to_string()]);
        assert_eq!(reg.get(id).unwrap().session.status, SessionStatus::Inactive);

        // Inactive stays inactive; mark_stale is idempotent.
        let demoted = reg.mark_stale(90);
        assert!(demoted.is_empty());
        assert_eq!(reg.get(id).unwrap().session.status, SessionStatus::Inactive);
    }

    #[test]
    fn touch_revives_inactive_session() {
        let mut reg = Registry::new();
        let id = "claude-session-abc";
        let mut s = make_session(id, None);
        s.status = SessionStatus::Inactive;
        reg.register(s);

        let changed = reg.touch(id);
        assert_eq!(changed, Some(SessionStatus::Active));
        assert_eq!(reg.get(id).unwrap().session.status, SessionStatus::Active);

        // Touching an already-active session is a no-op for status.
        assert!(reg.touch(id).is_none());
    }

    #[test]
    fn touch_promotes_new_to_active() {
        let mut reg = Registry::new();
        let id = "claude-session-abc";
        let mut s = make_session(id, None);
        s.status = SessionStatus::New;
        reg.register(s);

        let changed = reg.touch(id);
        assert_eq!(changed, Some(SessionStatus::Active));
    }

    #[test]
    fn update_heartbeat_refreshes_session() {
        let mut reg = Registry::new();
        let id = "claude-session-abc";
        reg.register(make_session(id, None));

        reg.update_heartbeat(id, 100, 50, 0.01);

        let handle = reg.get(id).unwrap();
        assert_eq!(handle.token_usage.tokens_in, 100);
        assert_eq!(handle.token_usage.tokens_out, 50);
        assert_eq!(handle.session.status, SessionStatus::Active);
    }

    #[test]
    fn update_token_usage_accumulates() {
        let mut reg = Registry::new();
        let id = "claude-session-abc";
        reg.register(make_session(id, None));

        reg.update_token_usage(id, 100, 50, 0.01);
        reg.update_token_usage(id, 200, 100, 0.02);

        let handle = reg.get(id).unwrap();
        assert_eq!(handle.token_usage.tokens_in, 300);
        assert_eq!(handle.token_usage.tokens_out, 150);
    }

    #[test]
    fn set_policy_stores_rules() {
        let mut reg = Registry::new();
        let id = "claude-session-abc";
        reg.register(make_session(id, None));

        let rules = vec![crate::policy::PolicyRule {
            tool: "Bash".into(),
            action: crate::policy::PolicyAction::Deny,
            pattern: Some("rm -rf".into()),
            reason: Some("dangerous".into()),
        }];
        reg.set_policy(id, rules);

        let handle = reg.get(id).unwrap();
        assert_eq!(handle.policy_rules.len(), 1);
        assert_eq!(handle.policy_rules[0].tool, "Bash");
    }

    #[test]
    fn re_register_same_session_id_replaces() {
        let mut reg = Registry::new();
        let id = "claude-session-abc";
        reg.register(make_session(id, None));
        reg.queue_message(id, "old message".into());

        // Re-register with the same ID (simulates server restart + re-registration)
        reg.register(make_session(id, None));
        assert_eq!(reg.list().len(), 1);
        // Pending messages are cleared (fresh handle)
        let messages = reg.take_pending_messages(id);
        assert!(messages.is_empty());
    }
}
