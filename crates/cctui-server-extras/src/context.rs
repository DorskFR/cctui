use std::future::Future;
use std::sync::Arc;

use cctui_server_kernel::context::KernelContext;
use sqlx::PgPool;

use crate::langfuse::LangfuseClient;

/// Rebuilds the search chunks of events whose payload changed.
pub trait SearchReindex: Send + Sync + 'static {
    fn reindex_search(
        &self,
        pool: &PgPool,
        ids: &[i64],
    ) -> impl Future<Output = Result<(), sqlx::Error>> + Send;
}

/// What the extras routes and jobs need from the host beyond the kernel.
pub trait ExtrasContext: KernelContext + SearchReindex {
    fn langfuse(&self) -> Option<&Arc<LangfuseClient>>;
}
