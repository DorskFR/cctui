//! Provider passthrough endpoints.
use super::{Family, anthropic_upstream, fireworks_upstream, google_upstream, openai_upstream};
pub use crate::gateway::proxy::*;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;

/// `/gateway/anthropic/*path` — passthrough to api.anthropic.com.
pub async fn anthropic<S: ProxyContext>(
    State(state): State<S>,
    req: Request,
) -> Result<Response, StatusCode> {
    passthrough(state, req, Family::Anthropic, "/gateway/anthropic", &anthropic_upstream()).await
}

/// `/gateway/openai/*path` — passthrough to api.openai.com.
pub async fn openai<S: ProxyContext>(
    State(state): State<S>,
    req: Request,
) -> Result<Response, StatusCode> {
    passthrough(state, req, Family::Openai, "/gateway/openai", &openai_upstream()).await
}

/// `/gateway/fireworks/*path` — passthrough to Fireworks' OpenAI-compatible API.
///
/// A sibling route rather than a branch inside [`openai`]: the two differ in
/// upstream, in the worker env pair they are reached by, and in that this one
/// meters usage and sets the session-affinity header (per-account
/// [`FireworksSettings`]). Sharing the openai route would put a per-account
/// conditional on codex's hot path for nothing.
pub async fn fireworks<S: ProxyContext>(
    State(state): State<S>,
    req: Request,
) -> Result<Response, StatusCode> {
    passthrough(state, req, Family::Fireworks, "/gateway/fireworks", &fireworks_upstream()).await
}

/// `/gateway/google/*path` — passthrough to the Gemini API.
///
/// The worker's
/// session token arrives as `x-goog-api-key`, the only header the Gemini SDK
/// carries a key in, and leaves as the account's key under the same name.
pub async fn google<S: ProxyContext>(
    State(state): State<S>,
    req: Request,
) -> Result<Response, StatusCode> {
    passthrough(state, req, Family::Google, "/gateway/google", &google_upstream()).await
}
