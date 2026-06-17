//! GitHub HTTP handlers. Skeleton stubs (GH-PKG-1) — real bodies land in the
//! connector/diff/review tickets. They return `501 Not Implemented` so the
//! mounted surface is exercisable end to end without yet implementing behaviour.
//!
//! Handlers stay `async` (axum requires it); the real bodies `.await` GitHub/DB
//! work in the later tickets.
#![allow(clippy::unused_async)]

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::GithubState;

const STUB: (StatusCode, &str) =
    (StatusCode::NOT_IMPLEMENTED, "github integration not yet implemented");

/// `GET /api/v1/github/connectors` — list configured connectors.
pub async fn list_connectors(State(_state): State<GithubState>) -> impl IntoResponse {
    STUB
}

/// `POST /api/v1/github/connectors` — create a connector (encrypted credential).
pub async fn create_connector(State(_state): State<GithubState>) -> impl IntoResponse {
    STUB
}

/// `GET /api/v1/github/pulls` — list tracked pull requests.
pub async fn list_pulls(State(_state): State<GithubState>) -> impl IntoResponse {
    STUB
}

/// `POST /api/v1/triggers/github` — GitHub webhook ingress (sig verify + route).
pub async fn webhook(State(_state): State<GithubState>) -> impl IntoResponse {
    STUB
}
