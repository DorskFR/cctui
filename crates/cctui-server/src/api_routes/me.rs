//! Caller identity, settings, capabilities and enrolment routes.

use super::GET;
use crate::authz::Authz::{self, Authenticated, Scope as ScopeAz};
use crate::authz::{Action, Authn, IdFrom, ResourceKind, Routes};
use crate::{auth, routes};
use axum::http::Method;
use axum::routing::{get, post, put};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/me",
        "Get the current principal (user, scopes, machine).",
        get(routes::me::me),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET],
        "/settings",
        "Get your user settings.",
        get(routes::settings::get_settings),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::PUT],
        "/settings",
        "Replace your user settings.",
        put(routes::settings::put_settings),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/settings/rescrub",
        "Re-apply the secret-scrub list to your stored events.",
        post(routes::settings::rescrub_settings),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET],
        "/capabilities",
        "List server capabilities/feature flags.",
        get(routes::capabilities::capabilities),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/enroll",
        "Enroll this machine and mint its machine key.",
        post(routes::enroll::enroll),
        Authn::Bearer,
        ScopeAz(auth::Scope::Enroll),
    )
    .add(
        &[GET],
        "/machines/resources",
        "The caller's daemon machines with their last host CPU/memory/disk snapshot.",
        get(crate::machine_resources::list),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET],
        "/machines/{machine_id}/status",
        "Machine connectivity/liveness snapshot (remote-enroll verification).",
        get(routes::enroll::machine_status),
        Authn::Bearer,
        Authz::Resource(ResourceKind::Machine, Action::Read, IdFrom::Path("machine_id")),
    )
    .add(
        &[Method::POST],
        "/deenroll",
        "Deenroll the current machine.",
        post(routes::enroll::deenroll),
        Authn::Bearer,
        // In-handler: requires a machine token (machine_id present).
        Authenticated,
    )
}
