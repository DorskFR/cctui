//! Daemon manifest and binary download routes.

use super::GET;
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::routing::get;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/manifest/daemon",
        "Daemon update manifest (latest version + download URLs).",
        get(routes::manifest::daemon_manifest),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET],
        "/daemon/binary/{target}",
        "Download a daemon binary for a target (self-update proxy).",
        get(routes::manifest::download_daemon_binary),
        Authn::Bearer,
        Authenticated,
    )
}
