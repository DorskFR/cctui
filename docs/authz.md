# Route authn/authz

Design notes for `crates/cctui-server/src/authz.rs`.

## Enforcement model

`add` attaches each route's [`Authz`] as a per-route request extension via
`route_layer`. A single [`authz_layer`] middleware, layered on the
authenticated `/api/v1` group AFTER `auth_middleware`, looks up that
extension and evaluates it against the request's [`AuthContext`]:

  * a request that reaches the layer with **no** `Authz` extension is
    rejected `403` — **default deny**, the fail-closed backstop;
  * otherwise the policy is evaluated (see [`Authz::enforce`]).

## Scope

The [`Authn`] axis only records how identity is proven for the coverage
test; it does not re-implement authentication (`auth_middleware` for
`/api/v1`, inline self-auth on the daemon/dispatcher/trigger/gateway
endpoints are untouched). Per-OBJECT ownership is centralized in the
[`Resource`] guard: single-object session routes and the machine-scoped fs
route declare [`Authz::Resource`], with in-handler owner checks gone (the
guard is authoritative). Routes whose authorization can't be a yes/no gate
— self-scoped list/search/stats endpoints (the `owner_filter()` SQL filter)
and batch endpoints (`filter_owned_ids`) — declare [`Authz::Authenticated`]
and keep their filter in the handler; they are enumerated in the coverage
test. A handful of single-object routes (accounts/dispatchers/prompts/keys)
also stay `Authenticated`/`Scope` because they fold ownership into the
mutating SQL's `WHERE` clause and return `404` (not `403`) for a cross-user
id — moving them onto the guard would change that client-visible
semantics, so they are intentionally left inline.

## The full authn × authz model

Two orthogonal, declarative axes are demanded per route:

  * **Authn** — how identity is proven: [`Authn`] `{None, Bearer, BodyToken}`.
    `None` is `/health` only; everything else proves a principal. `Bearer`
    resolves from the `Authorization` header or the `HttpOnly` auth cookie.
  * **Authz** — what the principal may do: [`Authz`] `{Public, Authenticated,
    Human, Scope, Resource, Custom}`.

For an [`Authz::Resource(kind, action, id)`] route the guard
([`authorize_resource`]) evaluates THREE composable steps, all in one place so
no endpoint ever re-implements authorization:

  1. **RBAC capability** ([`role_permits`]): may the principal's role exercise
     `(ResourceKind, Action)` at all? Today the coarse [`Scope`] enforced
     upstream is the only capability, so this is `true` for any authenticated
     principal. A future role→`(kind, action)` table slots in HERE with no
     per-endpoint change — the descriptor already names `kind` and `action`.
  2. **Resource authorization** ([`Resource::authorize`]): may the principal
     act on THIS object? The default rule is `admin || owner(id) == principal`.
     Share grants compose here (see below).
  3. (Self-scoped list/search/stats endpoints can't be a yes/no gate; they
     declare [`Authz::Authenticated`] and apply `owner_filter()` in SQL.)

### Resource sharing extension point (design + seam, not yet implemented)

Ownership is just the FIRST rule in [`Resource::authorize`]. Grants are added
in ONE place — that default method — and every [`Authz::Resource`] route
inherits them automatically, touching neither the guard nor any handler. A
future implementation adds a `shares` table and a grant lookup inside
`authorize`:

```sql
-- shares(resource ResourceKind, id, grantee_user_id NULL, token NULL,
--        action, expires, revoked)  -- DB-backed/revocable preferred
```

```ignore
async fn authorize(ctx, action, id, db) -> Decision {
    if ctx.is_admin() { return Decision::Allowed; }
    if Self::owner_of(id, db).await? == Some(ctx.user_id) { return Decision::Allowed; }
    // grant lookup composes here, no guard/endpoint change:
    // if shares::granted(kind, id, ctx.user_id, action, db).await? { Decision::Allowed }
    Decision::Denied
}
```

Self-scoped list queries would additionally `UNION` shared-in rows.

### `Principal::Share` deeplink tokens (design only, not built)

Deeplink share tokens slot into the **Authn** axis without changing the real
[`AuthContext`] or auth flow. The plan:

  * A new principal variant resolved by the authenticator:
    `Principal::Share { resource: ResourceKind, id: Uuid, action: Action,
    expires: DateTime }`. The token is minted for exactly one
    resource+action+object — least privilege.
  * The guard checks the share principal against the route's declared
    [`Authz::Resource(kind, action, IdFrom)`]: the token is honored ONLY when
    `share.resource == kind && share.action permits action && share.id == the
    resolved id && now < share.expires`. It can do nothing else — a Share
    principal fails every other policy (other resources, other actions, scope
    gates, admin collections).
  * For sensitive session data, DB-backed tokens (a `shares` row) are
    preferred over self-contained JWTs so a share is revocable.

This documents the extension point; `Principal::Share` is not added to
