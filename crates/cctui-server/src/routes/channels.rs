use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::state::AppState;

// --- Channel store (in-memory, channels are ephemeral) ---

#[derive(Debug, Clone)]
pub struct PendingChannel {
    #[allow(dead_code)]
    pub channel_id: String,
    pub machine_id: String,
    pub ppid: u32,
    #[allow(dead_code)]
    pub cwd: String,
    pub registered_at: DateTime<Utc>,
    /// Filled in when `SessionStart` hook matches this channel
    pub session_info: Option<SessionAssignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAssignment {
    pub session_id: String,
    pub transcript_path: String,
    pub model: String,
}

pub type SharedChannelStore = Arc<RwLock<ChannelStore>>;

pub struct ChannelStore {
    /// Channels indexed by `channel_id`
    channels: HashMap<String, PendingChannel>,
    /// Unmatched hook payloads waiting for a channel to claim them
    pending_hooks: Vec<HookPayload>,
}

#[derive(Debug, Clone)]
struct HookPayload {
    pub machine_id: String,
    pub ppid: u32,
    pub session_id: String,
    pub transcript_path: String,
    pub model: String,
    pub received_at: DateTime<Utc>,
}

impl ChannelStore {
    pub fn new() -> Self {
        Self { channels: HashMap::new(), pending_hooks: Vec::new() }
    }

    pub fn shared() -> SharedChannelStore {
        Arc::new(RwLock::new(Self::new()))
    }

    /// Register a new channel. If a matching hook payload already arrived, match immediately.
    pub fn register_channel(&mut self, machine_id: String, ppid: u32, cwd: String) -> String {
        let channel_id = Uuid::new_v4().to_string();

        // Check if a hook payload already arrived for this (machine_id, ppid)
        let assignment = self.try_match_hook(&machine_id, ppid);

        self.channels.insert(
            channel_id.clone(),
            PendingChannel {
                channel_id: channel_id.clone(),
                machine_id,
                ppid,
                cwd,
                registered_at: Utc::now(),
                session_info: assignment,
            },
        );
        channel_id
    }

    /// Get session assignment for a channel. Returns None if not yet matched.
    pub fn get_assignment(&self, channel_id: &str) -> Option<&SessionAssignment> {
        self.channels.get(channel_id).and_then(|c| c.session_info.as_ref())
    }

    /// Receive a `SessionStart` hook payload. Try to match to a pending channel.
    pub fn receive_hook(
        &mut self,
        machine_id: String,
        ppid: u32,
        session_id: String,
        transcript_path: String,
        model: String,
    ) {
        // Try to match to an existing channel
        let matched = self
            .channels
            .values_mut()
            .find(|c| c.machine_id == machine_id && c.ppid == ppid && c.session_info.is_none());

        if let Some(channel) = matched {
            channel.session_info = Some(SessionAssignment { session_id, transcript_path, model });
        } else {
            // No channel registered yet — queue for later matching
            self.pending_hooks.push(HookPayload {
                machine_id,
                ppid,
                session_id,
                transcript_path,
                model,
                received_at: Utc::now(),
            });
        }
    }

    /// Try to match a channel against queued hook payloads
    fn try_match_hook(&mut self, machine_id: &str, ppid: u32) -> Option<SessionAssignment> {
        let idx =
            self.pending_hooks.iter().position(|h| h.machine_id == machine_id && h.ppid == ppid)?;
        let hook = self.pending_hooks.remove(idx);
        Some(SessionAssignment {
            session_id: hook.session_id,
            transcript_path: hook.transcript_path,
            model: hook.model,
        })
    }

    /// Clean up channels older than `max_age_secs`
    pub fn reap_stale(&mut self, max_age_secs: i64) {
        let cutoff = Utc::now() - chrono::Duration::seconds(max_age_secs);
        self.channels.retain(|_, c| c.registered_at > cutoff);
        self.pending_hooks.retain(|h| h.received_at > cutoff);
    }
}

// --- Request/Response types ---

#[derive(Debug, Deserialize)]
pub struct RegisterChannelRequest {
    pub machine_id: String,
    pub ppid: u32,
    pub cwd: String,
}

#[derive(Debug, Serialize)]
pub struct RegisterChannelResponse {
    pub channel_id: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SessionPollResponse {
    Waiting,
    Matched { session_id: String, transcript_path: String, model: String },
}

#[derive(Debug, Deserialize)]
pub struct SessionStartHookPayload {
    pub session_id: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub ppid: Option<u32>,
    /// `machine_id` sent by the hook script
    #[serde(default)]
    pub machine_id: Option<String>,
}

// --- Handlers ---

pub async fn register_channel(
    State(state): State<AppState>,
    Json(req): Json<RegisterChannelRequest>,
) -> (StatusCode, Json<RegisterChannelResponse>) {
    let channel_id =
        state.channel_store.write().await.register_channel(req.machine_id, req.ppid, req.cwd);
    tracing::info!(channel_id = %channel_id, "channel registered");
    (StatusCode::CREATED, Json(RegisterChannelResponse { channel_id }))
}

pub async fn poll_session(
    State(state): State<AppState>,
    axum::extract::Path(channel_id): axum::extract::Path<String>,
) -> Json<SessionPollResponse> {
    let store = state.channel_store.read().await;
    store.get_assignment(&channel_id).map_or(Json(SessionPollResponse::Waiting), |assignment| {
        Json(SessionPollResponse::Matched {
            session_id: assignment.session_id.clone(),
            transcript_path: assignment.transcript_path.clone(),
            model: assignment.model.clone(),
        })
    })
}

pub async fn session_start_hook(
    State(state): State<AppState>,
    Json(req): Json<SessionStartHookPayload>,
) -> StatusCode {
    let machine_id = req.machine_id.unwrap_or_default();
    let ppid = req.ppid.unwrap_or(0);

    tracing::info!(
        session_id = %req.session_id,
        machine_id = %machine_id,
        ppid = ppid,
        "SessionStart hook received"
    );

    if ppid == 0 || machine_id.is_empty() {
        tracing::warn!("SessionStart hook missing ppid or machine_id, cannot match to channel");
        return StatusCode::BAD_REQUEST;
    }

    state.channel_store.write().await.receive_hook(
        machine_id,
        ppid,
        req.session_id,
        req.transcript_path.unwrap_or_default(),
        req.model.unwrap_or_default(),
    );

    StatusCode::OK
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_hook_matches() {
        let mut store = ChannelStore::new();
        let cid = store.register_channel("machine-1".into(), 1234, "/tmp".into());
        assert!(store.get_assignment(&cid).is_none());

        store.receive_hook(
            "machine-1".into(),
            1234,
            "session-abc".into(),
            "/path/t.jsonl".into(),
            "opus".into(),
        );
        let assignment = store.get_assignment(&cid).unwrap();
        assert_eq!(assignment.session_id, "session-abc");
        assert_eq!(assignment.transcript_path, "/path/t.jsonl");
    }

    #[test]
    fn hook_then_register_matches() {
        let mut store = ChannelStore::new();
        store.receive_hook(
            "machine-1".into(),
            1234,
            "session-abc".into(),
            "/path/t.jsonl".into(),
            "opus".into(),
        );

        let cid = store.register_channel("machine-1".into(), 1234, "/tmp".into());
        let assignment = store.get_assignment(&cid).unwrap();
        assert_eq!(assignment.session_id, "session-abc");
    }

    #[test]
    fn different_ppid_no_match() {
        let mut store = ChannelStore::new();
        let cid = store.register_channel("machine-1".into(), 1234, "/tmp".into());
        store.receive_hook(
            "machine-1".into(),
            5678,
            "session-other".into(),
            "/path".into(),
            String::new(),
        );
        assert!(store.get_assignment(&cid).is_none());
    }

    #[test]
    fn different_machine_no_match() {
        let mut store = ChannelStore::new();
        let cid = store.register_channel("machine-1".into(), 1234, "/tmp".into());
        store.receive_hook(
            "machine-2".into(),
            1234,
            "session-other".into(),
            "/path".into(),
            String::new(),
        );
        assert!(store.get_assignment(&cid).is_none());
    }

    #[test]
    fn multiple_channels_match_correctly() {
        let mut store = ChannelStore::new();
        let cid1 = store.register_channel("m".into(), 100, "/a".into());
        let cid2 = store.register_channel("m".into(), 200, "/b".into());

        store.receive_hook("m".into(), 200, "session-b".into(), "/b.jsonl".into(), String::new());
        store.receive_hook("m".into(), 100, "session-a".into(), "/a.jsonl".into(), String::new());

        assert_eq!(store.get_assignment(&cid1).unwrap().session_id, "session-a");
        assert_eq!(store.get_assignment(&cid2).unwrap().session_id, "session-b");
    }

    #[test]
    fn reap_stale_removes_old_entries() {
        let mut store = ChannelStore::new();
        store.register_channel("m".into(), 100, "/tmp".into());
        assert_eq!(store.channels.len(), 1);
        // With max_age 0, everything is stale
        store.reap_stale(0);
        assert_eq!(store.channels.len(), 0);
    }
}
