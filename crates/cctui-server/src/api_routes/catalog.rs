//! Domain catalogs: the server-owned tables both clients derive their pickers
//! from.

use super::GET;
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::routing::get;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/meta/usage-probes",
        "The quota-probe registry, for the account usage-probe picker.",
        get(routes::usage_probes::get_usage_probes),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET],
        "/harnesses",
        "The harness table: ids, families, capabilities and permission modes.",
        get(routes::harnesses::list_harnesses),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET],
        "/models/{harness}",
        "Model and effort options for a harness picker (optional machine_id).",
        get(routes::harness_models::get_harness_models),
        Authn::Bearer,
        Authenticated,
    )
}
