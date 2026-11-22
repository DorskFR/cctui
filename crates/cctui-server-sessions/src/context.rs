use std::sync::{Arc, Mutex};

use cctui_server_kernel::bus::Bus;
use cctui_server_kernel::context::KernelContext;
use dashmap::DashMap;
use sqlx::PgPool;
use uuid::Uuid;

use crate::bandwidth_watch::{ConnectTracker, DivergenceTracker, EvictionTracker};
use crate::ports::{
    SessionsAccounts, SessionsBlobs, SessionsDispatch, SessionsExtras, SessionsHost,
    SessionsNotify, SessionsSearch,
};
use crate::routes::codex_models::{CachedCatalog, CachedVersion};
use crate::routes::permissions::SharedPermissionStore;

/// What daemon reconciliation and transcript checkpoints read; also satisfied by
/// hosts without a full [`SessionsContext`].
pub trait DaemonContext: Send + Sync {
    fn daemon_pool(&self) -> &PgPool;
    fn daemon_bus(&self) -> &Bus;
    /// Machines whose daemon advertised `mark_acks`.
    fn mark_ack_daemons(&self) -> &Arc<DashMap<Uuid, ()>>;
}

/// Router state as the sessions domain sees it.
pub trait SessionsContext:
    KernelContext
    + DaemonContext
    + SessionsAccounts
    + SessionsBlobs
    + SessionsDispatch
    + SessionsExtras
    + SessionsHost
    + SessionsNotify
    + SessionsSearch
{
    fn permission_store(&self) -> &SharedPermissionStore;
    fn machine_event_inserts(&self) -> &Arc<DashMap<Uuid, u64>>;
    /// Machines advertising targeted authentication recovery in their current connection.
    fn auth_recovery_daemons(&self) -> &Arc<DashMap<Uuid, ()>>;
    fn pr_status_cache(&self) -> &cctui_proto::classifier::PrStatusCache;
    fn codex_catalogs(&self) -> &Arc<DashMap<Uuid, CachedCatalog>>;
    fn codex_account_catalogs(&self) -> &Arc<DashMap<Uuid, CachedCatalog>>;
    fn codex_latest_version(&self) -> &Arc<Mutex<Option<CachedVersion>>>;
    fn eviction_tracker(&self) -> &Arc<EvictionTracker>;
    fn connect_tracker(&self) -> &Arc<ConnectTracker>;
    fn divergence_tracker(&self) -> &Arc<DivergenceTracker>;
}
