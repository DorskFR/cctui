use std::collections::HashMap;
use std::sync::Arc;

use cctui_proto::diagnose::{CodexProtocolError, CodexRpcFrame, CodexStderrLine};
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

use super::config::AppServerConfig;

/// Per-session commands routed from the adapter-level command pump.
#[derive(Debug, Clone)]
pub enum SessionCommand {
    /// Answer a pending approval (`request_id` came from the emitted
    /// `PermissionRequest`).
    Permission { request_id: String, allow: bool },
    /// Start a new turn with user text. `command_id` correlates the
    /// `turn/start` (or `turn/steer`) JSON-RPC outcome back to an
    /// [`AdapterEvent::CommandResult`].
    Send { text: String, command_id: Option<Uuid> },
    /// Persist the display name into Codex's thread metadata.
    Rename { name: String },
    /// Interrupt the in-flight turn and terminate the session. `signal` is
    /// the requested POSIX signal: `Some(15)` (SIGTERM) for a graceful stop
    /// that lets codex flush its rollout file; anything else (incl. `None`)
    /// falls back to an immediate SIGKILL.
    Kill { signal: Option<i32> },
    /// Interrupt the in-flight turn but KEEP the session alive:
    /// sends `turn/interrupt` WITHOUT terminating the app-server, so the
    /// thread stays resumable. Distinct from `Kill`, which interrupts *and*
    /// terminates the child. `command_id` correlates the `turn/interrupt`
    /// JSON-RPC outcome back to an [`AdapterEvent::CommandResult`].
    Interrupt { command_id: Option<Uuid> },
    /// Change the model and/or reasoning effort of the running thread in place:
    /// records the override so the next `turn/start` carries it (a
    /// stable per-turn override codex promotes to the later default),
    /// and echoes the resolved values back via [`AdapterEvent::Status`] so the
    /// webui chip updates live. `command_id` correlates the outcome back as an
    /// [`AdapterEvent::CommandResult`].
    SetModel { model: Option<String>, effort: Option<String>, command_id: Option<Uuid> },
    /// Gather a point-in-time snapshot of the live driver's internal state for
    /// the adapter-neutral diagnose report and return it on `reply`.
    Diagnose { reply: mpsc::Sender<CodexLiveSnapshot> },
}

impl SessionCommand {
    #[must_use]
    pub const fn is_resumable(&self) -> bool {
        matches!(self, Self::Send { .. } | Self::Rename { .. } | Self::SetModel { .. })
    }

    #[must_use]
    pub const fn command_id(&self) -> Option<Uuid> {
        match self {
            Self::Send { command_id, .. }
            | Self::SetModel { command_id, .. }
            | Self::Interrupt { command_id } => *command_id,
            _ => None,
        }
    }
}

/// Point-in-time snapshot of a live codex session's internal driver state,
/// gathered on demand for the diagnose report.
#[derive(Debug, Clone, Default)]
pub struct CodexLiveSnapshot {
    pub codex_version: Option<String>,
    pub pid: Option<u32>,
    pub active_turn_id: Option<String>,
    pub pending_rpc_methods: Vec<String>,
    pub protocol_errors: Vec<CodexProtocolError>,
    pub stderr_tail: Vec<CodexStderrLine>,
    pub rpc_tail: Vec<CodexRpcFrame>,
    pub rollout_path: Option<String>,
    pub rollout_size_bytes: Option<u64>,
}

/// Live command registry: `local_id` → command sender for the owning app-server
/// task. Senders disappear when the app-server exits; the durable
/// [`SessionRegistry`] below stays so a later reply can revive the thread.
pub type LiveSessionRegistry = Arc<Mutex<HashMap<String, mpsc::Sender<SessionCommand>>>>;

/// Durable-in-daemon metadata for cctui-owned Codex threads. This is not a
/// process handle; it is the minimum launch context needed to call
/// `thread/resume` after a clean app-server exit. The log-tail also
/// uses this map as the ownership set so it does not double-ingest these
/// rollout files while they are hibernated.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub cfg: AppServerConfig,
    pub cwd: String,
    pub name: Option<String>,
    /// Resolved launch-time env — chiefly the gateway-routing
    /// credential pulled from the server's durable `sessions.account_id`
    /// binding. Stored so a resume relaunches the codex app-server with the
    /// same gateway env instead of starting env-less and 401ing (the codex
    /// analogue of the claude cold-launch bug).
    pub env: std::collections::BTreeMap<String, String>,
    /// Whether this thread's launch declared the `CctuiAgent` relay. Persisted
    /// because the relay map is process-local and a resume carries no
    /// capability to re-derive the decision from.
    pub spawn_relay: bool,
}

/// `local_id` → cctui-owned Codex thread metadata.
pub type SessionRegistry = Arc<Mutex<HashMap<String, SessionRecord>>>;

// `Resume` carries a full `SessionRecord` (including the launch env);
// the size gap to the unit `Delivered`/`Missing` variants is intrinsic and the
// value is short-lived (built, matched, dropped per command), so boxing it
// would add an allocation for no real benefit.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum RouteAction {
    Delivered,
    Resume { record: SessionRecord, command: SessionCommand },
    Missing,
}

/// Try the live sender first. If it is gone or closed, fall back to the
/// durable Codex thread record so the caller can spawn a resume driver.
pub async fn route_or_prepare_resume(
    live: &LiveSessionRegistry,
    sessions: &SessionRegistry,
    local_id: &str,
    command: SessionCommand,
) -> RouteAction {
    let sender = live.lock().await.get(local_id).cloned();
    if let Some(tx) = sender {
        if tx.send(command.clone()).await.is_ok() {
            return RouteAction::Delivered;
        }
        live.lock().await.remove(local_id);
        tracing::warn!(%local_id, "codex: live session command channel closed");
    }

    sessions
        .lock()
        .await
        .get(local_id)
        .cloned()
        .map_or(RouteAction::Missing, |record| RouteAction::Resume { record, command })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_model_is_resumable() {
        assert!(
            SessionCommand::SetModel { model: Some("m".into()), effort: None, command_id: None }
                .is_resumable()
        );
    }

    #[tokio::test]
    async fn route_delivers_to_live_sender() {
        let live = LiveSessionRegistry::default();
        let registry = SessionRegistry::default();
        let (tx, mut rx) = mpsc::channel(1);
        live.lock().await.insert("tid".to_owned(), tx);
        registry.lock().await.insert(
            "tid".to_owned(),
            SessionRecord {
                cfg: AppServerConfig::default(),
                cwd: "/tmp".to_owned(),
                name: Some("n".to_owned()),
                env: std::collections::BTreeMap::new(),
                spawn_relay: false,
            },
        );

        let action = route_or_prepare_resume(
            &live,
            &registry,
            "tid",
            SessionCommand::Send { text: "hi".to_owned(), command_id: None },
        )
        .await;
        assert!(matches!(action, RouteAction::Delivered));
        assert!(matches!(rx.recv().await, Some(SessionCommand::Send { text, .. }) if text == "hi"));
    }

    #[tokio::test]
    async fn route_prepares_resume_when_live_sender_is_closed() {
        let live = LiveSessionRegistry::default();
        let registry = SessionRegistry::default();
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        live.lock().await.insert("tid".to_owned(), tx);
        registry.lock().await.insert(
            "tid".to_owned(),
            SessionRecord {
                cfg: AppServerConfig::default(),
                cwd: "/repo".to_owned(),
                name: Some("stale".to_owned()),
                env: std::collections::BTreeMap::new(),
                spawn_relay: false,
            },
        );

        let action = route_or_prepare_resume(
            &live,
            &registry,
            "tid",
            SessionCommand::Rename { name: "new".to_owned() },
        )
        .await;
        match action {
            RouteAction::Resume { record, command: SessionCommand::Rename { name } } => {
                assert_eq!(record.cwd, "/repo");
                assert_eq!(record.name.as_deref(), Some("stale"));
                assert_eq!(name, "new");
            }
            other => panic!("expected resume action, got {other:?}"),
        }
        assert!(!live.lock().await.contains_key("tid"));
    }

    #[tokio::test]
    async fn route_missing_without_durable_record() {
        let live = LiveSessionRegistry::default();
        let registry = SessionRegistry::default();
        let action = route_or_prepare_resume(
            &live,
            &registry,
            "missing",
            SessionCommand::Send { text: "hi".to_owned(), command_id: None },
        )
        .await;
        assert!(matches!(action, RouteAction::Missing));
    }

    #[test]
    fn command_id_only_on_correlated_commands() {
        let cid = Uuid::new_v4();
        assert_eq!(
            SessionCommand::SetModel { model: None, effort: None, command_id: Some(cid) }
                .command_id(),
            Some(cid)
        );
        assert_eq!(SessionCommand::Interrupt { command_id: Some(cid) }.command_id(), Some(cid));
        assert_eq!(
            SessionCommand::Send { text: String::new(), command_id: None }.command_id(),
            None
        );
        assert_eq!(SessionCommand::Kill { signal: None }.command_id(), None);
    }
}
