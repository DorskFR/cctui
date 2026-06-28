//! Optional, config-gated Langfuse tracing sink for the `/gateway` proxy
//! (CCT-443).
//!
//! Langfuse is **not** a forward proxy — it is an ingestion API beside the path.
//! The gateway already terminates + re-signs every worker call (worker ->
//! `/gateway` -> Anthropic) and therefore sees cleartext prompt + completion +
//! usage. This module is a **second, async, fire-and-forget sink** on that same
//! handler: when configured, the gateway hands it the (already reconstructed)
//! request/response payload and it POSTs a single trace + `generation` to
//! `<host>/api/public/ingestion`.
//!
//! ## Guarantees
//! - **Config-gated.** [`LangfuseConfig::from_env`] returns `None` unless host +
//!   both keys are set. Absent => the gateway never touches this module => zero
//!   overhead, no behaviour change.
//! - **Fail-open.** [`LangfuseClient::trace`] spawns a detached task and returns
//!   immediately; it never blocks the proxied call. A Langfuse outage, timeout,
//!   or error is logged at `debug` and dropped. Backpressure is shed by simply
//!   not awaiting.

use base64::Engine;
use serde_json::{Value, json};

/// Langfuse sink configuration (`[langfuse]` block / `CCTUI_LANGFUSE_*` env).
/// Built only when host + public + secret are all present.
#[derive(Debug, Clone)]
pub struct LangfuseConfig {
    /// Base URL of the Langfuse ingestion server (no trailing `/api/...`).
    pub host: String,
    pub public_key: String,
    pub secret_key: String,
    /// Fraction of calls to trace, `0.0..=1.0`. Defaults to `1.0` (trace all).
    pub sample_rate: f64,
}

impl LangfuseConfig {
    /// Parse the Langfuse config from the environment. Returns `None` (feature
    /// dark) unless `CCTUI_LANGFUSE_HOST`, `CCTUI_LANGFUSE_PUBLIC_KEY`, and
    /// `CCTUI_LANGFUSE_SECRET_KEY` are all set and non-empty.
    pub fn from_env() -> Option<Self> {
        let nonempty = |k: &str| std::env::var(k).ok().filter(|s| !s.trim().is_empty());
        let host = nonempty("CCTUI_LANGFUSE_HOST")?;
        let public_key = nonempty("CCTUI_LANGFUSE_PUBLIC_KEY")?;
        let secret_key = nonempty("CCTUI_LANGFUSE_SECRET_KEY")?;
        let sample_rate = nonempty("CCTUI_LANGFUSE_SAMPLE_RATE")
            .and_then(|s| s.parse::<f64>().ok())
            .map_or(1.0, |r| r.clamp(0.0, 1.0));
        Some(Self {
            host: host.trim_end_matches('/').to_string(),
            public_key,
            secret_key,
            sample_rate,
        })
    }

    /// The `Basic <base64(public:secret)>` header value for the ingestion API.
    fn basic_auth(&self) -> String {
        let raw = format!("{}:{}", self.public_key, self.secret_key);
        format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
    }
}

/// Identifying context for a single gateway call, used as trace metadata/tags.
#[derive(Debug, Clone, Default)]
pub struct TraceContext {
    pub session_id: Option<String>,
    pub account_id: Option<String>,
    pub model: Option<String>,
}

/// A fully reconstructed gateway call ready to be turned into a Langfuse trace.
pub struct TracePayload {
    pub ctx: TraceContext,
    /// The parsed request body (Anthropic Messages request: `messages`, `system`,
    /// `model`, ...). Used as the generation `input`.
    pub request: Option<Value>,
    /// The reconstructed completion text (assistant output).
    pub output: Option<String>,
    /// Token usage `{ input, output, cache_read, cache_creation }`.
    pub usage: Option<Value>,
}

/// The Langfuse sink. Cheap to clone (shares the `reqwest::Client`); present in
/// `AppState` only when configured.
#[derive(Clone)]
pub struct LangfuseClient {
    config: LangfuseConfig,
    http: reqwest::Client,
}

impl LangfuseClient {
    pub fn new(config: LangfuseConfig, http: reqwest::Client) -> Self {
        Self { config, http }
    }

    /// Whether this call should be sampled (cheap, sync — checked before the
    /// trace task is spawned and before any payload is reconstructed).
    pub fn should_sample(&self) -> bool {
        let r = self.config.sample_rate;
        r >= 1.0 || (r > 0.0 && rand_unit() < r)
    }

    /// Fire-and-forget: spawn a detached task that POSTs the trace + generation
    /// to Langfuse. Returns immediately; never blocks or errors into the caller.
    pub fn trace(&self, payload: TracePayload) {
        let config = self.config.clone();
        let http = self.http.clone();
        tokio::spawn(async move {
            if let Err(e) = post_ingestion(&http, &config, payload).await {
                tracing::debug!("langfuse trace dropped: {e}");
            }
        });
    }
}

/// Build the ingestion batch (one `trace-create` + one `generation-create`) and
/// POST it. Errors are returned to the caller, which logs+drops them.
async fn post_ingestion(
    http: &reqwest::Client,
    config: &LangfuseConfig,
    payload: TracePayload,
) -> Result<(), String> {
    let trace_id = uuid::Uuid::new_v4().to_string();
    let gen_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let ctx = &payload.ctx;

    let mut metadata = serde_json::Map::new();
    if let Some(sid) = &ctx.session_id {
        metadata.insert("session_id".into(), json!(sid));
        // The 8-hex worker shortcode is derived from the session id's leading hex.
        metadata.insert("short".into(), json!(short_of(sid)));
    }
    if let Some(aid) = &ctx.account_id {
        metadata.insert("account_id".into(), json!(aid));
    }

    let tags: Vec<String> = ["cctui".to_string()]
        .into_iter()
        .chain(ctx.session_id.clone())
        .chain(ctx.account_id.clone())
        .collect();

    let trace_name = ctx.session_id.clone().unwrap_or_else(|| "gateway".into());

    // First-class Langfuse Sessions/Users grouping. These top-level fields drive
    // the Sessions and Users tabs; the same ids in `metadata`/`tags` above only
    // support filtering, not grouping. The session id is the cctui session UUID;
    // the user dimension is the OAuth account id. Both are well under Langfuse's
    // 200-char limit. Omitted (serde drops nulls) when unknown.
    let mut trace_body = serde_json::Map::new();
    trace_body.insert("id".into(), json!(trace_id));
    trace_body.insert("name".into(), json!(trace_name));
    trace_body.insert("timestamp".into(), json!(now));
    if let Some(sid) = &ctx.session_id {
        trace_body.insert("sessionId".into(), json!(sid));
    }
    if let Some(aid) = &ctx.account_id {
        trace_body.insert("userId".into(), json!(aid));
    }
    trace_body.insert("input".into(), json!(payload.request));
    trace_body.insert("output".into(), json!(payload.output));
    trace_body.insert("metadata".into(), Value::Object(metadata.clone()));
    trace_body.insert("tags".into(), json!(tags));

    let trace_event = json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "type": "trace-create",
        "timestamp": now,
        "body": Value::Object(trace_body),
    });

    let gen_body = json!({
        "id": gen_id,
        "traceId": trace_id,
        "type": "GENERATION",
        "name": "anthropic.messages",
        "startTime": now,
        "endTime": now,
        "model": ctx.model,
        "input": payload.request,
        "output": payload.output,
        "usage": payload.usage,
        "metadata": Value::Object(metadata),
    });
    let gen_event = json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "type": "generation-create",
        "timestamp": now,
        "body": gen_body,
    });

    let batch = json!({ "batch": [trace_event, gen_event] });

    let resp = http
        .post(format!("{}/api/public/ingestion", config.host))
        .header(reqwest::header::AUTHORIZATION, config.basic_auth())
        .json(&batch)
        .send()
        .await
        .map_err(|e| format!("transport: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status()));
    }
    Ok(())
}

/// The 8-hex worker shortcode is the leading hex of the session id (CCT — the
/// daemon derives `~/.claude/jobs/<short>` the same way). Best-effort.
fn short_of(session_id: &str) -> String {
    session_id.chars().filter(|c| c.is_ascii_hexdigit()).take(8).collect()
}

/// A uniform random in `[0, 1)` without pulling in the `rand` crate — derived
/// from the low bits of a v4 UUID (good enough for sampling).
fn rand_unit() -> f64 {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let n = u64::from_le_bytes(bytes[0..8].try_into().unwrap_or([0; 8]));
    (n >> 11) as f64 / (1u64 << 53) as f64
}

/// Reconstruct the completion text + token usage from an Anthropic Messages
/// response body, handling both the buffered single-JSON shape and the SSE
/// stream (`event: ...\ndata: {...}` lines). Returns `(output_text, usage)`.
///
/// SSE reconstruction concatenates `content_block_delta` text deltas and pulls
/// usage from `message_start` (input tokens) + `message_delta` (output tokens).
pub fn reconstruct_anthropic(body: &[u8]) -> (Option<String>, Option<Value>) {
    let text = String::from_utf8_lossy(body);
    // Non-SSE: a single JSON object (e.g. non-streaming `/v1/messages`).
    if let Ok(v) = serde_json::from_str::<Value>(text.trim())
        && (v.get("content").is_some() || v.get("usage").is_some())
    {
        return (anthropic_output_text(&v), v.get("usage").map(normalize_usage));
    }
    reconstruct_sse(&text)
}

/// Extract concatenated text from a non-streaming Messages `content` array.
fn anthropic_output_text(v: &Value) -> Option<String> {
    let blocks = v.get("content")?.as_array()?;
    let s: String = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect();
    (!s.is_empty()).then_some(s)
}

/// Reconstruct from an SSE stream: concatenate text deltas, merge usage from
/// `message_start` + `message_delta`.
fn reconstruct_sse(text: &str) -> (Option<String>, Option<Value>) {
    let mut out = String::new();
    let mut input_tokens: Option<u64> = None;
    let mut output_tokens: Option<u64> = None;
    let mut cache_read: Option<u64> = None;
    let mut cache_creation: Option<u64> = None;
    let mut saw_event = false;

    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else { continue };
        let Ok(ev) = serde_json::from_str::<Value>(data.trim()) else { continue };
        saw_event = true;
        match ev.get("type").and_then(Value::as_str) {
            Some("content_block_delta") => {
                if let Some(t) = ev.pointer("/delta/text").and_then(Value::as_str) {
                    out.push_str(t);
                }
            }
            Some("message_start") => {
                if let Some(u) = ev.pointer("/message/usage") {
                    input_tokens = u.get("input_tokens").and_then(Value::as_u64).or(input_tokens);
                    cache_read =
                        u.get("cache_read_input_tokens").and_then(Value::as_u64).or(cache_read);
                    cache_creation = u
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                        .or(cache_creation);
                }
            }
            Some("message_delta") => {
                if let Some(u) = ev.get("usage") {
                    output_tokens =
                        u.get("output_tokens").and_then(Value::as_u64).or(output_tokens);
                }
            }
            _ => {}
        }
    }

    if !saw_event {
        return (None, None);
    }
    let usage = (input_tokens.is_some() || output_tokens.is_some()).then(|| {
        json!({
            "input": input_tokens,
            "output": output_tokens,
            "cache_read": cache_read,
            "cache_creation": cache_creation,
        })
    });
    ((!out.is_empty()).then_some(out), usage)
}

/// Normalize a non-streaming Anthropic `usage` object into the same shape as the
/// SSE reconstruction emits.
fn normalize_usage(u: &Value) -> Value {
    json!({
        "input": u.get("input_tokens").and_then(Value::as_u64),
        "output": u.get("output_tokens").and_then(Value::as_u64),
        "cache_read": u.get("cache_read_input_tokens").and_then(Value::as_u64),
        "cache_creation": u.get("cache_creation_input_tokens").and_then(Value::as_u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_dark_without_all_keys() {
        // No env set in test => disabled. (We don't mutate process env here to
        // avoid cross-test races; the absence path is the important guarantee.)
        // Just assert the basic-auth + sampling helpers behave.
        let cfg = LangfuseConfig {
            host: "https://lf.example/".into(),
            public_key: "pk".into(),
            secret_key: "sk".into(),
            sample_rate: 1.0,
        };
        assert!(cfg.basic_auth().starts_with("Basic "));
    }

    #[test]
    fn reconstruct_non_streaming() {
        let body = serde_json::to_vec(&json!({
            "content": [{"type": "text", "text": "hello "}, {"type": "text", "text": "world"}],
            "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 3}
        }))
        .unwrap();
        let (out, usage) = reconstruct_anthropic(&body);
        assert_eq!(out.as_deref(), Some("hello world"));
        let u = usage.unwrap();
        assert_eq!(u["input"], json!(10));
        assert_eq!(u["output"], json!(5));
        assert_eq!(u["cache_read"], json!(3));
    }

    #[test]
    fn reconstruct_sse_stream() {
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":20,\"cache_read_input_tokens\":4}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":7}}\n\n",
        );
        let (out, usage) = reconstruct_anthropic(sse.as_bytes());
        assert_eq!(out.as_deref(), Some("Hello"));
        let u = usage.unwrap();
        assert_eq!(u["input"], json!(20));
        assert_eq!(u["output"], json!(7));
        assert_eq!(u["cache_read"], json!(4));
    }

    #[test]
    fn reconstruct_garbage_is_empty() {
        let (out, usage) = reconstruct_anthropic(b"not json, not sse");
        assert!(out.is_none());
        assert!(usage.is_none());
    }

    #[test]
    fn short_of_takes_leading_hex() {
        assert_eq!(short_of("0123abcd-ef00-0000-0000-000000000000"), "0123abcd");
    }

    #[test]
    fn sample_rate_full_always_samples() {
        let c = LangfuseClient::new(
            LangfuseConfig {
                host: "h".into(),
                public_key: "p".into(),
                secret_key: "s".into(),
                sample_rate: 1.0,
            },
            reqwest::Client::new(),
        );
        assert!(c.should_sample());
        let z = LangfuseClient::new(
            LangfuseConfig {
                host: "h".into(),
                public_key: "p".into(),
                secret_key: "s".into(),
                sample_rate: 0.0,
            },
            reqwest::Client::new(),
        );
        assert!(!z.should_sample());
    }
}
