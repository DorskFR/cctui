use dashmap::DashMap;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthConfig;
use crate::bus::Bus;
use crate::commands::{CommandWaiters, PendingCommand};
use crate::config::Config;
use crate::registry::SharedRegistry;

/// Router state as a domain crate sees it. The host implements this (and each
/// domain's own context trait on top of it), so no domain names the host state.
///
/// Only state read by more than one domain lives here; a domain declares the
/// rest on its own `XContext: KernelContext`.
pub trait KernelContext: Clone + Send + Sync + 'static {
    fn pool(&self) -> &PgPool;
    fn bus(&self) -> &Bus;
    fn auth_config(&self) -> &AuthConfig;
    fn config(&self) -> &Config;
    fn http_client(&self) -> &reqwest::Client;
    fn registry(&self) -> &SharedRegistry;
    /// Commands awaiting their daemon `CommandResult`, keyed by `command_id`.
    fn pending_commands(&self) -> &DashMap<Uuid, PendingCommand>;
    fn command_waiters(&self) -> &CommandWaiters;
    /// Read-through cache over `session_spawn_capabilities`.
    fn spawn_capabilities(&self) -> &DashMap<String, cctui_proto::api::SpawnCapability>;
    fn session_usd_budgets(&self) -> &DashMap<String, f64>;
}
