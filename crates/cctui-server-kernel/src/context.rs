use sqlx::PgPool;

use crate::auth::AuthConfig;
use crate::bus::Bus;
use crate::config::Config;

/// Router state as a domain crate sees it. The host implements this (and each
/// domain's own context trait on top of it), so no domain names the host state.
pub trait KernelContext: Clone + Send + Sync + 'static {
    fn pool(&self) -> &PgPool;
    fn bus(&self) -> &Bus;
    fn auth_config(&self) -> &AuthConfig;
    fn config(&self) -> &Config;
    fn http_client(&self) -> &reqwest::Client;
}
