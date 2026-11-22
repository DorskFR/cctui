use std::future::Future;
use std::sync::Arc;

use cctui_server_kernel::context::KernelContext;
use sqlx::PgPool;

use crate::hooks::{BlobsHooks, SessionsHooks};
use crate::langfuse::LangfuseClient;
use crate::plugins::PluginRegistry;
use crate::presence::PodIdentity;

/// Rebuilds the search chunks of events whose payload changed.
pub trait SearchReindex: Send + Sync + 'static {
    fn reindex_search(
        &self,
        pool: &PgPool,
        ids: &[i64],
    ) -> impl Future<Output = Result<(), sqlx::Error>> + Send;
}

/// What the extras routes and jobs need from the host beyond the kernel.
pub trait ExtrasContext: KernelContext + SearchReindex + BlobsHooks + SessionsHooks {
    fn langfuse(&self) -> Option<&Arc<LangfuseClient>>;
    fn plugins(&self) -> &Arc<PluginRegistry>;
    fn preview(&self) -> &Arc<crate::preview::Registry>;
    fn presence(&self) -> &Arc<PodIdentity>;
    /// Shared secret for pod-to-pod `/internal/*` calls.
    fn internal_secret(&self) -> Option<&Arc<str>>;
}
