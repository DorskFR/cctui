//! Upstream provider incident status.

use super::GET;
use crate::authz::Routes;
use axum::routing::get;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Authenticated;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/provider-status",
        "What each upstream provider family reports on its status page.",
        get(cctui_server_accounts::routes::provider_status::provider_status::<
            crate::state::AppState,
        >),
        Authn::Bearer,
        Authenticated,
    )
}
