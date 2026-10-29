//! Full-host provider endpoints and optional workspace observation.
use super::{Family, anthropic_upstream, fireworks_upstream, google_upstream, openai_upstream};
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
pub use cctui_server_core::gateway::proxy::*;
use std::sync::Arc;

struct FullTraceSink {
    client: Arc<crate::langfuse::LangfuseClient>,
    pool: sqlx::PgPool,
}
impl TraceSink for FullTraceSink {
    fn should_sample(&self) -> bool {
        self.client.should_sample()
    }
    fn report(
        &self,
        capture: TraceCapture,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let (output, usage) = match capture.family {
                Family::Anthropic => crate::langfuse::reconstruct_anthropic(&capture.response),
                Family::Google => crate::langfuse::reconstruct_gemini(&capture.response),
                Family::Openai | Family::Fireworks => {
                    crate::langfuse::reconstruct_openai(&capture.response)
                }
            };
            let ctx = crate::langfuse::TraceContext {
                session_id: capture.session_id,
                account_id: Some(capture.account_id),
                model: capture.model,
            };
            let (level, status_message) = crate::cache_bust::trace_annotation(
                &self.pool,
                ctx.session_id.as_deref(),
                ctx.model.as_deref(),
                usage.as_ref(),
            )
            .await;
            self.client.trace(crate::langfuse::TracePayload {
                ctx,
                request: capture.request,
                output,
                usage,
                level,
                status_message,
            });
        })
    }
}
struct FullVoiceTee {
    bus: crate::bus::Bus,
    voice: super::voicetee::VoiceTee,
}
impl ResponseTee for FullVoiceTee {
    fn apply(self: Box<Self>, stream: ResponseStream, readable_sse: bool) -> ResponseStream {
        super::voicetee::maybe_tee(stream, &self.bus, Some(self.voice), readable_sse)
    }
}
impl ProxyContext for AppState {
    fn rate_windows(&self) -> &Arc<dashmap::DashMap<uuid::Uuid, super::RateWindow>> {
        &self.gateway_rate_windows
    }
    fn upstream_url_permitted(&self, url: &str) -> Result<(), crate::outbound::OutboundUrlError> {
        crate::outbound::upstream_url_permitted(url)
    }
    fn upstream_client(&self) -> &reqwest::Client {
        crate::outbound::upstream_client()
    }
    fn trace_sink(&self) -> Option<Arc<dyn TraceSink>> {
        self.langfuse.as_ref().map(|client| {
            Arc::new(FullTraceSink { client: client.clone(), pool: self.pool.clone() })
                as Arc<dyn TraceSink>
        })
    }
    fn any_voice_watch(&self) -> bool {
        self.bus.any_voice_watch()
    }
    fn watched_session(&self, session: Option<String>) -> Option<String> {
        super::voicetee::watched_session(&self.bus, session)
    }
    fn voice_from_request(
        &self,
        session: String,
        body: &serde_json::Value,
    ) -> Option<Box<dyn ResponseTee>> {
        super::voicetee::VoiceTee::from_request(session, body).map(|voice| {
            Box::new(FullVoiceTee { bus: self.bus.clone(), voice }) as Box<dyn ResponseTee>
        })
    }
    async fn deliver_usage_notice(&self, account: &super::Account, token: &str) {
        super::usage_notices::deliver_if_due(self, account, token).await;
    }
}

/// `/gateway/anthropic/*path` — passthrough to api.anthropic.com.
pub async fn anthropic(
    State(state): State<AppState>,
    req: Request,
) -> Result<Response, StatusCode> {
    passthrough(state, req, Family::Anthropic, "/gateway/anthropic", &anthropic_upstream()).await
}

/// `/gateway/openai/*path` — passthrough to api.openai.com.
pub async fn openai(State(state): State<AppState>, req: Request) -> Result<Response, StatusCode> {
    passthrough(state, req, Family::Openai, "/gateway/openai", &openai_upstream()).await
}

/// `/gateway/fireworks/*path` — passthrough to Fireworks' OpenAI-compatible API.
///
/// A sibling route rather than a branch inside [`openai`]: the two differ in
/// upstream, in the worker env pair they are reached by, and in that this one
/// meters usage and sets the session-affinity header (per-account
/// [`FireworksSettings`]). Sharing the openai route would put a per-account
/// conditional on codex's hot path for nothing.
pub async fn fireworks(
    State(state): State<AppState>,
    req: Request,
) -> Result<Response, StatusCode> {
    passthrough(state, req, Family::Fireworks, "/gateway/fireworks", &fireworks_upstream()).await
}

/// `/gateway/google/*path` — passthrough to the Gemini API. The worker's
/// session token arrives as `x-goog-api-key`, the only header the Gemini SDK
/// carries a key in, and leaves as the account's key under the same name.
pub async fn google(State(state): State<AppState>, req: Request) -> Result<Response, StatusCode> {
    passthrough(state, req, Family::Google, "/gateway/google", &google_upstream()).await
}
