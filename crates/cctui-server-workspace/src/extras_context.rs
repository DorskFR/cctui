use std::sync::Arc;

use cctui_server_extras::context::{ExtrasContext, SearchReindex};
use cctui_server_extras::langfuse::LangfuseClient;
use sqlx::PgPool;

use crate::state::AppState;

impl SearchReindex for AppState {
    async fn reindex_search(&self, pool: &PgPool, ids: &[i64]) -> Result<(), sqlx::Error> {
        crate::search_index::reindex(pool, ids).await
    }
}

impl ExtrasContext for AppState {
    fn langfuse(&self) -> Option<&Arc<LangfuseClient>> {
        self.langfuse.as_ref()
    }
}
