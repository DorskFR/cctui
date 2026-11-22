//! What the accounts routes and jobs need from their host.
use cctui_server_kernel::context::KernelContext;
use sqlx::PgPool;

use crate::accounts::AccountsContext;
use crate::gateway::proxy::ProxyContext;
use crate::session_credentials::SessionCredentialContext;

/// The host state the account handlers run on.
pub trait AccountsHost:
    KernelContext + AccountsContext + ProxyContext + SessionCredentialContext
{
    /// `None` when no secure public URL is configured.
    fn webauthn(&self) -> Option<&webauthn_rs::Webauthn>;

    /// The kernel pool, apart from the account contexts' own `pool` accessors.
    fn kernel_pool(&self) -> &PgPool {
        KernelContext::pool(self)
    }
}
