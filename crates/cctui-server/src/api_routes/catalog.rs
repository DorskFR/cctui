//! Domain catalogs: the static tables and the per-harness model lists both
//! clients derive their pickers from.

use super::GET;
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::routing::get;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/meta/domain",
        "Provider metadata, quota probes, end-reason tones, permission modes.",
        get(routes::domain_meta::get_domain_meta),
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
