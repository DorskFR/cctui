use std::sync::Arc;

use cctui_proto::ws::{DaemonFrameDown, ServerEvent};
use dashmap::DashMap;
use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use crate::archive_store::ArchiveStore;
use crate::auth::AuthConfig;
use crate::config::Config;
use crate::dispatchers::Registry as DispatcherRegistry;
use crate::registry::SharedRegistry;
use crate::routes::permissions::SharedPermissionStore;
use crate::skill_store::SkillStore;

/// Per-machine outbound channel into the connected daemon's WS task.
/// Clients dispatch `DaemonFrameDown` commands by looking up the target
/// machine here. Absent entry = daemon offline; command should be written
/// to the `commands` table for replay on reconnect (post-v0).
pub type DaemonConnections = Arc<DashMap<Uuid, mpsc::Sender<DaemonFrameDown>>>;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub registry: SharedRegistry,
    pub permission_store: SharedPermissionStore,
    /// Broadcast channel for server-initiated TUI events (e.g. permission requests).
    pub tui_tx: broadcast::Sender<ServerEvent>,
    #[allow(dead_code)]
    pub auth_config: AuthConfig,
    pub archive: Arc<ArchiveStore>,
    pub skills: Arc<SkillStore>,
    pub daemon_connections: DaemonConnections,
    pub dispatchers: Arc<DispatcherRegistry>,
}
