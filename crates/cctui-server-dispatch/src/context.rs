//! Execution state as dispatch reads it, implemented by the host.
use std::sync::Arc;

use cctui_proto::models::MachineLiveness;
use cctui_server_kernel::auth::AuthConfig;
use cctui_server_kernel::bus::Bus;
use cctui_server_kernel::commands::{CommandWaiters, PendingCommand};
use dashmap::DashMap;
use sqlx::PgPool;
use uuid::Uuid;

use crate::dispatchers::Registry as DispatcherRegistry;

pub trait DispatchCore: Clone + Send + Sync + 'static {
    fn pool(&self) -> &PgPool;
    fn bus(&self) -> &Bus;
    fn auth_config(&self) -> &AuthConfig;
    fn http_client(&self) -> &reqwest::Client;
    fn pending_commands(&self) -> &DashMap<Uuid, PendingCommand>;
    fn command_waiters(&self) -> &CommandWaiters;
    fn spawn_capabilities(&self) -> &DashMap<String, cctui_proto::api::SpawnCapability>;
    fn session_usd_budgets(&self) -> &DashMap<String, f64>;
    fn dispatcher_registry(&self) -> &Arc<DispatcherRegistry>;
    fn dispatcher_liveness(&self) -> &DashMap<Uuid, MachineLiveness>;
    fn machine_liveness(&self) -> &DashMap<Uuid, MachineLiveness>;
}
