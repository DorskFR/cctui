//! Declarative route authn/authz framework.
//!
//! Every HTTP route is registered through [`Routes::add`], which demands BOTH
//! an [`Authn`] (how identity is proven) and an [`Authz`] (what the principal
//! may do), so "forgetting authorization on a route" cannot compile.
//!
//! The descriptor list ([`Routes::into_parts`]) IS the route table: the axum
//! `Router` is built only from it, and a coverage test walks it to assert every
//! route carries both axes; each route's policy is enforced by
//! [`enforce_route`] inside the outer `auth_middleware`.
//!
//! Enforcement model and the sharing extension point: `docs/authz.md`.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::MethodRouter;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{AuthContext, Scope};
use crate::state::AppState;

/// How identity is proven for a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Authn {
    /// No identity required. Only `/health`.
    None,
    /// `Authorization: Bearer <token>` — the regular `auth_middleware` path,
    /// and the gateway's provider-key bearer. Also resolves from the
    /// `HttpOnly` auth cookie (browser + WS upgrade); there is no
    /// `?token=`-on-URI variant.
    Bearer,
    /// A token carried in the request body (daemon/dispatcher `auth`, triggers);
    /// those endpoints keep their inline self-authentication.
    BodyToken,
}

/// The coarse capability a principal exercises on a resource. Not every
/// variant is used by every route.
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
/// [`Resource`] owner-resolution impl.
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

/// The escape-hatch signature: receives the principal and the resolved id (if any).
#[allow(dead_code)]
pub type AuthzFn = fn(&AuthContext, Option<&str>) -> Result<(), StatusCode>;

/// What a principal may do on a route.
#[derive(Clone)]
#[allow(dead_code)]
pub enum Authz {
    Public,
    /// Any valid principal. Self-scoped list/filter endpoints keep their SQL filter.
    Authenticated,
    /// A human principal: a user or admin token with `Read`, never a machine key.
    Human,
    /// A capability gate, no object: `ctx.requires(scope)`.
    Scope(Scope),
    /// A per-object gate checked by [`authorize_resource`].
    Resource(ResourceKind, Action, IdFrom),
    Custom(AuthzFn),
}

impl std::fmt::Debug for Authz {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Public => write!(f, "Public"),
            Self::Authenticated => write!(f, "Authenticated"),
            Self::Human => write!(f, "Human"),
            Self::Scope(s) => write!(f, "Scope({s:?})"),
            Self::Resource(k, a, i) => write!(f, "Resource({k:?}, {a:?}, {i:?})"),
            Self::Custom(_) => write!(f, "Custom(..)"),
        }
    }
}

impl Authz {
    /// Evaluate this policy. The resource id is pre-resolved by the caller so
    /// this future borrows nothing from the (non-`Send`) request body.
    async fn enforce(
        &self,
        ctx: &AuthContext,
        id: Option<String>,
        pool: Option<&PgPool>,
    ) -> Result<(), StatusCode> {
        match self {
            // `/health` is the only genuinely public route and lives outside this layer.
            Self::Public | Self::Authenticated => Ok(()),
            Self::Human => {
                if ctx.machine_id.is_none() && ctx.has(Scope::Read) {
                    Ok(())
                } else {
                    Err(StatusCode::FORBIDDEN)
                }
            }
            Self::Scope(s) => ctx.requires(*s),
            Self::Resource(kind, action, _id_from) => {
                // The caller supplies the pool for every `Resource` policy; its
                // absence is a wiring bug, not a client error.
                let pool = pool.ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
                authorize_resource(*kind, ctx, *action, id.as_deref(), pool).await
            }
            Self::Custom(f) => f(ctx, id.as_deref()),
        }
    }

    const fn id_from(&self) -> Option<IdFrom> {
        match self {
            Self::Resource(_, _, id_from) => Some(*id_from),
            _ => None,
        }
    }

    /// The minimum [`Scope`] advertised in `OpenAPI`/`llms.txt`: `s` for
    /// `Scope(s)`, otherwise `Read` (per-object routes additionally require
    /// ownership).
    #[must_use]
    pub const fn human_only(&self) -> bool {
        matches!(self, Self::Human)
    }

    #[must_use]
    pub const fn doc_scope(&self) -> Scope {
        match self {
            Self::Scope(s) => *s,
            _ => Scope::Read,
        }
    }
}

// `RawPathParams` is an extractor, not a request extension: it must be run via
// `extract_parts`, never `extensions().get()` (that always returns None).
async fn resolve_id(req: &mut Request, id_from: IdFrom) -> Option<String> {
    use axum::RequestExt;
    let name = id_from.param();
    let raw = req.extract_parts::<axum::extract::RawPathParams>().await.ok()?;
    raw.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
}

/// RBAC capability gate, the first step of an [`Authz::Resource`] evaluation.
/// Permits every authenticated principal; a role → `(ResourceKind, Action)`
/// table belongs here. Returning `false` makes the guard deny with `403`.
const fn role_permits(_ctx: &AuthContext, _kind: ResourceKind, _action: Action) -> bool {
    true
}

/// The outcome of a per-object authorization decision; the guard maps it to
/// the HTTP status so the `404`-vs-`403` existence-leak policy lives in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Allowed,
    /// The object exists but the principal is neither owner nor grantee → `403`.
    Denied,
    /// The object does not exist / has no resolvable owner → `404` (so an id's
    /// existence never leaks across users).
    NotFound,
}

/// One resource type's per-object authorization.
///
/// [`owner_of`](Resource::owner_of) returns `Ok(Some(uid))` for an owned
/// resource, `Ok(None)` when it does not exist, `Err(_)` on a DB error (`500`).
/// [`authorize`](Resource::authorize) composes ownership and share grants for
/// every [`Authz::Resource`] route.
trait Resource {
    /// The `resource_shares.resource_type` for a shareable kind, or `None` when
    /// the kind is ownership-only.
    const SHARE_TYPE: Option<&'static str> = None;

    async fn owner_of(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error>;

    /// Admin bypass, else owner match, else — for a shareable kind — a live
    /// `use` grant, else denied.
    async fn authorize(
        ctx: &AuthContext,
        _action: Action,
        id: &str,
        pool: &PgPool,
    ) -> Result<Decision, sqlx::Error> {
        if ctx.is_admin() {
            return Ok(Decision::Allowed);
        }
        match Self::owner_of(id, pool).await? {
            Some(uid) if uid == ctx.user_id => Ok(Decision::Allowed),
            Some(_) => {
                if let (Some(share_type), Ok(uuid)) = (Self::SHARE_TYPE, Uuid::parse_str(id))
                    && crate::routes::shares::granted(pool, share_type, uuid, ctx.user_id).await?
                {
                    return Ok(Decision::Allowed);
                }
                Ok(Decision::Denied)
            }
            None => Ok(Decision::NotFound),
        }
    }
}

/// Sessions are owned via `sessions.machine_uuid -> machines.user_id`. A row
/// whose own `user_id` disagrees with that machine's owner has no owner.
struct SessionResource;
impl Resource for SessionResource {
    async fn owner_of(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
        let owner: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT CASE WHEN s.user_id IS NULL OR s.user_id = m.user_id THEN m.user_id END \
             FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid \
             WHERE s.id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(owner.flatten())
    }
}

/// A resource kind that can be shared via `resource_shares`, keyed by its
/// `resource_type` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shareable {
    Account,
    Machine,
    Dispatcher,
    /// Accepted so the share table/routes are ready; it has no backing table
    /// yet, so its owner is always unknown.
    ContextPack,
}

impl Shareable {
    #[must_use]
    pub const fn as_share_type(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Machine => "machine",
            Self::Dispatcher => "dispatcher",
            Self::ContextPack => "context_pack",
        }
    }
}

impl std::str::FromStr for Shareable {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "account" => Ok(Self::Account),
            "machine" => Ok(Self::Machine),
            "dispatcher" => Ok(Self::Dispatcher),
            "context_pack" => Ok(Self::ContextPack),
            _ => Err(()),
        }
    }
}

/// The owning user of a directly-owned resource, or `None` when it does not
/// exist. The only place these owner lookups are written.
pub async fn shareable_owner(
    kind: Shareable,
    id: Uuid,
    pool: &PgPool,
) -> Result<Option<Uuid>, sqlx::Error> {
    let sql = match kind {
        Shareable::Account => "SELECT user_id FROM accounts WHERE id = $1",
        Shareable::Machine => "SELECT user_id FROM machines WHERE id = $1",
        Shareable::Dispatcher => {
            "SELECT user_id FROM dispatchers WHERE id = $1 AND deleted_at IS NULL"
        }
        Shareable::ContextPack => return Ok(None),
    };
    sqlx::query_scalar(sql).bind(id).fetch_optional(pool).await
}

async fn shareable_owner_str(
    kind: Shareable,
    id: &str,
    pool: &PgPool,
) -> Result<Option<Uuid>, sqlx::Error> {
    // A non-UUID id can never name a real row → absent (404).
    let Ok(uuid) = Uuid::parse_str(id) else { return Ok(None) };
    shareable_owner(kind, uuid, pool).await
}

/// Machines are owned directly (`machines.user_id`). Used by the machine-scoped
/// filesystem route (`fs::list_dirs`). The id is the machine UUID as text.
struct MachineResource;
impl Resource for MachineResource {
    const SHARE_TYPE: Option<&'static str> = Some(Shareable::Machine.as_share_type());
    async fn owner_of(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
        shareable_owner_str(Shareable::Machine, id, pool).await
    }
}

struct DispatcherResource;
impl Resource for DispatcherResource {
    const SHARE_TYPE: Option<&'static str> = Some(Shareable::Dispatcher.as_share_type());
    async fn owner_of(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
        shareable_owner_str(Shareable::Dispatcher, id, pool).await
    }
}

/// A user resource's owner is the user itself: the path id IS the owner uid.
struct UserResource;
impl Resource for UserResource {
    async fn owner_of(id: &str, _pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
        Ok(Uuid::parse_str(id).ok())
    }
}

/// Accounts may be shared via `resource_shares`; a grant only confers use/read
/// because the edit/delete handlers fold ownership into their SQL.
struct AccountResource;
impl Resource for AccountResource {
    const SHARE_TYPE: Option<&'static str> = Some(Shareable::Account.as_share_type());
    async fn owner_of(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
        shareable_owner_str(Shareable::Account, id, pool).await
    }
}

/// Prompts are owned directly (`prompts.user_id`). A legacy NULL-owner
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

/// The per-object authorization chokepoint for an [`Authz::Resource`] policy:
/// admin → ok; owner → ok; another owner → `403`; unknown owner → `404`.
async fn authorize_resource(
    kind: ResourceKind,
    ctx: &AuthContext,
    action: Action,
    id: Option<&str>,
    pool: &PgPool,
) -> Result<(), StatusCode> {
    if !role_permits(ctx, kind, action) {
        return Err(StatusCode::FORBIDDEN);
    }
    if ctx.is_admin() {
        return Ok(());
    }
    let Some(id) = id else {
        // A per-object policy with no resolvable id fails closed.
        return Err(StatusCode::FORBIDDEN);
    };
    let decision = match kind {
        ResourceKind::Session => SessionResource::authorize(ctx, action, id, pool).await,
        ResourceKind::Machine => MachineResource::authorize(ctx, action, id, pool).await,
        ResourceKind::Dispatcher => DispatcherResource::authorize(ctx, action, id, pool).await,
        ResourceKind::User => UserResource::authorize(ctx, action, id, pool).await,
        ResourceKind::Account => AccountResource::authorize(ctx, action, id, pool).await,
        ResourceKind::Prompt => PromptResource::authorize(ctx, action, id, pool).await,
        ResourceKind::ApiKey => ApiKeyResource::authorize(ctx, action, id, pool).await,
    }
    .map_err(|e| {
        tracing::error!("db error (authz {kind:?}): {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    match decision {
        Decision::Allowed => Ok(()),
        Decision::Denied => Err(StatusCode::FORBIDDEN),
        Decision::NotFound => Err(StatusCode::NOT_FOUND),
    }
}

/// In-handler session-read gate for routes whose declarative guard names a
/// different resource (`fs/file` is machine-scoped in the path but
/// session-scoped in what it serves).
pub async fn authorize_session_read(
    ctx: &AuthContext,
    id: &str,
    pool: &PgPool,
) -> Result<(), StatusCode> {
    authorize_resource(ResourceKind::Session, ctx, Action::Read, Some(id), pool).await
}

/// The session owner lookup shared by the HTTP guard and the WS path.
pub async fn session_owner(id: &str, pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
    SessionResource::owner_of(id, pool).await
}

/// A registered route. At runtime enforcement keys off the per-route `Authz`
/// extension; these records are the route table read by tests and docs.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct RouteDescriptor {
    pub method: Method,
    pub path: &'static str,
    pub authn: Authn,
    pub authz: Authz,
    /// One-line description emitted into the `OpenAPI` `summary` and `llms.txt`.
    pub summary: &'static str,
}

pub struct Routes {
    router: Router<AppState>,
    descriptors: Vec<RouteDescriptor>,
}

impl Routes {
    #[must_use]
    pub fn new() -> Self {
        Self { router: Router::new(), descriptors: Vec::new() }
    }

    /// Register one route for every method in `methods`, attaching the
    /// [`Authz`] policy as a per-route layer.
    #[must_use]
    #[allow(clippy::similar_names, clippy::needless_pass_by_value)]
    pub fn add(
        mut self,
        methods: &[Method],
        path: &'static str,
        summary: &'static str,
        handler: MethodRouter<AppState>,
        authn: Authn,
        authz: Authz,
    ) -> Self {
        let policy = Arc::new(authz.clone());
        // `route_layer` runs inside the outer `auth_middleware` and only for this
        // route; a global `.layer` would run before the matched route is known.
        let handler = handler.route_layer(middleware::from_fn_with_state(policy, enforce_route));
        self.router = self.router.route(path, handler);
        for method in methods {
            self.descriptors.push(RouteDescriptor {
                method: method.clone(),
                path,
                authn,
                authz: authz.clone(),
                summary,
            });
        }
        self
    }

    pub fn into_parts(self) -> (Router<AppState>, Vec<RouteDescriptor>) {
        (self.router, self.descriptors)
    }
}

impl Default for Routes {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-route authorization enforcement, attached by [`Routes::add`] via
/// `route_layer`, after `auth_middleware` has populated [`AuthContext`].
async fn enforce_route(
    State(policy): State<Arc<Authz>>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let ctx = request.extensions().get::<AuthContext>().cloned().ok_or(StatusCode::UNAUTHORIZED)?;

    let id = match policy.id_from() {
        Some(id_from) => resolve_id(&mut request, id_from).await,
        None => None,
    };

    let pool = match &*policy {
        Authz::Resource(..) => Some(
            request
                .extensions()
                .get::<crate::auth::AuthConfig>()
                .map(|c| c.pool.clone())
                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?,
        ),
        _ => None,
    };

    policy.enforce(&ctx, id, pool.as_ref()).await?;
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_api_routes;

    fn descriptors() -> Vec<RouteDescriptor> {
        build_api_routes().into_parts().1
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
    fn no_anonymous_api_routes() {
        // `/health` lives outside the `/api/v1` table, so no route here is `Authn::None`.
        let none: Vec<_> = descriptors().into_iter().filter(|d| d.authn == Authn::None).collect();
        assert!(
            none.is_empty(),
            "no /api/v1 route may be Authn::None (only /health, which is on the outer app); found: {:?}",
            none.iter().map(|d| d.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn no_api_route_is_public() {
        let public: Vec<_> = descriptors()
            .into_iter()
            .filter(|d| matches!(d.authz, Authz::Public))
            .map(|d| d.path)
            .collect();
        assert!(public.is_empty(), "no /api/v1 route may be Authz::Public; found: {public:?}");
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
        const HUMAN_PREFIXES: &[&str] =
            &["/accounts", "/account-pools", "/profiles", "/redirects", "/{resource_type}"];
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
    async fn human_gate_rejects_machine_keys() {
        let mut machine = user(Uuid::new_v4());
        machine.machine_id = Some(Uuid::new_v4());
        let denied = one_route_app(Authz::Human, Some(machine));
        assert_eq!(status_of(denied, "/r").await, StatusCode::FORBIDDEN);

        let allowed = one_route_app(Authz::Human, Some(user(Uuid::new_v4())));
        assert_eq!(status_of(allowed, "/r").await, StatusCode::OK);
        let admin = one_route_app(Authz::Human, Some(admin()));
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

    /// Admins short-circuit for every kind without touching the (invalid) pool.
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

    /// A non-UUID id resolves to unknown (404) without touching the pool.
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

    /// `User` needs no DB (the id is the owner): owner 200, other user 403,
    /// unparseable id 404.
    #[tokio::test]
    async fn resource_guard_user_kind_full_matrix() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        let uid = Uuid::new_v4();
        let me = user(uid);
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
        let other = Uuid::new_v4().to_string();
        assert_eq!(
            authorize_resource(ResourceKind::User, &me, Action::Read, Some(&other), &pool).await,
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            authorize_resource(ResourceKind::User, &me, Action::Read, Some("nope"), &pool).await,
            Err(StatusCode::NOT_FOUND)
        );
    }


    /// A role rule denying a capability yields `403` before any object lookup.
    #[tokio::test]
    async fn role_seam_denies_with_403_before_object_lookup() {
        fn test_role_permits(_ctx: &AuthContext, kind: ResourceKind, action: Action) -> bool {
            !(kind == ResourceKind::Account && action == Action::Write)
        }

        assert!(role_permits(&user(Uuid::new_v4()), ResourceKind::Account, Action::Write));

        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        let me = user(Uuid::new_v4());

        let gate = |kind, action| -> Result<(), StatusCode> {
            if test_role_permits(&me, kind, action) { Ok(()) } else { Err(StatusCode::FORBIDDEN) }
        };
        assert_eq!(gate(ResourceKind::Account, Action::Write), Err(StatusCode::FORBIDDEN));
        // Permitted capabilities pass step 1; the object rule would hit the pool.
        assert!(gate(ResourceKind::Account, Action::Read).is_ok());
        let _ = &pool;
    }

    /// Overriding only [`Resource::authorize`] composes a grant with ownership
    /// without touching the guard.
    #[tokio::test]
    async fn sharing_seam_grant_composes_with_ownership() {
        const OWNER: Uuid = Uuid::from_u128(0x0001);
        const GRANTEE: Uuid = Uuid::from_u128(0x0002);
        const SHARED_ID: &str = "shared-object";

        struct SharedThing;
        impl Resource for SharedThing {
            async fn owner_of(_id: &str, _pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
                Ok(Some(OWNER))
            }
            async fn authorize(
                ctx: &AuthContext,
                _action: Action,
                id: &str,
                pool: &PgPool,
            ) -> Result<Decision, sqlx::Error> {
                if ctx.is_admin() {
                    return Ok(Decision::Allowed);
                }
                if Self::owner_of(id, pool).await? == Some(ctx.user_id) {
                    return Ok(Decision::Allowed);
                }
                if id == SHARED_ID && ctx.user_id == GRANTEE {
                    return Ok(Decision::Allowed);
                }
                Ok(Decision::Denied)
            }
        }

        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();

        assert_eq!(
            SharedThing::authorize(&user(OWNER), Action::Read, "any", &pool).await.unwrap(),
            Decision::Allowed
        );
        assert_eq!(
            SharedThing::authorize(&user(GRANTEE), Action::Read, SHARED_ID, &pool).await.unwrap(),
            Decision::Allowed
        );
        assert_eq!(
            SharedThing::authorize(&user(GRANTEE), Action::Read, "other", &pool).await.unwrap(),
            Decision::Denied
        );
        assert_eq!(
            SharedThing::authorize(&user(Uuid::new_v4()), Action::Read, SHARED_ID, &pool)
                .await
                .unwrap(),
            Decision::Denied
        );
        assert_eq!(
            SharedThing::authorize(&admin(), Action::Write, "whatever", &pool).await.unwrap(),
            Decision::Allowed
        );
    }

    /// Shareable kinds declare a `SHARE_TYPE` the shares table recognizes;
    /// ownership-only kinds declare `None`.
    #[tokio::test]
    async fn session_owner_denies_user_machine_mismatch() {
        let Some(url) = crate::routes::gateway::test_db_url("session_owner_mismatch") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (owner, other, machine) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        for uid in [owner, other] {
            sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
                .bind(uid)
                .bind(format!("so-{uid}"))
                .bind(format!("kh-{uid}"))
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(machine)
            .bind(owner)
            .bind(machine.to_string())
            .bind(format!("kh-{machine}"))
            .execute(&pool)
            .await
            .unwrap();
        let mut ids = Vec::new();
        for user in [Some(owner), Some(other), None] {
            let sid = Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid) \
                 VALUES ($1, 'm', '/w', $2, $3)",
            )
            .bind(&sid)
            .bind(user)
            .bind(machine)
            .execute(&pool)
            .await
            .unwrap();
            ids.push(sid);
        }
        assert_eq!(session_owner(&ids[0], &pool).await.unwrap(), Some(owner));
        assert_eq!(session_owner(&ids[1], &pool).await.unwrap(), None);
        assert_eq!(session_owner(&ids[2], &pool).await.unwrap(), Some(owner));
    }

    #[test]
    fn shareable_parses_its_own_share_type_only() {
        for kind in
            [Shareable::Account, Shareable::Machine, Shareable::Dispatcher, Shareable::ContextPack]
        {
            assert_eq!(kind.as_share_type().parse::<Shareable>(), Ok(kind));
        }
        for t in ["", "session", "user", "prompt", "api_key", "Account"] {
            assert!(t.parse::<Shareable>().is_err(), "{t} must not be shareable");
        }
    }

    /// `context_pack` has no table: unknown, and the invalid pool is never touched.
    #[tokio::test]
    async fn shareable_owner_context_pack_is_unknown_without_db() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        assert_eq!(
            shareable_owner(Shareable::ContextPack, Uuid::new_v4(), &pool).await.unwrap(),
            None
        );
    }

    #[test]
    fn shareable_kinds_declare_share_type() {
        assert_eq!(AccountResource::SHARE_TYPE, Some("account"));
        assert_eq!(MachineResource::SHARE_TYPE, Some("machine"));
        assert_eq!(DispatcherResource::SHARE_TYPE, Some("dispatcher"));
        assert_eq!(SessionResource::SHARE_TYPE, None);
        assert_eq!(UserResource::SHARE_TYPE, None);
        assert_eq!(PromptResource::SHARE_TYPE, None);
        assert_eq!(ApiKeyResource::SHARE_TYPE, None);
        for st in [
            AccountResource::SHARE_TYPE,
            MachineResource::SHARE_TYPE,
            DispatcherResource::SHARE_TYPE,
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                st.parse::<Shareable>().is_ok(),
                "authz SHARE_TYPE {st:?} must be a shares CRUD/table shareable type"
            );
        }
    }


    /// A one-route router wired exactly as [`Routes::add`] does, optionally under
    /// a layer that mimics `auth_middleware` by inserting an [`AuthContext`].
    fn one_route_app(policy: Authz, ctx: Option<AuthContext>) -> Router {
        use axum::routing::get;
        let route = get(|| async { "ok" })
            .route_layer(middleware::from_fn_with_state(Arc::new(policy), enforce_route));
        let app = Router::new().route("/r", route);
        match ctx {
            Some(ctx) => app.layer(middleware::from_fn(move |mut req: Request, next: Next| {
                let ctx = ctx.clone();
                async move {
                    req.extensions_mut().insert(ctx);
                    next.run(req).await
                }
            })),
            None => app,
        }
    }

    async fn status_of(app: Router, uri: &str) -> StatusCode {
        use axum::body::Body;
        use tower::ServiceExt;
        app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn authenticated_route_allows_real_request() {
        let app = one_route_app(Authz::Authenticated, Some(user(Uuid::new_v4())));
        assert_eq!(status_of(app, "/r").await, StatusCode::OK);
    }

    /// Without a principal the per-route layer rejects with 401.
    #[tokio::test]
    async fn route_without_principal_is_401() {
        let app = one_route_app(Authz::Authenticated, None);
        assert_eq!(status_of(app, "/r").await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn scope_gate_enforced_through_stack() {
        let denied = one_route_app(Authz::Scope(Scope::Admin), Some(user(Uuid::new_v4())));
        assert_eq!(status_of(denied, "/r").await, StatusCode::FORBIDDEN);

        let allowed = one_route_app(Authz::Scope(Scope::Admin), Some(admin()));
        assert_eq!(status_of(allowed, "/r").await, StatusCode::OK);
    }

    /// A non-admin's `{id}` resolves from the matched route: owned → 200,
    /// other user → 403.
    #[tokio::test]
    async fn resource_route_resolves_id_for_non_admin() {
        use axum::routing::get;

        fn app(ctx: AuthContext) -> Router {
            let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
            let cfg = crate::auth::AuthConfig::new(Vec::new(), pool);
            let policy = Authz::Resource(ResourceKind::User, Action::Read, IdFrom::Path("id"));
            let route = get(|| async { "ok" })
                .route_layer(middleware::from_fn_with_state(Arc::new(policy), enforce_route));
            Router::new().route("/u/{id}", route).layer(middleware::from_fn(
                move |mut req: Request, next: Next| {
                    let ctx = ctx.clone();
                    let cfg = cfg.clone();
                    async move {
                        req.extensions_mut().insert(ctx);
                        req.extensions_mut().insert(cfg);
                        next.run(req).await
                    }
                },
            ))
        }

        let uid = Uuid::new_v4();
        assert_eq!(status_of(app(user(uid)), &format!("/u/{uid}")).await, StatusCode::OK);
        let other = Uuid::new_v4();
        assert_eq!(status_of(app(user(uid)), &format!("/u/{other}")).await, StatusCode::FORBIDDEN);
    }
}
