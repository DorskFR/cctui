//! Red-team run results: posted by the runner, read by admins.

use super::GET;
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::{get, post};
use cctui_server_kernel::auth;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Scope as ScopeAz;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/redteam/runs",
        "Record a red-team T2 or T3 run result.",
        post(cctui_server_sessions::routes::redteam::record::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Dispatch),
    )
    .add(
        &[GET],
        "/admin/redteam",
        "The latest red-team T2 and T3 run per reported version, with escapes (admin).",
        get(cctui_server_sessions::routes::redteam::overview::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
}
