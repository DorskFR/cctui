//! Each module stands in for the extras module its tests came from, so their
//! `super::` paths resolve against the host state.

mod cctuiverse {
    pub use crate::cctuiverse::*;
    use crate::error::AppError;
    use crate::state::AppState;

    mod tests;
}

mod peer {
    pub use crate::routes::peer::*;
    use crate::state::AppState;
    use crate::transcript_md;
    use cctui_server_extras::peer_policy;
    use serde_json::{Value, json};

    mod tests;
}

mod plugin_proxy {
    pub use crate::plugin_proxy::*;

    mod tests;
}

mod plugin_store {
    use std::collections::BTreeMap;

    pub use crate::plugin_store::*;
    use crate::plugins::{Plugin, PluginRegistry};
    use crate::state::AppState;

    pub async fn install(
        pool: &sqlx::PgPool,
        registry: &PluginRegistry,
        bytes: &[u8],
        installed_by: Option<uuid::Uuid>,
    ) -> Result<Plugin, InstallError> {
        let blobs = AppState::for_test(pool.clone());
        crate::plugin_store::install(pool, registry, bytes, installed_by, &blobs).await
    }

    pub async fn load_all(pool: &sqlx::PgPool) -> sqlx::Result<BTreeMap<String, Plugin>> {
        crate::plugin_store::load_all(pool, &AppState::for_test(pool.clone())).await
    }

    pub async fn sync(pool: &sqlx::PgPool, registry: &PluginRegistry) -> sqlx::Result<()> {
        crate::plugin_store::sync(pool, registry, &AppState::for_test(pool.clone())).await
    }

    pub async fn uninstall(
        pool: &sqlx::PgPool,
        registry: &PluginRegistry,
        id: &str,
    ) -> sqlx::Result<bool> {
        crate::plugin_store::uninstall(pool, registry, id, &AppState::for_test(pool.clone())).await
    }

    mod tests;
}

mod plugins {
    pub use crate::routes::plugins::*;

    mod tests;
}

mod plugins_admin {
    pub use crate::routes::plugins_admin::*;

    mod tests;
}

mod presence {
    pub use crate::presence::*;
    use crate::state::AppState;
    use sqlx::PgPool;
    use uuid::Uuid;

    mod tests;
}

mod preview {
    pub use crate::preview::*;

    mod e2e;
}
