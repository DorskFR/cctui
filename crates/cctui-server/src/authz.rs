//! Declarative route authn/authz framework (CCT-419, epic CCT-416).
//!
//! Every HTTP route is registered through [`Routes::add`], which demands BOTH
//! an [`Authn`] (how identity is proven) and an [`Authz`] (what the principal
//! may do). There is deliberately no overload that omits them, so "forgetting
//! authorization on a route" cannot compile.
//!
//! The descriptor list ([`Routes::into_parts`]) IS the route table: the axum
//! `Router` is built only from it, and a coverage test walks it to assert every
//! route carries both axes. The list is the single source of truth.
//!
//! ## Enforcement model
//!
//! `add` attaches each route's [`Authz`] as a per-route request extension via
//! `route_layer`. A single [`authz_layer`] middleware, layered on the
//! authenticated `/api/v1` group AFTER `auth_middleware`, looks up that
//! extension and evaluates it against the request's [`AuthContext`]:
//!
//!   * a request that reaches the layer with **no** `Authz` extension is
//!     rejected `403` — **default deny**, the fail-closed backstop;
//!   * otherwise the policy is evaluated (see [`Authz::enforce`]).
//!
//! ## Scope of this ticket
//!
//! Correctness over purity: the existing, proven authentication paths
//! (`auth_middleware` for `/api/v1`; the inline self-auth on the daemon /
//! dispatcher / trigger / gateway endpoints) are LEFT UNTOUCHED. The [`Authn`]
//! axis is recorded for the coverage test and to document each route's auth
//! method; it does not re-implement authentication. The new runtime behavior is
//! the [`Authz`] evaluation plus default-deny.
//!
//! In-handler ownership checks (the CCT-417 session checks, the
//! `god_view_uid()` list filters, the per-resource owner gates) are also left
//! in place. Routes whose authorization cannot be expressed as a yes/no gate
//! (self-scoped list/filter endpoints) declare [`Authz::Authenticated`] and
//! keep their filter in the handler — they are enumerated in the coverage test.

use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::MethodRouter;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{AuthContext, Scope};
use crate::state::AppState;

/// How identity is proven for a route.
///
/// `QueryToken` and `BodyToken` describe the self-authenticating endpoints'
/// methods for the route table / coverage test even though those endpoints keep
/// their inline auth this ticket, so the variants are not constructed in the
/// `/api/v1` descriptor list (only `Bearer`/`None` are).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Authn {
    /// No identity required. Only `/health`.
    None,
    /// `Authorization: Bearer <token>` — the regular `auth_middleware` path,
    /// and the gateway's provider-key bearer.
    Bearer,
    /// `?token=` on the WS upgrade URI. WebSocket upgrades from browsers cannot
    /// carry an `Authorization` header, so the token rides the query string.
    ///
    /// Retained deliberately: WS still authenticates via `?token=` until
    /// CCT-423 moves it onto an `HttpOnly` cookie (then this variant goes away
    /// and `Bearer` resolves from header-or-cookie). See the CCT-419 design note.
    QueryToken,
    /// A token carried in the request body (daemon/dispatcher `auth`, triggers).
    /// The field name differs per endpoint, so these keep their inline
    /// self-authentication rather than folding into a generic body authenticator.
    BodyToken,
}

/// The coarse capability a principal exercises on a resource. The full set is
/// part of the framework's surface (CCT-420 wires more routes onto
/// `Resource`); not every variant is used by the routes migrated this ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Action {
    Read,
    Write,
    Admin,
}

/// Where a resource id is sourced from when resolving a per-object policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum IdFrom {
    /// A path parameter of the given name (e.g. `IdFrom::Path("id")`).
    Path(&'static str),
}

impl IdFrom {
    #[must_use]
    pub const fn param(self) -> &'static str {
        match self {
            Self::Path(p) => p,
        }
    }
}

/// The kinds of resource the per-object guard knows about. Extended as routes
/// migrate onto [`Authz::Resource`] (CCT-420); only `Session` is wired this
/// ticket, so the other kinds are not yet constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ResourceKind {
    Session,
    Dispatcher,
    User,
    Account,
    Machine,
    Prompt,
    ApiKey,
}

/// The escape-hatch signature: a small, audited closure when neither a scope
/// nor a single-object gate fits. Receives the principal and the resolved id
/// (if any). Kept intentionally narrow.
#[allow(dead_code)]
pub type AuthzFn = fn(&AuthContext, Option<&str>) -> Result<(), StatusCode>;

/// What a principal may do on a route. `Public`, `Resource`, and `Custom` are
/// part of the framework's surface but not yet used by any migrated `/api/v1`
/// route (sessions keep their in-handler check; `/health` is on the outer app),
/// so those variants are constructed only in tests this ticket.
#[derive(Clone)]
#[allow(dead_code)]
pub enum Authz {
    /// No identity required (only `/health`).
    Public,
    /// Any valid principal. For `/api/v1` this is already guaranteed by
    /// `auth_middleware`; the layer then re-asserts a principal is present.
    /// Self-scoped list/filter endpoints use this and keep their SQL filter.
    Authenticated,
    /// A capability gate, no object: `ctx.requires(scope)`.
    Scope(Scope),
    /// A per-object gate. The id is resolved from the request via [`IdFrom`]
    /// and checked by [`Resource::authorize`].
    Resource(ResourceKind, Action, IdFrom),
    /// Small audited escape hatch.
    Custom(AuthzFn),
}

impl std::fmt::Debug for Authz {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Public => write!(f, "Public"),
            Self::Authenticated => write!(f, "Authenticated"),
            Self::Scope(s) => write!(f, "Scope({s:?})"),
            Self::Resource(k, a, i) => write!(f, "Resource({k:?}, {a:?}, {i:?})"),
            Self::Custom(_) => write!(f, "Custom(..)"),
        }
    }
}

impl Authz {
    /// Evaluate this policy. Called by [`authz_layer`] after authentication has
    /// populated [`AuthContext`]. The resource id (if the policy is
    /// [`Authz::Resource`]) is pre-resolved from the request by the caller so
    /// this future borrows nothing from the (non-`Send`) request body.
    async fn enforce(
        &self,
        ctx: &AuthContext,
        id: Option<String>,
        pool: &PgPool,
    ) -> Result<(), StatusCode> {
        match self {
            // The authenticated `/api/v1` group never carries `Public` routes;
            // `Public` reaching here still requires the principal that
            // `auth_middleware` guaranteed. (The genuinely public `/health`
            // lives outside this layer entirely.)
            Self::Public | Self::Authenticated => Ok(()),
            Self::Scope(s) => ctx.requires(*s),
            Self::Resource(kind, action, _id_from) => {
                Resource::authorize(*kind, ctx, *action, id.as_deref(), pool).await
            }
            // The escape hatch receives the resolved id where one exists.
            Self::Custom(f) => f(ctx, id.as_deref()),
        }
    }

    /// The path param this policy sources its id from, if any.
    const fn id_from(&self) -> Option<IdFrom> {
        match self {
            Self::Resource(_, _, id_from) => Some(*id_from),
            _ => None,
        }
    }
}

/// Resolve a path-sourced resource id from the matched route. axum exposes the
/// captured path params on the request extensions as `RawPathParams` once the
/// router has matched the route, which is the case by the time this layer runs.
fn resolve_id(req: &Request, id_from: IdFrom) -> Option<String> {
    let name = id_from.param();
    req.extensions()
        .get::<axum::extract::RawPathParams>()
        .and_then(|raw| raw.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string()))
}

/// The single per-resource authorization chokepoint. Default rule:
/// `admin || owner(id) == principal`. Written once per resource type, not per
/// endpoint. CCT-420 generalizes this to every resource and renames
/// `god_view_uid`; for CCT-419 it implements [`ResourceKind::Session`] (matching
/// the CCT-417 `authorize_session` semantics) so the framework is real and
/// testable.
pub struct Resource;

impl Resource {
    pub async fn authorize(
        kind: ResourceKind,
        ctx: &AuthContext,
        _action: Action,
        id: Option<&str>,
        pool: &PgPool,
    ) -> Result<(), StatusCode> {
        if ctx.is_admin() {
            return Ok(());
        }
        let Some(id) = id else {
            // A per-object policy with no resolvable id cannot be satisfied
            // safely → fail closed.
            return Err(StatusCode::FORBIDDEN);
        };
        match kind {
            ResourceKind::Session => Self::authorize_session(ctx, id, pool).await,
            // Other kinds are not migrated onto the guard this ticket; their
            // routes keep their in-handler owner checks and declare
            // `Authenticated`. Reaching here for them is a programming error,
            // so fail closed.
            _ => Err(StatusCode::FORBIDDEN),
        }
    }

    /// Mirrors `routes::admin::authorize_session`: owner via
    /// `sessions.machine_uuid -> machines.user_id`. Owner match → ok; a session
    /// owned by someone else → 403; an unknown/unresolvable session → 404 (so
    /// session-id existence does not leak across users).
    async fn authorize_session(
        ctx: &AuthContext,
        id: &str,
        pool: &PgPool,
    ) -> Result<(), StatusCode> {
        let owner: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT m.user_id \
             FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid \
             WHERE s.id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            tracing::error!("db error (authz session): {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        match owner.flatten() {
            Some(uid) if uid == ctx.user_id => Ok(()),
            Some(_) => Err(StatusCode::FORBIDDEN),
            None => Err(StatusCode::NOT_FOUND),
        }
    }
}

/// A single registered route. The descriptor list of all of these is the route
/// table and the source of truth for the coverage test. The fields are read by
/// that test (and document each route's policy); at runtime enforcement keys
/// off the per-route `Authz` extension, not these records.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct RouteDescriptor {
    pub method: Method,
    pub path: &'static str,
    pub authn: Authn,
    pub authz: Authz,
}

/// Builder that records every route and assembles the axum `Router` from the
/// recorded descriptors. Every `add` demands both axes.
pub struct Routes {
    router: Router<AppState>,
    descriptors: Vec<RouteDescriptor>,
}

impl Routes {
    #[must_use]
    pub fn new() -> Self {
        Self { router: Router::new(), descriptors: Vec::new() }
    }

    /// Register one route. Records its descriptor(s) and attaches the [`Authz`]
    /// policy as a per-route extension so [`authz_layer`] can look it up by the
    /// matched route. There is intentionally no variant that omits the axes.
    ///
    /// `methods` lists every HTTP method served by the `handler` on `path` (so
    /// the descriptor table records one entry per method). A route that serves
    /// `GET`+`POST` on one path passes both; all methods on a path share the
    /// same authn+authz, which is the case for every route in this server.
    #[must_use]
    #[allow(clippy::similar_names, clippy::needless_pass_by_value)]
    pub fn add(
        mut self,
        methods: &[Method],
        path: &'static str,
        handler: MethodRouter<AppState>,
        authn: Authn,
        authz: Authz,
    ) -> Self {
        let policy = Arc::new(authz.clone());
        // Attach the policy to exactly this route's requests. `route_layer`
        // runs only for requests matched to this route, so the extension keys
        // the policy by matched path without a separate lookup table.
        let handler = handler.route_layer(axum::Extension(RoutePolicy(policy)));
        self.router = self.router.route(path, handler);
        for method in methods {
            self.descriptors.push(RouteDescriptor {
                method: method.clone(),
                path,
                authn,
                authz: authz.clone(),
            });
        }
        self
    }

    /// Consume the builder, returning the assembled router and the descriptor
    /// list for tests/inspection.
    pub fn into_parts(self) -> (Router<AppState>, Vec<RouteDescriptor>) {
        (self.router, self.descriptors)
    }
}

impl Default for Routes {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-route extension carrying the route's policy to [`authz_layer`].
#[derive(Clone)]
struct RoutePolicy(Arc<Authz>);

/// Default-deny authorization layer. Runs AFTER `auth_middleware` on the
/// `/api/v1` group: looks up the route's [`Authz`] (attached by [`Routes::add`])
/// and the [`AuthContext`], then enforces. A request reaching this layer with
/// no attached policy is rejected `403` — the fail-closed backstop.
pub async fn authz_layer(request: Request, next: Next) -> Result<Response, StatusCode> {
    let Some(policy) = request.extensions().get::<RoutePolicy>().map(|p| p.0.clone()) else {
        // No policy resolved for a matched route → default deny.
        return Err(StatusCode::FORBIDDEN);
    };
    // Every `/api/v1` route is authenticated by `auth_middleware`, which inserts
    // the context; its absence means the principal was not established.
    let ctx = request.extensions().get::<AuthContext>().cloned().ok_or(StatusCode::UNAUTHORIZED)?;
    let pool = request
        .extensions()
        .get::<crate::auth::AuthConfig>()
        .map(|c| c.pool.clone())
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    // Resolve any path-sourced id BEFORE awaiting so the enforce future borrows
    // nothing from the (non-`Send`) request.
    let id = policy.id_from().and_then(|id_from| resolve_id(&request, id_from));

    policy.enforce(&ctx, id, &pool).await?;
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_api_routes;

    /// The descriptor list IS the route table; walking it guarantees every
    /// route declares both axes and that the framework's invariants hold.
    fn descriptors() -> Vec<RouteDescriptor> {
        build_api_routes().into_parts().1
    }

    #[test]
    fn every_api_route_declares_both_axes() {
        let descs = descriptors();
        assert!(!descs.is_empty(), "route table is empty");
        // `add` cannot be called without both an Authn and an Authz (no
        // overload omits them), so reaching a descriptor proves both are
        // present. This test documents that contract and guards against a route
        // being registered through some other path.
        for d in &descs {
            // A trivially-true read that nonetheless forces every field to be
            // read for every descriptor (both axes + method + path).
            let _ = (&d.method, d.path, d.authn, &d.authz);
        }
        // Every path must start with `/` (it nests under `/api/v1`).
        assert!(descs.iter().all(|d| d.path.starts_with('/')));
    }

    #[test]
    fn no_anonymous_api_routes() {
        // `/health` is the ONLY `Authn::None` route, and it lives outside the
        // `/api/v1` descriptor table (on the outer app). So the `/api/v1` table
        // must contain ZERO `Authn::None` routes — everything is authenticated.
        let none: Vec<_> = descriptors().into_iter().filter(|d| d.authn == Authn::None).collect();
        assert!(
            none.is_empty(),
            "no /api/v1 route may be Authn::None (only /health, which is on the outer app); found: {:?}",
            none.iter().map(|d| d.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn no_api_route_is_public() {
        // Likewise, no `/api/v1` route is `Authz::Public`; `Public` is reserved
        // for `/health` on the outer app.
        let public: Vec<_> = descriptors()
            .into_iter()
            .filter(|d| matches!(d.authz, Authz::Public))
            .map(|d| d.path)
            .collect();
        assert!(public.is_empty(), "no /api/v1 route may be Authz::Public; found: {public:?}");
    }

    #[test]
    fn resource_id_param_appears_in_path() {
        // Each `Resource(_, _, IdFrom::Path(name))` must source from a param
        // that actually exists in the route's path (`{name}`), or the id can
        // never be resolved at runtime.
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
    fn custom_and_scope_routes_are_enumerated() {
        // Enumerate the non-`Authenticated` policies so any change to who-can-do
        // -what is visible in this test's expectations. `Custom` routes (none
        // today) must be listed explicitly when added.
        let mut custom: Vec<&'static str> = Vec::new();
        let mut scoped: Vec<(&'static str, String)> = Vec::new();
        for d in descriptors() {
            match &d.authz {
                Authz::Custom(_) => custom.push(d.path),
                Authz::Scope(s) => scoped.push((d.path, s.to_string())),
                _ => {}
            }
        }
        // No Custom escape-hatch routes exist this ticket; adding one must
        // update this assertion deliberately.
        assert!(custom.is_empty(), "unexpected Custom authz route(s): {custom:?}");
        // Scope-gated routes — the capability gates. (Deduped by path; multiple
        // methods on a path share one policy.)
        scoped.sort();
        scoped.dedup();
        assert!(
            scoped.iter().all(|(_, s)| matches!(s.as_str(), "dispatch" | "enroll" | "admin")),
            "unexpected scope on a route: {scoped:?}"
        );
        // Sanity: the admin surface and the dispatch/enroll gates are present.
        assert!(scoped.iter().any(|(_, s)| s == "admin"), "no admin-scoped route registered");
        assert!(scoped.iter().any(|(_, s)| s == "dispatch"), "no dispatch-scoped route");
        assert!(scoped.iter().any(|(_, s)| s == "enroll"), "no enroll-scoped route");
    }

    #[tokio::test]
    async fn resource_session_guard_admin_short_circuits() {
        // The `Resource(Session)` guard admits an admin without touching the DB
        // (mirrors `authorize_session`). Non-admin paths require a live pool and
        // are covered by the in-handler `authorize_session` tests.
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        let admin = AuthContext {
            user_id: Uuid::nil(),
            key_id: Uuid::nil(),
            machine_id: None,
            scopes: Scope::all().into_iter().collect(),
        };
        assert!(
            Resource::authorize(
                ResourceKind::Session,
                &admin,
                Action::Write,
                Some("sess-1"),
                &pool,
            )
            .await
            .is_ok()
        );
        // A per-object policy with no resolvable id fails closed.
        let user = AuthContext {
            user_id: Uuid::new_v4(),
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: std::iter::once(Scope::Read).collect(),
        };
        assert_eq!(
            Resource::authorize(ResourceKind::Session, &user, Action::Read, None, &pool).await,
            Err(StatusCode::FORBIDDEN)
        );
        // An un-migrated resource kind also fails closed for non-admins.
        assert_eq!(
            Resource::authorize(ResourceKind::Account, &user, Action::Read, Some("x"), &pool).await,
            Err(StatusCode::FORBIDDEN)
        );
    }
}
