use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use cctui_proto::models::{Session, SessionStatus, TokenUsage};
use cctui_proto::ws::AgentEvent;
use chrono::Utc;
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

#[derive(Debug)]
pub struct SessionHandle {
    pub session: Session,
    #[allow(dead_code)]
    pub last_heartbeat: Instant,
    #[allow(dead_code)]
    pub token_usage: TokenUsage,
    pub stream_tx: broadcast::Sender<AgentEvent>,
}

pub type SharedRegistry = Arc<RwLock<Registry>>;

pub struct Registry {
    sessions: HashMap<Uuid, SessionHandle>,
}

#[allow(dead_code)]
impl Registry {
    pub fn new() -> Self {
        Self { sessions: HashMap::new() }
    }

    pub fn shared() -> SharedRegistry {
        Arc::new(RwLock::new(Self::new()))
    }

    pub fn register(&mut self, session: Session) -> broadcast::Sender<AgentEvent> {
        let (stream_tx, _) = broadcast::channel(256);
        let tx = stream_tx.clone();
        self.sessions.insert(
            session.id,
            SessionHandle {
                session,
                last_heartbeat: Instant::now(),
                token_usage: TokenUsage::default(),
                stream_tx,
            },
        );
        tx
    }

    pub fn deregister(&mut self, id: &Uuid) -> Option<Session> {
        self.sessions.remove(id).map(|h| h.session)
    }

    pub fn get(&self, id: &Uuid) -> Option<&SessionHandle> {
        self.sessions.get(id)
    }

    pub fn get_mut(&mut self, id: &Uuid) -> Option<&mut SessionHandle> {
        self.sessions.get_mut(id)
    }

    pub fn list(&self) -> Vec<&SessionHandle> {
        self.sessions.values().collect()
    }

    pub fn update_heartbeat(&mut self, id: &Uuid, tokens_in: u64, tokens_out: u64, cost_usd: f64) {
        if let Some(handle) = self.sessions.get_mut(id) {
            handle.last_heartbeat = Instant::now();
            handle.session.last_heartbeat = Utc::now();
            handle.session.status = SessionStatus::Active;
            handle.token_usage.tokens_in = tokens_in;
            handle.token_usage.tokens_out = tokens_out;
            handle.token_usage.cost_usd = cost_usd;
        }
    }

    pub fn mark_stale(
        &mut self,
        disconnected_after_secs: u64,
        terminated_after_secs: u64,
    ) -> Vec<Uuid> {
        let now = Instant::now();
        let mut terminated = Vec::new();

        for handle in self.sessions.values_mut() {
            let elapsed = now.duration_since(handle.last_heartbeat).as_secs();
            match handle.session.status {
                SessionStatus::Active | SessionStatus::Idle | SessionStatus::Registering => {
                    if elapsed > terminated_after_secs {
                        handle.session.status = SessionStatus::Terminated;
                        terminated.push(handle.session.id);
                    } else if elapsed > disconnected_after_secs {
                        handle.session.status = SessionStatus::Disconnected;
                    }
                }
                SessionStatus::Disconnected => {
                    if elapsed > terminated_after_secs {
                        handle.session.status = SessionStatus::Terminated;
                        terminated.push(handle.session.id);
                    }
                }
                SessionStatus::Terminated => {}
            }
        }
        terminated
    }

    pub fn subscribe(&self, id: &Uuid) -> Option<broadcast::Receiver<AgentEvent>> {
        self.sessions.get(id).map(|h| h.stream_tx.subscribe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session(id: Uuid, parent: Option<Uuid>) -> Session {
        Session {
            id,
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
        let id = Uuid::new_v4();
        reg.register(make_session(id, None));
        assert_eq!(reg.list().len(), 1);
        assert!(reg.get(&id).is_some());
    }

    #[test]
    fn deregister_removes_session() {
        let mut reg = Registry::new();
        let id = Uuid::new_v4();
        reg.register(make_session(id, None));
        reg.deregister(&id);
        assert!(reg.get(&id).is_none());
        assert_eq!(reg.list().len(), 0);
    }

    #[test]
    fn subscribe_gets_broadcast_receiver() {
        let mut reg = Registry::new();
        let id = Uuid::new_v4();
        let tx = reg.register(make_session(id, None));
        let mut rx = reg.subscribe(&id).unwrap();

        tx.send(AgentEvent::Text { content: "hello".into(), ts: 0 }).unwrap();

        let event = rx.try_recv().unwrap();
        match event {
            AgentEvent::Text { content, .. } => assert_eq!(content, "hello"),
            _ => panic!("unexpected event"),
        }
    }
}
