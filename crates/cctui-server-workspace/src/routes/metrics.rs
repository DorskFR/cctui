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

use crate::state::AppState;

pub async fn metrics(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !crate::metrics::public_from_env() {
        let Some(token) = crate::auth::bearer_or_cookie(&headers) else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        if state.auth_config.validate(&token).await.is_none() {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    let body = crate::metrics::render(&crate::metrics::snapshot(&state).await);
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], body).into_response()
}
