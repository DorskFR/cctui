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
//! CCT-420 centralizes per-OBJECT ownership into the [`Resource`] guard: the
//! single-object session routes and the machine-scoped fs route declare
//! [`Authz::Resource`], and the in-handler owner checks are gone (the guard is
//! authoritative). Routes whose authorization cannot be expressed as a yes/no
//! gate — self-scoped list/search/stats endpoints (the `owner_filter()` SQL
//! filter) and batch endpoints (`filter_owned_ids`) — declare
//! [`Authz::Authenticated`] and keep their filter in the handler; they are
//! enumerated in the coverage test. A handful of single-object routes
//! (accounts/dispatchers/prompts/keys) also stay `Authenticated`/`Scope`
//! because they fold ownership into the mutating SQL's `WHERE` clause and return
//! `404` (not `403`) for a cross-user id — moving them onto the guard would
//! change that client-visible semantics, so they are intentionally left inline.

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

/// The kinds of resource the per-object guard knows about. Each has a
/// [`Resource`] owner-resolution impl (CCT-420). `Session` and `Machine` are
/// wired onto HTTP routes; the rest are implemented for completeness and are
/// reachable via the guard once their routes adopt it (their single-object
/// routes currently fold ownership into the SQL `WHERE` clause — see the module
/// doc).
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

/// What a principal may do on a route. `Resource` is now used by the migrated
/// per-object routes (sessions + the machine fs route, CCT-420). `Public` and
/// `Custom` remain part of the framework's surface but are unused by any
/// `/api/v1` route (`/health` is `Public` on the outer app; no `Custom` rule
/// exists yet), so they are constructed only in tests.
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
    /// and checked by [`authorize_resource`].
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
                authorize_resource(*kind, ctx, *action, id.as_deref(), pool).await
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

/// One resource type's owner-resolution. The single per-resource
/// authorization primitive: written **once per resource type, not per
/// endpoint** — the owning-user DB lookup for that kind lives here, nowhere
/// else. The generic guard ([`authorize_resource`]) composes the default rule
/// (`admin || owner(id) == principal`) on top of it, so a resource impl only
/// has to answer "who owns this id?".
///
/// `owner_of` returns:
///   * `Ok(Some(uid))` — the resource exists and `uid` owns it;
///   * `Ok(None)` — the resource does not exist (or has no resolvable owner),
///     which the guard maps to `404` so an id's existence never leaks across
///     users;
///   * `Err(_)` — a DB error, mapped to `500`.
///
/// CCT-422 will layer an RBAC `(kind, action)` capability check in front of the
/// ownership rule; the `Action` is already carried on the descriptor for that.
trait Resource {
    async fn owner_of(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error>;
}

/// Sessions are owned via `sessions.machine_uuid -> machines.user_id`
/// (CCT-417). Mirrors `routes::admin::authorize_session` and `ws::ws_owns_session`.
struct SessionResource;
impl Resource for SessionResource {
    async fn owner_of(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
        let owner: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT m.user_id \
             FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid \
             WHERE s.id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(owner.flatten())
    }
}

/// Machines are owned directly (`machines.user_id`). Used by the machine-scoped
/// filesystem route (`fs::list_dirs`). The id is the machine UUID as text.
struct MachineResource;
impl Resource for MachineResource {
    async fn owner_of(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
        let Ok(uuid) = Uuid::parse_str(id) else {
            // A non-UUID id can never name a real machine → treat as absent
            // (404), matching the in-handler `Uuid::parse_str` + not-found path.
            return Ok(None);
        };
        sqlx::query_scalar("SELECT user_id FROM machines WHERE id = $1")
            .bind(uuid)
            .fetch_optional(pool)
            .await
    }
}

/// Dispatchers are owned directly (`dispatchers.user_id`).
struct DispatcherResource;
impl Resource for DispatcherResource {
    async fn owner_of(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
        let Ok(uuid) = Uuid::parse_str(id) else { return Ok(None) };
        sqlx::query_scalar("SELECT user_id FROM dispatchers WHERE id = $1 AND deleted_at IS NULL")
            .bind(uuid)
            .fetch_optional(pool)
            .await
    }
}

/// A user resource's owner is the user itself: the path id IS the owner uid.
struct UserResource;
impl Resource for UserResource {
    async fn owner_of(id: &str, _pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
        Ok(Uuid::parse_str(id).ok())
    }
}

/// OAuth accounts are owned directly (`oauth_accounts.user_id`).
struct AccountResource;
impl Resource for AccountResource {
    async fn owner_of(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
        let Ok(uuid) = Uuid::parse_str(id) else { return Ok(None) };
        sqlx::query_scalar("SELECT user_id FROM oauth_accounts WHERE id = $1")
            .bind(uuid)
            .fetch_optional(pool)
            .await
    }
}

/// Prompts are owned directly (`prompts.user_id`, CCT-418). A legacy NULL-owner
/// row resolves to `None` → 404 for non-admins (admins short-circuit earlier).
struct PromptResource;
impl Resource for PromptResource {
    async fn owner_of(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
        let Ok(uuid) = Uuid::parse_str(id) else { return Ok(None) };
        let owner: Option<Option<Uuid>> =
            sqlx::query_scalar("SELECT user_id FROM prompts WHERE id = $1")
                .bind(uuid)
                .fetch_optional(pool)
                .await?;
        Ok(owner.flatten())
    }
}

/// Stored provider keys are owned directly (`api_keys.user_id`, CCT-418).
struct ApiKeyResource;
impl Resource for ApiKeyResource {
    async fn owner_of(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
        let Ok(uuid) = Uuid::parse_str(id) else { return Ok(None) };
        let owner: Option<Option<Uuid>> =
            sqlx::query_scalar("SELECT user_id FROM api_keys WHERE id = $1")
                .bind(uuid)
                .fetch_optional(pool)
                .await?;
        Ok(owner.flatten())
    }
}

/// The single per-object authorization chokepoint, evaluated by [`authz_layer`]
/// for an [`Authz::Resource`] policy. Resolves the resource's owner via the
/// per-kind [`Resource`] impl, then applies the default rule:
/// `admin || owner(id) == principal`.
///
/// * admin → ok without a DB lookup;
/// * owner match → ok;
/// * a resource owned by someone else → `403`;
/// * an unknown / unresolvable resource (owner `None`) → `404`, so a resource
///   id's existence never leaks across users.
async fn authorize_resource(
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
        // A per-object policy with no resolvable id cannot be satisfied safely
        // → fail closed.
        return Err(StatusCode::FORBIDDEN);
    };
    let owner = match kind {
        ResourceKind::Session => SessionResource::owner_of(id, pool).await,
        ResourceKind::Machine => MachineResource::owner_of(id, pool).await,
        ResourceKind::Dispatcher => DispatcherResource::owner_of(id, pool).await,
        ResourceKind::User => UserResource::owner_of(id, pool).await,
        ResourceKind::Account => AccountResource::owner_of(id, pool).await,
        ResourceKind::Prompt => PromptResource::owner_of(id, pool).await,
        ResourceKind::ApiKey => ApiKeyResource::owner_of(id, pool).await,
    }
    .map_err(|e| {
        tracing::error!("db error (authz {kind:?}): {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    match owner {
        Some(uid) if uid == ctx.user_id => Ok(()),
        Some(_) => Err(StatusCode::FORBIDDEN),
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// Public re-export of the session owner lookup so the WS path (`ws.rs`) can
/// reuse the exact same ownership query as the HTTP guard (one authorizer, two
/// transports — CCT-416). Returns the owning user, or `None` for an
/// unknown/unresolvable session.
pub async fn session_owner(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
    SessionResource::owner_of(id, pool).await
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

    fn admin() -> AuthContext {
        AuthContext {
            user_id: Uuid::nil(),
            key_id: Uuid::nil(),
            machine_id: None,
            scopes: Scope::all().into_iter().collect(),
        }
    }

    fn user(uid: Uuid) -> AuthContext {
        AuthContext {
            user_id: uid,
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: std::iter::once(Scope::Read).collect(),
        }
    }

    /// Every `ResourceKind` short-circuits to OK for an admin WITHOUT touching
    /// the DB (the invalid pool would error if any kind queried it). This is the
    /// admin-bypass arm of the guard's default rule for all seven kinds.
    #[tokio::test]
    async fn resource_guard_admin_bypasses_every_kind() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        let admin = admin();
        for kind in [
            ResourceKind::Session,
            ResourceKind::Machine,
            ResourceKind::Dispatcher,
            ResourceKind::User,
            ResourceKind::Account,
            ResourceKind::Prompt,
            ResourceKind::ApiKey,
        ] {
            assert!(
                authorize_resource(kind, &admin, Action::Write, Some("any-id"), &pool)
                    .await
                    .is_ok(),
                "admin should bypass {kind:?}"
            );
        }
    }

    /// A per-object policy with no resolvable id fails closed (403) for a
    /// non-admin, without a DB lookup.
    #[tokio::test]
    async fn resource_guard_missing_id_fails_closed() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        assert_eq!(
            authorize_resource(
                ResourceKind::Session,
                &user(Uuid::new_v4()),
                Action::Read,
                None,
                &pool
            )
            .await,
            Err(StatusCode::FORBIDDEN)
        );
    }

    /// A non-UUID id for a UUID-keyed resource resolves to "unknown" → 404
    /// (existence-leak-safe), and does NOT touch the DB (the invalid pool would
    /// error otherwise). Covers the unknown-resource arm for every UUID-keyed
    /// kind.
    #[tokio::test]
    async fn resource_guard_unknown_id_is_404() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        let u = user(Uuid::new_v4());
        for kind in [
            ResourceKind::Machine,
            ResourceKind::Dispatcher,
            ResourceKind::Account,
            ResourceKind::Prompt,
            ResourceKind::ApiKey,
        ] {
            assert_eq!(
                authorize_resource(kind, &u, Action::Read, Some("not-a-uuid"), &pool).await,
                Err(StatusCode::NOT_FOUND),
                "non-UUID id for {kind:?} should be 404"
            );
        }
    }

    /// The `User` resource needs no DB (the id IS the owner), so it exercises
    /// the full owner-match / cross-user / unknown matrix of the default rule
    /// without a live pool: owner allowed, another user 403, an unparseable id
    /// 404.
    #[tokio::test]
    async fn resource_guard_user_kind_full_matrix() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        let uid = Uuid::new_v4();
        let me = user(uid);
        // Owner acts on itself → ok.
        assert!(
            authorize_resource(
                ResourceKind::User,
                &me,
                Action::Write,
                Some(&uid.to_string()),
                &pool
            )
            .await
            .is_ok()
        );
        // Another user → 403 (resource exists, owned by someone else).
        let other = Uuid::new_v4().to_string();
        assert_eq!(
            authorize_resource(ResourceKind::User, &me, Action::Read, Some(&other), &pool).await,
            Err(StatusCode::FORBIDDEN)
        );
        // An id that can't name a user → 404.
        assert_eq!(
            authorize_resource(ResourceKind::User, &me, Action::Read, Some("nope"), &pool).await,
            Err(StatusCode::NOT_FOUND)
        );
    }
}
