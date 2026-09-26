//! Admin instance and harness auto-update routes.

use super::GET;
use crate::authz::Authz::Scope as ScopeAz;
use crate::authz::{Authn, Routes};
use crate::{auth, routes};
use axum::http::Method;
use axum::routing::{get, post, put};

pub(super) fn register(r: Routes) -> Routes {
    r
        // Admin surface: every route is `forbid_or` (Scope::Admin).
        .add(
            &[Method::POST, GET],
            "/admin/users",
            "List all users, or create a user (admin).",
            post(routes::admin_auth::create_user).get(routes::admin_auth::list_users),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::PUT],
            "/admin/instance",
            "Set or clear the server-wide deployment name shown in the webui header (admin).",
            put(routes::instance::update),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[GET, Method::PUT],
            "/admin/instance/self-update",
            "Read or set the machine + directory the self-update agent runs on (admin).",
            get(routes::instance::get_self_update_target).put(routes::instance::update_self_update_target),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[GET, Method::PUT],
            "/admin/instance/spawn-defaults",
            "Read or set the default CctuiAgent limits for sessions that declare none (admin).",
            get(routes::server_settings::get_spawn_defaults).put(routes::server_settings::update_spawn_defaults),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[GET, Method::PUT],
            "/admin/instance/upstream-hosts",
            "Read or set the hosts per-account upstreams may reach despite the SSRF guard (admin).",
            get(routes::server_settings::get_upstream_hosts).put(routes::server_settings::update_upstream_hosts),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[GET, Method::PUT],
            "/admin/harness-autoupdate",
            "Read the harness auto-update settings of every machine, or set the instance default (admin).",
            get(routes::harness_update::read).put(routes::harness_update::set_instance),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::PUT],
            "/admin/harness-autoupdate/{machine_id}",
            "Set or clear one machine's harness auto-update override (admin).",
            put(routes::harness_update::set_machine),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
}
