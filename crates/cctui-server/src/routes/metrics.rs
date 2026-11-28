//! `GET /metrics` — Prometheus scrape endpoint.
//!
//! Lives on the outer router, outside the `/api/v1` auth layer, because the
//! conventional path for a scrape is the root one and Prometheus cannot be
//! talked out of it. It therefore authenticates itself, with the same token
//! scheme as `/api/v1` — or not at all, when the operator opted in via
//! `CCTUI_METRICS_PUBLIC`.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::state::AppState;
use cctui_server_kernel::auth::{Principal, human_reader};

pub async fn metrics(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let owner = if crate::metrics::public_from_env() {
        None
    } else {
        let Some(token) = cctui_server_kernel::auth::bearer_or_cookie(&headers) else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        match scrape_owner(state.auth_config.validate_principal(&token).await) {
            Ok(owner) => owner,
            Err(status) => return status.into_response(),
        }
    };
    let body = crate::metrics::render(&crate::metrics::snapshot(&state, owner).await);
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], body).into_response()
}

/// Whose rows a scrape may see: everyone's for an admin, the caller's own for
/// any other person, nothing for a machine, dispatcher, worker or plugin key.
fn scrape_owner(principal: Option<Principal>) -> Result<Option<Uuid>, StatusCode> {
    let principal = principal.ok_or(StatusCode::UNAUTHORIZED)?;
    if !human_reader(&principal.ctx, &principal.kind) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(principal.ctx.owner_filter())
}

#[cfg(test)]
mod tests {
    use cctui_server_kernel::auth::{AuthContext, KeyKind, Principal, Scope};

    use super::*;

    fn principal(kind: KeyKind, scopes: &[Scope], machine: bool) -> Principal {
        let ctx = AuthContext {
            user_id: Uuid::new_v4(),
            key_id: Uuid::new_v4(),
            machine_id: machine.then(Uuid::new_v4),
            scopes: scopes.iter().copied().collect(),
        };
        Principal { ctx, bound: None, kind }
    }

    #[test]
    fn a_scrape_sees_only_what_the_caller_owns() {
        assert_eq!(scrape_owner(None), Err(StatusCode::UNAUTHORIZED));

        let admin = principal(KeyKind::Human, &Scope::all(), false);
        assert_eq!(scrape_owner(Some(admin)), Ok(None), "an admin sees the whole instance");

        let user = principal(KeyKind::Human, &[Scope::Read], false);
        let uid = user.ctx.user_id;
        assert_eq!(scrape_owner(Some(user)), Ok(Some(uid)), "a user sees only their own rows");

        for (kind, machine) in [
            (KeyKind::Machine, true),
            (KeyKind::Worker, true),
            (KeyKind::Dispatcher, false),
            (KeyKind::Plugin("ghreview".to_owned()), false),
        ] {
            let key = principal(kind.clone(), &Scope::all(), machine);
            assert_eq!(scrape_owner(Some(key)), Err(StatusCode::FORBIDDEN), "{kind:?}");
        }
    }
}
