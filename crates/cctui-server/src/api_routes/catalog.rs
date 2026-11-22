//! Domain catalogs: the server-owned tables both clients derive their pickers
//! from.

use super::GET;
use crate::authz::Routes;
use axum::routing::get;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Authenticated;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/meta/usage-probes",
        "The quota-probe registry, for the account usage-probe picker.",
        get(cctui_server_accounts::routes::usage_probes::get_usage_probes),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET],
        "/harnesses",
        "The harness table: ids, families, capabilities and permission modes.",
        get(cctui_server_sessions::routes::harnesses::list_harnesses),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET],
        "/models/{harness}",
        "Model and effort options for a harness picker (optional machine_id).",
        get(cctui_server_sessions::routes::harness_models::get_harness_models::<
            crate::state::AppState,
        >),
        Authn::Bearer,
        Authenticated,
    )
}
