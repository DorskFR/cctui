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
mod otlp;
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
    let r = session_view::register_result(r);
    let r = session_view::register_voice_notes(r);
    let r = previews::register(r);
    let r = session_messages::register(r);
    let r = session_control::register(r);
    let r = session_control::register_control(r);
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
    let r = otlp::register(r);
    users::register(r)
}

#[cfg(test)]
mod route_table {
    use std::collections::BTreeSet;

    use cctui_proto::api::routes::{self, ROUTES};
    use cctui_proto::features::{Feature, FeatureSet};

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

    fn mounted(features: FeatureSet) -> BTreeSet<(String, &'static str)> {
        crate::build_api_routes_with(features)
            .into_parts()
            .2
            .into_iter()
            .map(|d| (d.method.as_str().to_owned(), d.path))
            .collect()
    }

    #[test]
    fn every_feature_on_mounts_the_full_table() {
        assert_eq!(mounted(FeatureSet::all()), tabled());
    }

    #[test]
    fn turning_one_feature_off_unmounts_exactly_its_routes() {
        for feature in Feature::ALL {
            let expected: BTreeSet<_> = ROUTES
                .iter()
                .filter(|r| r.feature != Some(feature))
                .map(|r| (r.method.as_str().to_owned(), r.path))
                .collect();
            assert_eq!(mounted(FeatureSet::all().without(feature)), expected, "{feature}");
        }
    }

    #[test]
    fn the_core_feature_file_mounts_the_checked_in_core_routes() {
        let core =
            crate::config::parse_features(include_str!("../../../../deploy/features/core.toml"))
                .expect("core.toml resolves")
                .set;
        let actual: BTreeSet<String> =
            mounted(core).into_iter().map(|(m, p)| format!("{m} {p}")).collect();
        let expected: BTreeSet<String> =
            include_str!("core_routes.txt").lines().map(str::to_owned).collect();
        let unlisted: Vec<_> = actual.difference(&expected).collect();
        assert!(
            unlisted.is_empty(),
            "routes mounted on a core server but missing from api_routes/core_routes.txt: \
             {unlisted:?}. A route the core journey needs goes in core_routes.txt; any other \
             route gets `feature: Some(Feature::..)` on its entry in \
             cctui_proto::api::routes::ROUTES (see docs/features.md)."
        );
        let gone: Vec<_> = expected.difference(&actual).collect();
        assert!(
            gone.is_empty(),
            "api_routes/core_routes.txt lists routes a core server does not mount: {gone:?}. \
             Remove them from core_routes.txt, or set `feature: None` on their ROUTES entry."
        );
    }

    #[tokio::test]
    async fn a_dropped_route_answers_404_and_a_core_route_stays_mounted() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let router = crate::build_api_routes_with(FeatureSet::none()).into_parts().0;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://u:p@127.0.0.1:1/none")
            .unwrap();
        let app = router.with_state(crate::state::AppState::for_test(pool));
        let status = |method: &str, uri: &str| {
            let request = Request::builder().method(method).uri(uri).body(Body::empty()).unwrap();
            let app = app.clone();
            async move { app.oneshot(request).await.unwrap().status() }
        };
        assert_eq!(status("POST", "/admin/plugins").await, StatusCode::NOT_FOUND);
        assert_eq!(status("GET", "/auth/device/ABCD").await, StatusCode::NOT_FOUND);
        assert_eq!(status("GET", "/prompts").await, StatusCode::NOT_FOUND);
        assert_eq!(status("GET", "/skills/index").await, StatusCode::NOT_FOUND);
        assert_eq!(status("GET", "/labels").await, StatusCode::NOT_FOUND);
        assert_eq!(status("POST", "/sessions/s1/pin").await, StatusCode::NOT_FOUND);
        assert_eq!(status("POST", "/enroll").await, StatusCode::NOT_FOUND);
        assert_eq!(status("GET", "/manifest/daemon").await, StatusCode::NOT_FOUND);
        assert_eq!(status("GET", "/sessions").await, StatusCode::UNAUTHORIZED);
        assert_eq!(status("GET", "/accounts").await, StatusCode::UNAUTHORIZED);
    }
}
