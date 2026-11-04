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
    fn captures_content(&self) -> bool {
        self.client.captures_content()
    }
    fn response_digest(&self, family: Family) -> Option<Box<dyn ResponseDigest>> {
        (!self.client.captures_content()).then(|| {
            Box::new(crate::langfuse::TraceDigest::new(response_format(family)))
                as Box<dyn ResponseDigest>
        })
    }
    fn report(
        &self,
        capture: TraceCapture,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let payload = trace_payload(capture);
            let (level, status_message) = crate::cache_bust::trace_annotation(
                &self.pool,
                payload.ctx.session_id.as_deref(),
                payload.ctx.model.as_deref(),
                payload.usage.as_ref(),
            )
            .await;
            self.client.trace(crate::langfuse::TracePayload { level, status_message, ..payload });
        })
    }
}

const fn response_format(family: Family) -> crate::langfuse::ResponseFormat {
    match family {
        Family::Anthropic => crate::langfuse::ResponseFormat::Anthropic,
        Family::Google => crate::langfuse::ResponseFormat::Gemini,
        Family::Openai | Family::Fireworks => crate::langfuse::ResponseFormat::Openai,
    }
}

/// The trace a capture becomes, before its cache annotation.
fn trace_payload(capture: TraceCapture) -> crate::langfuse::TracePayload {
    let output_redacted = capture.response_summary.is_some();
    let (output, usage) = match capture.response_summary {
        Some(summary) => {
            (summary.output_chars.map(crate::langfuse::redacted_output), summary.usage)
        }
        None => crate::langfuse::reconstruct(response_format(capture.family), &capture.response),
    };
    crate::langfuse::TracePayload {
        ctx: crate::langfuse::TraceContext {
            session_id: capture.session_id,
            account_id: Some(capture.account_id),
            model: capture.model,
        },
        request: capture.request,
        output,
        usage,
        level: None,
        status_message: None,
        request_redacted: capture.request_redacted,
        output_redacted,
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

#[cfg(test)]
mod tests {
    use super::{
        Family, ResponseDigest, TraceCapture, request_shape, response_format, trace_payload,
    };
    use crate::langfuse::{BatchIds, TraceDigest, ingestion_batch};
    use serde_json::{Value, json};

    const SESSION: &str = "0123abcd-ef00-0000-0000-000000000000";

    const ANTHROPIC_REQUEST: &[u8] = br#"{"model":"claude-opus-5","max_tokens":32000,"stream":true,"temperature":1,"thinking":{"type":"enabled","budget_tokens":31999},"metadata":{"user_id":"u"},"system":[{"type":"text","text":"SECRET system","cache_control":{"type":"ephemeral"}}],"messages":[{"role":"user","content":[{"type":"text","text":"key SECRET-sk é\n"}]},{"role":"assistant","content":"ok"},{"role":"user","content":"go"}],"tools":[{"name":"Bash","input_schema":{"type":"object","properties":{"command":{"type":"string"}}}},{"name":"Read","input_schema":{}}]}"#;

    const ANTHROPIC_SSE: &str = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":100,\"cache_read_input_tokens\":5000,\"output_tokens\":1}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: ping\n",
        "data: {\"type\": \"ping\"}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"こんにちは SECRET-answer \"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"done 🎉\"}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":42}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}",
    );

    const ANTHROPIC_JSON: &str = "\n  {\"id\":\"msg\",\"content\":[{\"type\":\"text\",\"text\":\"SECRET answer\"}],\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}\n";

    const ANTHROPIC_ERROR: &str =
        r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;

    const OPENAI_REQUEST: &[u8] = br#"{"model":"gpt-5.5","stream":true,"service_tier":"priority","instructions":"SECRET","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"SECRET q"}]}],"tools":[{"type":"function","name":"shell","parameters":{"type":"object","properties":{"z":{},"a":{}}}}]}"#;

    const OPENAI_SSE: &str = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r\"}}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"SECRET \"}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"réponse\"}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":100,\"input_tokens_details\":{\"cached_tokens\":40},\"output_tokens\":12,\"total_tokens\":112}}}\n\n",
        "data: [DONE]\n\n",
    );

    const GEMINI_REQUEST: &[u8] = br#"{"contents":[{"role":"user","parts":[{"text":"SECRET q"}]}], "systemInstruction":{"parts":[{"text":"SECRET s"}]},"generationConfig":{"topP":1,"maxOutputTokens":8}}"#;

    const GEMINI_SSE: &str = concat!(
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"plan\",\"thought\":true}],\"role\":\"model\"}}],\"usageMetadata\":{\"promptTokenCount\":100,\"thoughtsTokenCount\":3}}\r\n\r\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"SECRET \"}],\"role\":\"model\"}}],\"usageMetadata\":{\"promptTokenCount\":100,\"candidatesTokenCount\":1,\"thoughtsTokenCount\":3}}\r\n\r\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"lo\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":100,\"cachedContentTokenCount\":40,\"candidatesTokenCount\":2,\"thoughtsTokenCount\":3}}\r\n\r\n",
    );

    const FIREWORKS_REQUEST: &[u8] = b"{\"model\":\"accounts/fireworks/models/kimi-k3\",\n \"messages\":[{\"role\":\"system\",\"content\":\"SECRET\"},{\"role\":\"user\",\"content\":\"q\"}],\"tools\":[],\"stream\":true}";

    const FIREWORKS_SSE: &str = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"SECRET\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":2}}\n\n",
        "data: [DONE]\n\n",
    );

    type Fixture = (&'static str, Family, Option<&'static str>, &'static [u8], &'static str);

    fn fixtures() -> [Fixture; 8] {
        let anthropic =
            |name, response| (name, Family::Anthropic, None, ANTHROPIC_REQUEST, response);
        [
            anthropic("anthropic sse", ANTHROPIC_SSE),
            anthropic("anthropic json", ANTHROPIC_JSON),
            anthropic("anthropic error", ANTHROPIC_ERROR),
            anthropic("anthropic empty", ""),
            ("not json", Family::Anthropic, None, b"not json SECRET", ANTHROPIC_SSE),
            ("openai sse", Family::Openai, None, OPENAI_REQUEST, OPENAI_SSE),
            ("gemini sse", Family::Google, Some("gemini-2.5-pro"), GEMINI_REQUEST, GEMINI_SSE),
            ("fireworks sse", Family::Fireworks, None, FIREWORKS_REQUEST, FIREWORKS_SSE),
        ]
    }

    fn ids() -> BatchIds {
        BatchIds {
            trace: "trace".into(),
            generation: "generation".into(),
            trace_event: "trace-event".into(),
            generation_event: "generation-event".into(),
            now: "2026-11-05T00:00:00+00:00".into(),
        }
    }

    fn capture(family: Family, model: Option<String>, request: Option<Value>) -> TraceCapture {
        TraceCapture {
            family,
            session_id: Some(SESSION.into()),
            account_id: "account".into(),
            model,
            request,
            request_redacted: false,
            response: Vec::new(),
            response_summary: None,
        }
    }

    /// What the gateway posted when it buffered and parsed both bodies.
    fn buffered_trace(
        family: Family,
        path_model: Option<&str>,
        request: &[u8],
        response: &[u8],
    ) -> String {
        let parsed = serde_json::from_slice::<Value>(request).ok();
        let model = parsed
            .as_ref()
            .and_then(|r| r.get("model"))
            .and_then(Value::as_str)
            .or(path_model)
            .map(str::to_owned);
        let capture =
            TraceCapture { response: response.to_vec(), ..capture(family, model, parsed) };
        ingestion_batch(trace_payload(capture), false, &ids()).to_string()
    }

    /// What it posts when the request streams and the response is digested.
    fn streamed_trace(
        family: Family,
        path_model: Option<&str>,
        request: &[u8],
        response: &[u8],
        chunk: usize,
    ) -> String {
        let shape = request_shape(request);
        let model =
            shape.as_ref().and_then(|s| s.model.clone()).or_else(|| path_model.map(str::to_owned));
        let mut digest: Box<dyn ResponseDigest> =
            Box::new(TraceDigest::new(response_format(family)));
        for piece in response.chunks(chunk) {
            digest.feed(piece);
        }
        let capture = TraceCapture {
            request_redacted: true,
            response_summary: Some(digest.finish()),
            ..capture(family, model, shape.map(|s| s.summary))
        };
        ingestion_batch(trace_payload(capture), false, &ids()).to_string()
    }

    #[test]
    fn a_streamed_content_free_trace_is_byte_identical_to_the_buffered_one() {
        for (name, family, path_model, request, response) in fixtures() {
            let response = response.as_bytes();
            let golden = buffered_trace(family, path_model, request, response);
            assert!(!golden.contains("SECRET"), "{name} leaks content: {golden}");
            for chunk in [1, 2, 3, 5, 7, 64, response.len().max(1)] {
                assert_eq!(
                    streamed_trace(family, path_model, request, response, chunk),
                    golden,
                    "{name} in {chunk}-byte chunks"
                );
            }
        }
    }

    #[test]
    fn the_golden_anthropic_trace_keeps_shape_usage_and_lengths_only() {
        let golden =
            buffered_trace(Family::Anthropic, None, ANTHROPIC_REQUEST, ANTHROPIC_SSE.as_bytes());
        let batch: Value = serde_json::from_str(&golden).unwrap();
        let generation = &batch["batch"][1]["body"];
        assert_eq!(generation["model"], json!("claude-opus-5"));
        assert_eq!(generation["output"], json!("[redacted: 26 chars]"));
        assert_eq!(
            generation["input"],
            json!({
                "model": "claude-opus-5",
                "stream": true,
                "max_tokens": 32000,
                "temperature": 1,
                "thinking": {"type": "enabled", "budget_tokens": 31999},
                "messages_count": 3,
                "system_count": 1,
                "tools_count": 2,
                "redacted": true,
            })
        );
        assert_eq!(
            generation["usageDetails"],
            json!({
                "input": 12,
                "output": 42,
                "cache_read_input_tokens": 5000,
                "cache_creation_input_tokens": 100,
            })
        );
        assert_eq!(batch["batch"][0]["body"]["input"], generation["input"]);
    }
}
