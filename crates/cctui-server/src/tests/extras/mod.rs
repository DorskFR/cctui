//! Each module stands in for the extras module its tests came from, so their
//! `super::` paths resolve against the host state.

mod cctuiverse {
    use crate::state::AppState;
    pub use cctui_server_extras::cctuiverse::*;
    use cctui_server_kernel::error::AppError;

    mod tests;
}

mod peer {
    use crate::state::AppState;
    use cctui_server_extras::peer_policy;
    pub use cctui_server_extras::routes::peer::*;
    use cctui_server_sessions::transcript_md;
    use serde_json::{Value, json};

    mod tests;
}

mod plugin_proxy {
    pub use cctui_server_extras::plugin_proxy::*;

    mod tests;
}

mod plugin_store {
    use std::collections::BTreeMap;

    use crate::state::AppState;
    pub use cctui_server_extras::plugin_store::*;
    use cctui_server_extras::plugins::{Plugin, PluginRegistry};

    pub async fn install(
        pool: &sqlx::PgPool,
        registry: &PluginRegistry,
        bytes: &[u8],
        installed_by: Option<uuid::Uuid>,
    ) -> Result<Plugin, InstallError> {
        let blobs = AppState::for_test(pool.clone());
        cctui_server_extras::plugin_store::install(pool, registry, bytes, installed_by, &blobs)
            .await
    }

    pub async fn load_all(pool: &sqlx::PgPool) -> sqlx::Result<BTreeMap<String, Plugin>> {
        cctui_server_extras::plugin_store::load_all(pool, &AppState::for_test(pool.clone())).await
    }

    pub async fn sync(pool: &sqlx::PgPool, registry: &PluginRegistry) -> sqlx::Result<()> {
        cctui_server_extras::plugin_store::sync(pool, registry, &AppState::for_test(pool.clone()))
            .await
    }

    pub async fn uninstall(
        pool: &sqlx::PgPool,
        registry: &PluginRegistry,
        id: &str,
    ) -> sqlx::Result<bool> {
        cctui_server_extras::plugin_store::uninstall(
            pool,
            registry,
            id,
            &AppState::for_test(pool.clone()),
        )
        .await
    }

    mod tests;
}

mod plugins {
    pub use cctui_server_extras::routes::plugins::*;

    mod tests;
}

mod plugins_admin {
    pub use cctui_server_extras::routes::plugins_admin::*;

    mod tests;
}

mod presence {
    use crate::state::AppState;
    pub use cctui_server_extras::presence::*;
    use sqlx::PgPool;
    use uuid::Uuid;

    mod tests;
}

mod preview {
    pub use cctui_server_extras::preview::*;

    mod e2e;
}
