#[cfg(test)]
use cctui_server_kernel::authz::*;

pub type Routes = cctui_server_kernel::authz::Routes<crate::state::AppState>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_api_routes;
    use axum::http::{Method, StatusCode};
    use cctui_server_kernel::auth::Scope;
    use cctui_server_kernel::authz::test_support::{admin, one_route_app, status_of, user};
    use uuid::Uuid;

    fn descriptors() -> Vec<RouteDescriptor> {
        build_api_routes().into_parts().2
    }

    #[test]
    fn every_api_route_declares_both_axes() {
        let descs = descriptors();
        assert!(!descs.is_empty(), "route table is empty");
        for d in &descs {
            let _ = (&d.method, d.path, d.authn, &d.authz);
        }
        assert!(descs.iter().all(|d| d.path.starts_with('/')));
    }

    #[test]
    fn every_route_has_a_summary() {
        for d in descriptors() {
            assert!(
                !d.summary.trim().is_empty(),
                "route {} {} has an empty summary — every Routes::add entry must document itself",
                d.method.as_str(),
                d.path
            );
        }
    }

    #[test]
    fn only_allowlisted_api_routes_are_anonymous() {
        let none: Vec<_> =
            descriptors().into_iter().filter(|d| d.authn == Authn::None).map(|d| d.path).collect();
        for path in &none {
            assert!(
                PUBLIC_PATHS.contains(path),
                "{path} is Authn::None but not in PUBLIC_PATHS — every other route authenticates"
            );
        }
    }

    /// A half-public route would be unreachable: it is mounted outside the layer
    /// that would build its principal.
    #[test]
    fn public_routes_are_exactly_the_allowlist() {
        const SIGNED: &[&str] = &["/hooks/{id}"];
        let public: Vec<_> =
            descriptors().into_iter().filter(|d| matches!(d.authz, Authz::Public)).collect();
        let mut paths: Vec<_> = public.iter().map(|d| d.path).collect();
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(paths, PUBLIC_PATHS, "the Authz::Public set must equal PUBLIC_PATHS");
        for d in &public {
            assert_eq!(d.authn, Authn::None, "{} must also be Authn::None", d.path);
            let signed = SIGNED.contains(&d.path);
            let expected = if signed { Method::POST } else { Method::GET };
            assert_eq!(d.method, expected, "{} must be read-only, or a signed POST", d.path);
        }
    }

    #[test]
    fn every_public_path_is_registered() {
        let descs = descriptors();
        for path in PUBLIC_PATHS {
            assert!(
                descs.iter().any(|d| d.path == *path),
                "{path} is in PUBLIC_PATHS but not registered"
            );
        }
    }

    /// The device-login approval pair decides who a minted key belongs to, so
    /// it must never join the device-facing `start`/`poll` pair outside the
    /// auth layer.
    #[test]
    fn device_login_approval_requires_a_principal() {
        let descs = descriptors();
        let approval: Vec<_> =
            descs.iter().filter(|d| d.path.starts_with("/auth/device/")).collect();
        assert_eq!(approval.len(), 2, "expected the info + decision routes");
        for d in approval {
            assert_eq!(d.authn, Authn::Bearer, "{} must authenticate", d.path);
            assert!(
                matches!(d.authz, Authz::Authenticated),
                "{} must require a principal, found {:?}",
                d.path,
                d.authz
            );
            assert!(d.path.contains("{user_code}"), "{} must name the code", d.path);
        }
    }

    #[test]
    fn resource_id_param_appears_in_path() {
        // A `Resource` id param must exist in the route's path.
        for d in descriptors() {
            if let Authz::Resource(_, _, IdFrom::Path(name)) = d.authz {
                let token = format!("{{{name}}}");
                assert!(
                    d.path.contains(&token),
                    "route {} declares IdFrom::Path(\"{name}\") but path has no {token}",
                    d.path
                );
            }
        }
    }

    #[test]
    fn single_session_routes_use_session_guard() {
        for d in descriptors() {
            if d.path.starts_with("/sessions/{id}") {
                assert!(
                    matches!(
                        d.authz,
                        Authz::Resource(ResourceKind::Session, _, IdFrom::Path("id"))
                    ),
                    "{} {} must declare a Session resource policy, found {:?}",
                    d.method.as_str(),
                    d.path,
                    d.authz
                );
            }
        }
    }

    #[test]
    fn account_family_routes_are_human_only() {
        const HUMAN_PREFIXES: &[&str] = &[
            "/accounts",
            "/account-pools",
            "/context",
            "/profiles",
            "/redirects",
            "/{resource_type}",
        ];
        const EXEMPT: &[&str] = &["/accounts/settings-catalog"];
        for d in descriptors() {
            if HUMAN_PREFIXES.iter().any(|p| d.path.starts_with(p)) && !EXEMPT.contains(&d.path) {
                assert!(
                    d.authz.human_only(),
                    "{} {} must declare Authz::Human, found {:?}",
                    d.method.as_str(),
                    d.path,
                    d.authz
                );
            }
        }
    }

    #[tokio::test]
    async fn spawn_route_needs_the_dispatch_scope() {
        let spawn = descriptors()
            .into_iter()
            .find(|d| d.method == Method::POST && d.path == "/sessions/spawn")
            .expect("spawn route registered");
        assert!(matches!(spawn.authz, Authz::Scope(Scope::Dispatch)), "{:?}", spawn.authz);

        let read_only = one_route_app(spawn.authz.clone(), Some(user(Uuid::new_v4())));
        assert_eq!(status_of(read_only, "/r").await, StatusCode::FORBIDDEN);

        let mut human = user(Uuid::new_v4());
        human.scopes = [Scope::Read, Scope::Enroll, Scope::Dispatch].into_iter().collect();
        let webui = one_route_app(spawn.authz.clone(), Some(human));
        assert_eq!(status_of(webui, "/r").await, StatusCode::OK);
        let admin = one_route_app(spawn.authz, Some(admin()));
        assert_eq!(status_of(admin, "/r").await, StatusCode::OK);
    }

    #[test]
    fn custom_and_scope_routes_are_enumerated() {
        // Enumerates every non-`Authenticated` policy so a change to who-can-do-what
        // shows up in this test.
        let mut custom: Vec<&'static str> = Vec::new();
        let mut scoped: Vec<(&'static str, String)> = Vec::new();
        for d in descriptors() {
            match &d.authz {
                Authz::Custom(_) => custom.push(d.path),
                Authz::Scope(s) => scoped.push((d.path, s.to_string())),
                _ => {}
            }
        }
        assert!(custom.is_empty(), "unexpected Custom authz route(s): {custom:?}");
        scoped.sort();
        scoped.dedup();
        assert!(
            scoped.iter().all(|(_, s)| matches!(s.as_str(), "dispatch" | "enroll" | "admin")),
            "unexpected scope on a route: {scoped:?}"
        );
        assert!(scoped.iter().any(|(_, s)| s == "admin"), "no admin-scoped route registered");
        assert!(scoped.iter().any(|(_, s)| s == "dispatch"), "no dispatch-scoped route");
        assert!(scoped.iter().any(|(_, s)| s == "enroll"), "no enroll-scoped route");
    }

    /// Every id-taking route that only declares `Authenticated` carries its
    /// ownership filter inside the handler, with nothing proving it from the
    /// route table. Each one is listed here deliberately: a new entry means a
    /// new route whose handler SQL must be read for an owner filter, and a
    /// route that gains a `Resource` policy should leave the list.
    #[test]
    fn id_taking_authenticated_routes_are_enumerated() {
        const FILTERED_IN_HANDLER: &[&str] = &[
            // The caller IS the id: the handler matches it against the principal.
            "/auth/device/{user_code}",
            "/auth/device/{user_code}/decision",
            "/users/{id}/tokens",
            // `user_id`-filtered SQL in the handler.
            "/bookmarks/{id}",
            "/dispatch-records/{id}/dismiss",
            "/drafts/{*key}",
            "/inbox/events/{id}",
            "/keys/{id}",
            "/labels/{id}",
            "/passkeys/{id}",
            "/presets/{id}",
            "/presets/{id}/attachments",
            "/presets/{id}/run",
            "/webhooks/{id}",
            "/webhooks/{id}/deliveries",
            "/webhooks/{id}/secret",
            "/prompts/{id}",
            "/rooms/{id}",
            "/plugins/{id}/backend/{*path}",
            // Machine-scoped, owner-checked inside the handler.
            "/machines/{machine_id}/commands/pending",
            // Server-wide catalogues, not per-user objects.
            "/daemon/binary/{target}",
            "/models/{harness}",
            "/skills/{name}",
        ];
        let mut found: Vec<&str> = descriptors()
            .iter()
            .filter(|d| matches!(d.authz, Authz::Authenticated) && d.path.contains('{'))
            .map(|d| d.path)
            .collect();
        found.sort_unstable();
        found.dedup();
        let mut expected = FILTERED_IN_HANDLER.to_vec();
        expected.sort_unstable();
        assert_eq!(
            found, expected,
            "an id-taking Authenticated route changed: give it an Authz::Resource policy, or \
             list it here once its handler filters by owner"
        );
    }
}
