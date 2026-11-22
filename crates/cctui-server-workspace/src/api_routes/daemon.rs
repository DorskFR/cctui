//! Daemon manifest and binary download routes.

use super::GET;
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use axum::routing::get;

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
