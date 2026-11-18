//! Red-team run results: posted by the runner, read by admins.

use super::GET;
use crate::authz::Authz::Scope as ScopeAz;
use crate::authz::{Authn, Routes};
use crate::{auth, routes};
use axum::http::Method;
use axum::routing::{get, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/redteam/runs",
        "Record a red-team T2 or T3 run result.",
        post(routes::redteam::record),
        Authn::Bearer,
        ScopeAz(auth::Scope::Dispatch),
    )
    .add(
        &[GET],
        "/admin/redteam",
        "The latest red-team T2 and T3 run per reported version, with escapes (admin).",
        get(routes::redteam::overview),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
}
