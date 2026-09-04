//! Browser auth-cookie endpoints.
//!
//! The bearer token rides an `HttpOnly`+`SameSite`+`Secure` cookie, not the WS
//! upgrade `?token=` query (which leaks into proxy/access logs). These two
//! endpoints move the credential into that cookie:
//! `login` validates a presented token and sets the cookie; `logout` clears it.
//! Both live OUTSIDE the `auth_middleware` group (login cannot require prior
//! auth). After login, `auth_middleware` and the WS upgrade resolve the cookie
//! via [`crate::auth::bearer_or_cookie`], and the webui reuses `GET /me` to probe
//! whether the cookie session is still valid.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::auth;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct LoginRequest {
    pub token: String,
}

/// `POST /api/v1/auth/login` — exchange a valid bearer token for the `HttpOnly`
/// auth cookie. Unauthenticated by design: it validates the supplied token
/// itself and only sets the cookie when it resolves to a real principal.
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Response {
    if state.auth_config.validate(&req.token).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let cookie = auth::set_auth_cookie(&req.token, auth::request_is_https(&headers));
    ([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response()
}

/// `POST /api/v1/auth/logout` — expire the auth cookie. Always succeeds; the
/// cookie is the only browser-side state to clear.
///
/// A session opened with a passkey also holds a server-side key minted for that
/// session alone, so logout revokes it too — otherwise signing out would leave a
/// live credential behind. A token session is untouched: its key is the user's,
/// and they did not ask us to throw it away.
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = auth::bearer_or_cookie(&headers) {
        crate::routes::passkeys::revoke_session_key(&state.pool, &token).await;
        state.auth_config.purge(&auth::sha256_hex(&token));
    }
    let cookie = auth::clear_auth_cookie(auth::request_is_https(&headers));
    ([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response()
}
