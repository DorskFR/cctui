//! The `/api/v1` route table, registered area by area.

mod account_usage;
mod accounts;
mod admin_instance;
mod admin_users;
mod catalog;
mod cctuiverse;
mod context;
mod daemon;
mod device_auth;
mod dispatch;
mod drafts;
mod keys;
mod labels;
mod machines;
mod me;
mod passkeys;
mod plugins;
mod previews;
mod profiles;
mod prompts;
mod provider_status;
mod rooms;
mod session_bulk;
mod session_control;
mod session_lifecycle;
mod session_list;
mod session_messages;
mod session_state;
mod session_view;
mod shares;
mod skills;
mod user_actions;
mod users;
mod version;
mod voice;

use axum::http::Method;

use crate::authz::{Action, Authz, IdFrom, ResourceKind, Routes};

const GET: Method = Method::GET;

// Per-session ownership guard: `machine_uuid -> machines.user_id`,
// id sourced from the `{id}` path param. `read`/`write` differ only in the
// recorded `Action` (for RBAC); the owner rule is identical today.
const fn sess_read() -> Authz {
    Authz::Resource(ResourceKind::Session, Action::Read, IdFrom::Path("id"))
}

const fn sess_write() -> Authz {
    Authz::Resource(ResourceKind::Session, Action::Write, IdFrom::Path("id"))
}

pub fn register(r: Routes) -> Routes {
    let r = passkeys::register(r);
    let r = plugins::register(r);
    let r = context::register(r);
    let r = version::register(r);
    let r = catalog::register(r);
    let r = session_lifecycle::register(r);
    let r = dispatch::register(r);
    let r = session_bulk::register(r);
    let r = session_list::register(r);
    let r = session_view::register(r);
    let r = session_view::register_voice_notes(r);
    let r = previews::register(r);
    let r = session_messages::register(r);
    let r = session_control::register(r);
    let r = session_state::register(r);
    let r = labels::register(r);
    let r = daemon::register(r);
    let r = prompts::register(r);
    let r = provider_status::register(r);
    let r = keys::register(r);
    let r = accounts::register(r);
    let r = profiles::register(r);
    let r = account_usage::register(r);
    let r = rooms::register(r);
    let r = cctuiverse::register(r);
    let r = shares::register(r);
    let r = machines::register(r);
    let r = me::register(r);
    let r = device_auth::register(r);
    let r = admin_instance::register(r);
    let r = admin_users::register(r);
    let r = skills::register(r);
    let r = user_actions::register(r);
    let r = drafts::register(r);
    let r = voice::register(r);
    users::register(r)
}

#[cfg(test)]
mod route_table {
    use std::collections::BTreeSet;

    use cctui_proto::api::routes::{self, ROUTES};

    /// `(METHOD, path)` pairs the axum router is actually built from.
    fn registered() -> BTreeSet<(String, &'static str)> {
        crate::build_api_routes()
            .into_parts()
            .2
            .into_iter()
            .map(|d| (d.method.as_str().to_owned(), d.path))
            .collect()
    }

    fn tabled() -> BTreeSet<(String, &'static str)> {
        ROUTES.iter().map(|r| (r.method.as_str().to_owned(), r.path)).collect()
    }

    #[test]
    fn every_registered_route_is_in_the_proto_table() {
        let missing: Vec<_> = registered().difference(&tabled()).cloned().collect();
        assert!(
            missing.is_empty(),
            "routes registered on the router but absent from cctui_proto::api::routes::ROUTES: \
             {missing:?}"
        );
    }

    #[test]
    fn every_proto_table_route_is_registered() {
        let extra: Vec<_> = tabled().difference(&registered()).cloned().collect();
        assert!(
            extra.is_empty(),
            "routes in cctui_proto::api::routes::ROUTES that no longer exist on the router: \
             {extra:?}"
        );
    }

    #[test]
    fn summaries_match_the_router() {
        for d in crate::build_api_routes().into_parts().2 {
            let method = routes::Method::parse(d.method.as_str()).expect("known method");
            let route = routes::find(method, d.path).expect("route is tabled");
            assert_eq!(
                route.summary, d.summary,
                "{} {}: summary drifted from the router",
                d.method, d.path
            );
        }
    }
}
