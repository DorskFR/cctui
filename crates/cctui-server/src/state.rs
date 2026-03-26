use crate::auth::AuthConfig;
use crate::config::Config;
use crate::registry::SharedRegistry;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub registry: SharedRegistry,
    #[allow(dead_code)]
    pub auth_config: AuthConfig,
}
