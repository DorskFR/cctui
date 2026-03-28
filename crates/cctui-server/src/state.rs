use crate::auth::AuthConfig;
use crate::config::Config;
use crate::registry::SharedRegistry;
use crate::routes::channels::SharedChannelStore;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub registry: SharedRegistry,
    pub channel_store: SharedChannelStore,
    #[allow(dead_code)]
    pub auth_config: AuthConfig,
}
