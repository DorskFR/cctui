//! Upstream provider incident status.

use super::GET;
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::routing::get;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/provider-status",
        "What each upstream provider family reports on its status page.",
        get(routes::provider_status::provider_status),
        Authn::Bearer,
        Authenticated,
    )
}
