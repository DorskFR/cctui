use std::sync::Arc;

use cctui_proto::ws::ServerEvent;
use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::archive_store::ArchiveStore;
use crate::auth::AuthConfig;
use crate::config::Config;
use crate::registry::SharedRegistry;
use crate::routes::channels::SharedChannelStore;
use crate::routes::permissions::SharedPermissionStore;
use crate::skill_store::SkillStore;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub registry: SharedRegistry,
    pub channel_store: SharedChannelStore,
    pub permission_store: SharedPermissionStore,
    /// Broadcast channel for server-initiated TUI events (e.g. permission requests).
    pub tui_tx: broadcast::Sender<ServerEvent>,
    #[allow(dead_code)]
    pub auth_config: AuthConfig,
    pub archive: Arc<ArchiveStore>,
    pub skills: Arc<SkillStore>,
}
