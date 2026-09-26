//! Profile, account-pool and rebind routes.

use super::{GET, sess_read};
use crate::authz::Authz::Human;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{get, patch};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET, Method::POST],
        "/profiles",
        "List the caller's spawn profiles, or create one.",
        get(routes::profiles::list_profiles).post(routes::profiles::create_profile),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::PUT],
        "/profiles/order",
        "Persist the caller's profile order.",
        axum::routing::put(routes::profiles::reorder_profiles),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/profiles/{id}",
        "Rename, adjust or delete a spawn profile.",
        patch(routes::profiles::update_profile).delete(routes::profiles::delete_profile),
        Authn::Bearer,
        Human,
    )
    // Account pools: the durable "these accounts are interchangeable"
    // statement that bounds both auto-binding and mid-session failover.
    .add(
        &[GET, Method::POST],
        "/account-pools",
        "List the caller's account pools, or create one.",
        get(routes::account_pools::list_pools).post(routes::account_pools::create_pool),
        Authn::Bearer,
        Human,
    )
    .add(
        &[GET],
        "/account-pools/usage",
        "Every pool's quota windows aggregated per provider family: level, pace, projection.",
        get(routes::account_pools::pools_usage),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/account-pools/{id}",
        "Edit a pool (name, strategy, failover, membership) or delete it.",
        patch(routes::account_pools::update_pool).delete(routes::account_pools::delete_pool),
        Authn::Bearer,
        Human,
    )
    .add(
        &[GET],
        "/sessions/{id}/rebinds",
        "Every mid-run account move this session made, newest first.",
        get(routes::account_pools::list_session_rebinds),
        Authn::Bearer,
        sess_read(),
    )
}
