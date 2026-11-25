//! Daemon manifest and binary download routes.

use super::GET;
use crate::authz::Routes;
use axum::routing::get;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Authenticated;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/manifest/daemon",
        "Daemon update manifest (latest version + download URLs).",
        get(cctui_server_dispatch::targets::manifest::daemon_manifest::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET],
        "/daemon/binary/{target}",
        "Download a daemon binary for a target (self-update proxy).",
        get(cctui_server_dispatch::targets::manifest::download_daemon_binary::<
            crate::state::AppState,
        >),
        Authn::Bearer,
        Authenticated,
    )
}
