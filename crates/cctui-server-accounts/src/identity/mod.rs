//! Headless identity and administrative HTTP services.
pub mod admin;
pub mod me;

use cctui_server_kernel::auth::AuthConfig;
use cctui_server_kernel::bus::Bus;
use cctui_server_kernel::context::KernelContext;
use sqlx::PgPool;

/// What the identity handlers read from their host.
pub trait IdentityContext: Clone + Send + Sync + 'static {
    fn pool(&self) -> &PgPool;
    fn bus(&self) -> &Bus;
    fn auth_config(&self) -> &AuthConfig;
}

impl<T: KernelContext> IdentityContext for T {
    fn pool(&self) -> &PgPool {
        KernelContext::pool(self)
    }
    fn bus(&self) -> &Bus {
        KernelContext::bus(self)
    }
    fn auth_config(&self) -> &AuthConfig {
        KernelContext::auth_config(self)
    }
}
