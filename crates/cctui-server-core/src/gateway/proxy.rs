use super::config::{
    Family, FireworksSettings, GatewayPathError, SESSION_AFFINITY_HEADER, gateway_path_permitted,
    google_path_model, tees_response,
};
use super::credentials::{Account, current_access_token, resolve_account};
use super::failover::{failover_retry_response, pick_failover_target, rebind_session};
use super::limits::{
    LimitContext, clear_account_reauth, clear_soft_limit_block, clear_soft_limit_block_for_token,
    durable_block_key, flag_account_reauth, mark_soft_limit_block, note_orphan_401,
    note_token_used, orphan_is_blocked, session_and_account_name_for_token, session_budget_limits,
};
use super::mint::session_id_for_token;
use super::toolguard::{GuardContext, guard_for, guard_stream, guardable};
use super::usage::{
    record_metered_usage, session_spend_usd_cached, usage_for_soft_limit_nonblocking,
};

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;

use super::failover::FailoverContext;
use super::ratelimit::{RateLimits, RateWindow};
use super::usage::UsageContext;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Raw response stream shared with optional workspace observers.
pub type ResponseStream = BoxStream<'static, Result<Bytes, reqwest::Error>>;

/// Optional response observer, installed by a host after tool-policy enforcement.
pub trait ResponseTee: Send + Sync {
    fn apply(self: Box<Self>, stream: ResponseStream, readable_sse: bool) -> ResponseStream;
}

/// Captured request/response data delivered after execution metering completes.
pub struct TraceCapture {
    pub family: Family,
    pub session_id: Option<String>,
    pub account_id: String,
    pub model: Option<String>,
    pub request: Option<serde_json::Value>,
    /// `request` is a [`RequestShape::summary`], not the body.
    pub request_redacted: bool,
    /// Empty when `response_summary` is set.
    pub response: Vec<u8>,
    pub response_summary: Option<ResponseSummary>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResponseSummary {
    pub output_chars: Option<usize>,
    pub usage: Option<serde_json::Value>,
}

pub trait ResponseDigest: Send {
    fn feed(&mut self, chunk: &[u8]);
    fn finish(self: Box<Self>) -> ResponseSummary;
}

/// Optional tracing sink. Sampling is decided once per request.
pub trait TraceSink: Send + Sync {
    fn should_sample(&self) -> bool;
    fn report(&self, capture: TraceCapture) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
    /// `false`: the sink only wants [`RequestShape`] and a [`ResponseDigest`],
    /// so the request streams upstream unbuffered.
    fn captures_content(&self) -> bool {
        true
    }
    fn response_digest(&self, _family: Family) -> Option<Box<dyn ResponseDigest>> {
        None
    }
}

pub const TRACE_KEPT_FIELDS: [&str; 6] =
    ["model", "stream", "max_tokens", "temperature", "service_tier", "thinking"];
pub const TRACE_COUNTED_FIELDS: [&str; 3] = ["messages", "system", "tools"];

#[derive(Clone, Debug, PartialEq)]
pub struct RequestShape {
    pub summary: serde_json::Value,
    pub model: Option<String>,
}

/// `None` exactly when the body is not JSON a `serde_json::Value` parse accepts.
#[must_use]
pub fn request_shape(body: &[u8]) -> Option<RequestShape> {
    serde_json::from_slice::<shape::Shape>(body).ok().map(shape::Shape::into_request_shape)
}

mod shape {
    use super::{RequestShape, TRACE_COUNTED_FIELDS, TRACE_KEPT_FIELDS};
    use serde::de::{Deserialize, Deserializer, Error, MapAccess, SeqAccess, Visitor};
    use serde_json::Value;

    /// Visits through `deserialize_any` rather than `IgnoredAny`, so strings,
    /// surrogates and nesting depth are validated as a `Value` parse does.
    struct Skip;

    impl<'de> Deserialize<'de> for Skip {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            d.deserialize_any(Scalar(Skip))
        }
    }

    struct Scalar<T>(T);

    impl<'de, T> Visitor<'de> for Scalar<T> {
        type Value = T;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a JSON value")
        }
        fn visit_bool<E: Error>(self, _: bool) -> Result<T, E> {
            Ok(self.0)
        }
        fn visit_i64<E: Error>(self, _: i64) -> Result<T, E> {
            Ok(self.0)
        }
        fn visit_u64<E: Error>(self, _: u64) -> Result<T, E> {
            Ok(self.0)
        }
        fn visit_f64<E: Error>(self, _: f64) -> Result<T, E> {
            Ok(self.0)
        }
        fn visit_str<E: Error>(self, _: &str) -> Result<T, E> {
            Ok(self.0)
        }
        fn visit_unit<E: Error>(self) -> Result<T, E> {
            Ok(self.0)
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<T, A::Error> {
            while seq.next_element::<Skip>()?.is_some() {}
            Ok(self.0)
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<T, A::Error> {
            while map.next_entry::<Skip, Skip>()?.is_some() {}
            Ok(self.0)
        }
    }

    /// An array's length; any other value counts as one.
    struct Count(usize);

    impl<'de> Deserialize<'de> for Count {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            d.deserialize_any(CountVisitor)
        }
    }

    struct CountVisitor;

    impl<'de> Visitor<'de> for CountVisitor {
        type Value = Count;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a JSON value")
        }
        fn visit_bool<E: Error>(self, _: bool) -> Result<Count, E> {
            Ok(Count(1))
        }
        fn visit_i64<E: Error>(self, _: i64) -> Result<Count, E> {
            Ok(Count(1))
        }
        fn visit_u64<E: Error>(self, _: u64) -> Result<Count, E> {
            Ok(Count(1))
        }
        fn visit_f64<E: Error>(self, _: f64) -> Result<Count, E> {
            Ok(Count(1))
        }
        fn visit_str<E: Error>(self, _: &str) -> Result<Count, E> {
            Ok(Count(1))
        }
        fn visit_unit<E: Error>(self) -> Result<Count, E> {
            Ok(Count(1))
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Count, A::Error> {
            let mut n = 0;
            while seq.next_element::<Skip>()?.is_some() {
                n += 1;
            }
            Ok(Count(n))
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Count, A::Error> {
            Scalar(Count(1)).visit_map(map)
        }
    }

    enum Field {
        Kept(usize),
        Counted(usize),
        Other,
    }

    impl<'de> Deserialize<'de> for Field {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            d.deserialize_str(FieldVisitor)
        }
    }

    struct FieldVisitor;

    impl Visitor<'_> for FieldVisitor {
        type Value = Field;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an object key")
        }
        fn visit_str<E: Error>(self, key: &str) -> Result<Field, E> {
            let kept = TRACE_KEPT_FIELDS.iter().position(|k| *k == key).map(Field::Kept);
            let counted =
                || TRACE_COUNTED_FIELDS.iter().position(|k| *k == key).map(Field::Counted);
            Ok(kept.or_else(counted).unwrap_or(Field::Other))
        }
    }

    #[derive(Default)]
    pub(super) struct Shape {
        kept: [Option<Value>; TRACE_KEPT_FIELDS.len()],
        counts: [Option<usize>; TRACE_COUNTED_FIELDS.len()],
    }

    impl Shape {
        pub(super) fn into_request_shape(self) -> RequestShape {
            let model = self.kept[0].as_ref().and_then(Value::as_str).map(str::to_owned);
            let mut out = serde_json::Map::new();
            for (key, value) in TRACE_KEPT_FIELDS.iter().zip(self.kept) {
                if let Some(value) = value {
                    out.insert((*key).to_owned(), value);
                }
            }
            for (key, n) in TRACE_COUNTED_FIELDS.iter().zip(self.counts) {
                if let Some(n) = n {
                    out.insert(format!("{key}_count"), Value::from(n));
                }
            }
            out.insert("redacted".into(), Value::Bool(true));
            RequestShape { summary: Value::Object(out), model }
        }
    }

    impl<'de> Deserialize<'de> for Shape {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            d.deserialize_any(ShapeVisitor)
        }
    }

    struct ShapeVisitor;

    impl<'de> Visitor<'de> for ShapeVisitor {
        type Value = Shape;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a JSON value")
        }
        fn visit_bool<E: Error>(self, _: bool) -> Result<Shape, E> {
            Ok(Shape::default())
        }
        fn visit_i64<E: Error>(self, _: i64) -> Result<Shape, E> {
            Ok(Shape::default())
        }
        fn visit_u64<E: Error>(self, _: u64) -> Result<Shape, E> {
            Ok(Shape::default())
        }
        fn visit_f64<E: Error>(self, _: f64) -> Result<Shape, E> {
            Ok(Shape::default())
        }
        fn visit_str<E: Error>(self, _: &str) -> Result<Shape, E> {
            Ok(Shape::default())
        }
        fn visit_unit<E: Error>(self) -> Result<Shape, E> {
            Ok(Shape::default())
        }
        fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Shape, A::Error> {
            Scalar(Shape::default()).visit_seq(seq)
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Shape, A::Error> {
            let mut shape = Shape::default();
            while let Some(field) = map.next_key::<Field>()? {
                match field {
                    Field::Kept(i) => shape.kept[i] = Some(map.next_value::<Value>()?),
                    Field::Counted(i) => shape.counts[i] = Some(map.next_value::<Count>()?.0),
                    Field::Other => {
                        map.next_value::<Skip>()?;
                    }
                }
            }
            Ok(shape)
        }
    }
}

/// Forwards the request body chunk for chunk, holding refcounted handles (no
/// copy) only until the body ends, when its [`RequestShape`] is read.
struct ShapeTee {
    inner: BoxStream<'static, Result<Bytes, axum::Error>>,
    held: Vec<Bytes>,
    held_len: usize,
    tracking: bool,
    shape: Arc<std::sync::OnceLock<RequestShape>>,
}

impl ShapeTee {
    fn new(
        inner: BoxStream<'static, Result<Bytes, axum::Error>>,
    ) -> (Self, Arc<std::sync::OnceLock<RequestShape>>) {
        let shape = Arc::new(std::sync::OnceLock::new());
        let tee =
            Self { inner, held: Vec::new(), held_len: 0, tracking: true, shape: shape.clone() };
        (tee, shape)
    }

    fn hold(&mut self, chunk: &Bytes) {
        if !self.tracking {
            return;
        }
        self.held_len += chunk.len();
        if self.held_len > MAX_BUFFERED_BODY {
            self.abandon();
        } else {
            self.held.push(chunk.clone());
        }
    }

    fn abandon(&mut self) {
        self.tracking = false;
        self.held = Vec::new();
    }

    fn finish(&mut self) {
        if !self.tracking {
            return;
        }
        let shape = match self.held.as_slice() {
            [one] => request_shape(one),
            held => request_shape(&held.concat()),
        };
        if let Some(shape) = shape {
            let _ = self.shape.set(shape);
        }
        self.abandon();
    }
}

impl futures_util::Stream for ShapeTee {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let next = std::task::ready!(self.inner.poll_next_unpin(cx));
        match &next {
            Some(Ok(chunk)) => self.hold(chunk),
            Some(Err(_)) => self.abandon(),
            None => self.finish(),
        }
        std::task::Poll::Ready(next)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestTrace {
    Off,
    Shape,
    Full,
}

impl RequestTrace {
    fn of(langfuse: Option<&Arc<dyn TraceSink>>) -> Self {
        langfuse.map_or(Self::Off, |sink| {
            if sink.captures_content() { Self::Full } else { Self::Shape }
        })
    }
}

const fn must_buffer(trace: RequestTrace, metered: bool, model_gate: bool, voice: bool) -> bool {
    matches!(trace, RequestTrace::Full) || metered || model_gate || voice
}

/// Mandatory execution resources and explicit optional workspace adapters.
pub trait ProxyContext:
    LimitContext + FailoverContext + UsageContext + Clone + Send + 'static
{
    fn proxy_pool(&self) -> &sqlx::PgPool {
        super::credentials::CredentialContext::pool(self)
    }
    fn rate_windows(&self) -> &Arc<dashmap::DashMap<uuid::Uuid, RateWindow>>;
    fn upstream_url_permitted(&self, url: &str) -> Result<(), crate::outbound::OutboundUrlError>;
    fn upstream_client(&self) -> &reqwest::Client;
    fn trace_sink(&self) -> Option<Arc<dyn TraceSink>>;
    fn any_voice_watch(&self) -> bool;
    fn watched_session(&self, session: Option<String>) -> Option<String>;
    fn voice_from_request(
        &self,
        session: String,
        body: &serde_json::Value,
    ) -> Option<Box<dyn ResponseTee>>;
    fn deliver_usage_notice(
        &self,
        account: &Account,
        token: &str,
    ) -> impl Future<Output = ()> + Send;
}

fn guard_context(state: &impl ProxyContext) -> GuardContext {
    GuardContext { pool: state.proxy_pool().clone(), bus: state.limit_bus().clone() }
}

fn admit_request(
    state: &impl ProxyContext,
    provider: uuid::Uuid,
    limits: &RateLimits,
) -> Result<(), i64> {
    let mut window = state.rate_windows().entry(provider).or_default();
    super::ratelimit::admit(&mut window, limits, std::time::Instant::now())
}

/// Which side of the gateway rejected an authenticated request.
///
/// The two are easy to confuse from a worker's point of view — both surface as a
/// 401 — but they need opposite remedies, so we label every gateway 401 with
/// one of these in both the body message and the `x-cctui-auth-stage` header.
#[derive(Clone, Copy)]
pub enum AuthStage {
    /// cctui itself rejected the inbound `cctui_s_…` session token: unknown,
    /// revoked, or not bound to an account. The LLM login is irrelevant here.
    SessionToken,
    /// cctui accepted the session token and mapped it to an account, but the
    /// upstream LLM provider rejected that account's OAuth credentials (expired
    /// / revoked refresh token, failed refresh, upstream 401). The cctui token
    /// is fine; the account needs re-authenticating.
    ProviderOauth,
}

/// Build a labeled 401 response.
///
/// The body uses the provider's native
/// error envelope so the CLI surfaces the message verbatim, and the
/// `x-cctui-auth-stage` header makes the cause machine-readable in logs/clients.
pub fn auth_error(stage: AuthStage, is_anthropic: bool) -> Response {
    let (stage_tag, message) = match stage {
        AuthStage::SessionToken => (
            "session-token",
            "cctui gateway rejected the session token: the cctui_s_ credential is \
             unknown, revoked, or not bound to an account. This is a cctui gateway \
             credential problem, NOT an LLM provider login problem — re-create or \
             re-resume the session to mint a fresh token.",
        ),
        AuthStage::ProviderOauth => (
            "provider-oauth",
            "cctui accepted the session token, but the upstream LLM provider returned \
             401 for the bound account's OAuth credentials. The cctui token is valid — \
             re-authenticate the LLM account in cctui.",
        ),
    };
    // Native error envelopes: Anthropic `{type:error, error:{type,message}}`;
    // OpenAI `{error:{message,type}}`. Both render the message in the CLI.
    let body = if is_anthropic {
        serde_json::json!({
            "type": "error",
            "error": { "type": "authentication_error", "message": message },
        })
    } else {
        serde_json::json!({
            "error": { "message": message, "type": "authentication_error" },
        })
    };
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("x-cctui-auth-stage", stage_tag)
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| StatusCode::UNAUTHORIZED.into_response())
}

/// 502 for an account whose `base_url` the outbound guard refuses, in the
/// family's native error envelope so the CLI shows the message.
pub fn upstream_refused(
    reason: &crate::outbound::OutboundUrlError,
    is_anthropic: bool,
) -> Response {
    let message = format!(
        "cctui gateway refused this account's base_url: it {reason}. An admin can \
         allow a trusted host in Settings (allowed upstream hosts) or with CCTUI_UPSTREAM_ALLOWED_HOSTS."
    );
    let body = if is_anthropic {
        serde_json::json!({ "type": "error", "error": { "type": "api_error", "message": message } })
    } else {
        serde_json::json!({ "error": { "message": message, "type": "upstream_refused" } })
    };
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

/// Largest request body the gateway will buffer (the observed/reshaped path).
/// An unbuffered passthrough streams and is not bounded by this.
const MAX_BUFFERED_BODY: usize = 32 * 1024 * 1024;

fn declared_body_len(headers: &HeaderMap) -> Option<usize> {
    headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<usize>().ok())
}

/// 403 for a path or method outside the provider's allowlist.
fn path_refused(reason: GatewayPathError, is_anthropic: bool) -> Response {
    let message = format!(
        "cctui gateway refused this request: the path {reason}. Only the provider's \
         inference endpoints are forwarded."
    );
    let body = if is_anthropic {
        serde_json::json!({ "type": "error", "error": { "type": "invalid_request_error", "message": message } })
    } else {
        serde_json::json!({ "error": { "message": message, "type": "invalid_request_error" } })
    };
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| StatusCode::FORBIDDEN.into_response())
}

/// Refuse a request the soft limit blocks: fail it over to a sibling account
/// with headroom if there is one, else flag the session and 429.
///
/// `model` is what the sibling must have room for; `None` (model not yet read
/// off the body) elects conservatively, counting every window.
#[allow(clippy::too_many_arguments)]
async fn soft_limit_refusal(
    state: &impl ProxyContext,
    session_token: &str,
    acct: &Account,
    is_anthropic: bool,
    model: Option<&str>,
    retry_after_secs: i64,
    reason: String,
    block_key: String,
) -> Result<Response, StatusCode> {
    // Before refusing with the account's own reset horizon, try to rebind the
    // session to a sibling with headroom — the worker's 429 retry then lands on
    // the new account instead of stalling.
    if let Some(target) = pick_failover_target(state, session_token, acct.id, model).await
        && rebind_session(state, &target, acct.id).await
    {
        return Ok(failover_retry_response(&target.account_name, &target.reason, is_anthropic));
    }
    // Surface the block as a per-session signal so the webui can offer
    // "continue on another account". Best-effort + dedup'd.
    if let Some((session_id, account_name)) =
        session_and_account_name_for_token(state, session_token).await
    {
        mark_soft_limit_block(
            state,
            &session_id,
            acct.id,
            &account_name,
            &reason,
            &block_key,
            retry_after_secs,
        )
        .await;
    }
    Response::builder()
        .status(StatusCode::TOO_MANY_REQUESTS)
        .header(http::header::RETRY_AFTER, retry_after_secs.to_string())
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::json!({ "error": reason }).to_string()))
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)
}

#[must_use]
pub fn skip_request_header(lower_name: &str) -> bool {
    // `x-openai-actor-authorization` is a dummy the codex provider config carries
    // purely to unlock the built-in image_gen tool; it must never reach upstream.
    matches!(
        lower_name,
        "authorization"
            | "x-goog-api-key"
            | "host"
            | "content-length"
            | "connection"
            | "x-openai-actor-authorization"
    )
}

/// The worker's session token: a bearer, or the Gemini SDK's `x-goog-api-key`.
pub fn session_token_from(headers: &HeaderMap) -> Option<String> {
    let bearer = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let goog = || headers.get("x-goog-api-key").and_then(|v| v.to_str().ok());
    bearer.or_else(goog).map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned)
}

#[must_use]
pub fn skip_response_header(lower_name: &str) -> bool {
    matches!(lower_name, "connection" | "transfer-encoding" | "content-length")
}

/// The body to forward upstream and the model it names, from a buffered body.
///
/// The forwarded body is always the client's bytes, never a re-serialization:
/// `serde_json` re-emits objects with sorted keys, which moves every token after
/// the first reordered object and loses the provider's prompt cache. The parsed
/// copy is only read.
fn buffered_payload(
    bytes: Bytes,
    path_model: Option<String>,
) -> (reqwest::Body, Option<serde_json::Value>, Option<String>) {
    let parsed = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
    let request_model = parsed
        .as_ref()
        .and_then(|r| r.get("model"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or(path_model);
    (reqwest::Body::from(bytes), parsed, request_model)
}

/// Whether requests on this account carry [`SESSION_AFFINITY_HEADER`].
fn wants_affinity(family: Family, provider_settings: Option<&serde_json::Value>) -> bool {
    family == Family::Fireworks && FireworksSettings::resolve(provider_settings).session_affinity
}

type Reply = Result<Response, StatusCode>;

/// A worker request whose bearer resolved to an account.
struct Authenticated {
    session_token: String,
    acct: Account,
}

/// What the soft-limit gate could not decide before reading the body: the
/// usage windows to re-evaluate once the request's model is known.
struct Admission {
    effective_limits: cctui_proto::api::soft_limit::SoftLimits,
    model_gate: Option<Vec<cctui_proto::api::soft_limit::UsageWindow>>,
}

/// Where the request goes and with which credentials.
struct UpstreamTarget {
    client: reqwest::Client,
    url: String,
    method: reqwest::Method,
    access_token: String,
}

/// Everything that wants to read the request or response.
struct Observers {
    /// Fireworks usage is metered from the responses, priced by request model.
    fireworks: bool,
    /// Usage is read from the Gemini responses this account's calls return.
    google: bool,
    affinity_session: Option<String>,
    langfuse: Option<Arc<dyn TraceSink>>,
    trace_session_id: Option<String>,
    tool_guard: Option<super::toolguard::ActiveGuard>,
    voice_session: Option<String>,
}

/// What was learned from the request body while preparing it.
struct RequestInfo {
    traced_request: Option<serde_json::Value>,
    request_redacted: bool,
    /// Set once a streamed body has gone upstream in full.
    request_shape: Option<Arc<std::sync::OnceLock<RequestShape>>>,
    request_model: Option<String>,
    voice: Option<Box<dyn ResponseTee>>,
}

impl RequestInfo {
    const fn streamed(request_model: Option<String>) -> Self {
        Self {
            traced_request: None,
            request_redacted: false,
            request_shape: None,
            request_model,
            voice: None,
        }
    }

    /// The body's model wins over the path's, as in [`buffered_payload`].
    fn model(&self) -> Option<String> {
        self.request_shape
            .as_ref()
            .and_then(|shape| shape.get())
            .and_then(|shape| shape.model.clone())
            .or_else(|| self.request_model.clone())
    }
}

/// Map the worker's bearer (the session token) to an account. A missing
/// bearer or one that doesn't resolve is a *cctui* rejection — distinguished
/// from a provider rejection so the worker/operator knows which to fix.
#[tracing::instrument(target = "otel", skip_all, fields(code.function.name = "authenticate"))]
async fn authenticate(
    state: &impl ProxyContext,
    headers: &HeaderMap,
    is_anthropic: bool,
) -> Result<Authenticated, Reply> {
    let Some(session_token) = session_token_from(headers) else {
        return Err(Ok(auth_error(AuthStage::SessionToken, is_anthropic)));
    };

    // Orphan-spam guard: an unbound worker retries `/gateway` forever; each retry
    // that reaches the DB starves the pool. Fingerprint the token and, if it is
    // already flagged as a spamming orphan, drop the request *before* the DB.
    let token_fp = crate::auth::sha256_hex(&session_token);
    if orphan_is_blocked(state, &token_fp) {
        return Err(Ok(auth_error(AuthStage::SessionToken, is_anthropic)));
    }

    match resolve_account(state, &session_token).await {
        Ok(Some(acct)) => {
            note_token_used(state, &token_fp);
            Ok(Authenticated { session_token, acct })
        }
        // Genuinely unknown/revoked/unbound token — a real orphan. Count it
        // toward the spam guard and reject as a cctui auth failure.
        Ok(None) => {
            note_orphan_401(state, &token_fp);
            Err(Ok(auth_error(AuthStage::SessionToken, is_anthropic)))
        }
        // The DB lookup itself failed (cold/starved pool during a server
        // restart, transient network). This is NOT an orphan — a valid bound
        // token can land here while the pool warms up. Returning a retryable
        // 503 (and crucially NOT feeding the orphan-spam block) keeps a server
        // restart from poisoning live tokens for 300s.
        Err(e) => {
            tracing::warn!(
                stage = "session-token",
                error = %e,
                "gateway token resolution failed transiently (DB) — returning 503, not orphaning"
            );
            Err(Err(StatusCode::SERVICE_UNAVAILABLE))
        }
    }
}

/// Soft limit: cap cctui's own share of the account's usage windows so it
/// leaves headroom for the human sharing the subscription. Only the
/// configured windows gate; bypass near reset.
///
/// The usage cache is warmed only by the accounts-page route, which headless
/// dispatch never opens, so on the dispatch path a cold cache is read from
/// upstream before evaluating and a stale one is served while it refreshes in
/// the background. Fetch errors fail open.
/// A `CctuiAgent` child carries its own `session_usd` cap, which the account's
/// stored limits know nothing about. Overlay it here; the map is empty on the
/// ordinary path, so this costs a lock-free length check per request.
///
/// Then the gateway rate limit: a pay-per-token provider's RPM/TPM tier is
/// shared by every session on the account, so throttle at the proxy. Requests
/// count on admission; tokens accrue when a response's usage lands. Unset ⇒ skip.
#[tracing::instrument(target = "otel", skip_all, fields(code.function.name = "admit"))]
async fn admit(
    state: &impl ProxyContext,
    auth: &Authenticated,
    is_anthropic: bool,
) -> Result<Admission, Reply> {
    let Authenticated { session_token, acct } = auth;
    let effective_limits = session_budget_limits(state, acct, session_token).await;
    let mut model_gate: Option<Vec<cctui_proto::api::soft_limit::UsageWindow>> = None;
    if !effective_limits.is_unset() {
        let cached = usage_for_soft_limit_nonblocking(state, acct.id).await;
        let mut windows = cached
            .as_ref()
            .map(cctui_proto::api::soft_limit::normalize_usage_windows)
            .unwrap_or_default();
        // The per-session budget is session-scoped, so it can't come from the
        // per-account usage cache — resolve it here, and only when one is set.
        if effective_limits.limits.contains_key(cctui_proto::api::soft_limit::KEY_SESSION_USD)
            && let Some(session_id) = session_id_for_token(state, session_token).await
            && let Some(spent) = session_spend_usd_cached(state, acct.id, &session_id).await
        {
            windows.push(cctui_proto::api::soft_limit::usd_window(
                cctui_proto::api::soft_limit::KEY_SESSION_USD,
                spent,
                None,
            ));
        }
        // A `weekly_model:` cap can only be judged once the request's model is
        // known, which needs the body. Gate the model-blind windows here so the
        // zero-copy passthrough survives for every account without such a cap,
        // and defer the scoped ones to `prepare_body`.
        let scoped_caps = effective_limits
            .limits
            .keys()
            .any(|k| cctui_proto::api::soft_limit::is_model_scoped_key(k));
        let unscoped: Vec<cctui_proto::api::soft_limit::UsageWindow> = windows
            .iter()
            .filter(|w| !cctui_proto::api::soft_limit::is_model_scoped_key(&w.key))
            .cloned()
            .collect();
        if let cctui_proto::api::soft_limit::Decision::Block { retry_after_secs, reason, key } =
            cctui_proto::api::soft_limit::evaluate_soft_limit(
                &unscoped,
                &effective_limits,
                None,
                Utc::now(),
            )
        {
            tracing::info!(account = %acct.id, retry_after_secs, "soft limit hit: {reason}");
            return Err(soft_limit_refusal(
                state,
                session_token,
                acct,
                is_anthropic,
                None,
                retry_after_secs,
                reason,
                durable_block_key(&acct.soft_limits, &effective_limits, &key),
            )
            .await);
        }
        if scoped_caps {
            model_gate = Some(windows);
        }
    }

    if !acct.rate_limits.is_unset()
        && let Err(retry_after_secs) = admit_request(state, acct.id, &acct.rate_limits)
    {
        tracing::info!(account = %acct.id, retry_after_secs, "gateway rate limit hit");
        let resp = Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header(http::header::RETRY_AFTER, retry_after_secs.to_string())
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({ "error": "gateway rate limit exceeded" }).to_string(),
            ))
            .map_err(|_| StatusCode::TOO_MANY_REQUESTS);
        return Err(resp);
    }

    Ok(Admission { effective_limits, model_gate })
}

/// Pick the upstream credentials, host and client for this account.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(target = "otel", skip_all, fields(code.function.name = "resolve_upstream"))]
async fn resolve_upstream(
    state: &impl ProxyContext,
    acct: &Account,
    uri: &axum::http::Uri,
    req_method: &axum::http::Method,
    family: Family,
    prefix: &str,
    upstream_base: &str,
    is_anthropic: bool,
) -> Result<UpstreamTarget, Reply> {
    // Decided before a credential is spent on it, and never normalized: a tail
    // that could re-resolve is refused, not rewritten.
    let path = uri.path();
    let tail = path.strip_prefix(prefix).unwrap_or(path);
    if let Err(e) = gateway_path_permitted(family, req_method.as_str(), tail) {
        tracing::warn!(account = %acct.id, %tail, method = %req_method, "gateway refused path: {e}");
        return Err(Ok(path_refused(e, is_anthropic)));
    }

    // The session token is valid (resolved already); a failure to obtain an
    // upstream access token here is a provider-credential problem (no/expired
    // refresh token, failed refresh) — label it as such.
    let Ok(access_token) = current_access_token(state, acct).await else {
        tracing::warn!(account = %acct.id, stage = "provider-oauth", "gateway 401: no upstream access token for account");
        flag_account_reauth(state, acct.id, "no upstream access token (refresh failed)");
        return Err(Ok(auth_error(AuthStage::ProviderOauth, is_anthropic)));
    };

    // Per-account upstream: a compatible endpoint overrides the
    // built-in upstream with its stored `base_url`; native subscription accounts
    // fall back to the built-in `api.anthropic.com`/`chatgpt.com`.
    let custom = acct.base_url.as_deref().filter(|u| !u.trim().is_empty());
    if let Some(base) = custom
        && let Err(e) = state.upstream_url_permitted(base)
    {
        tracing::warn!(
            account = %acct.id,
            base_url = %base,
            "gateway refused account base_url ({e}); allow the host in Settings \
             or CCTUI_UPSTREAM_ALLOWED_HOSTS if it is trusted"
        );
        return Err(Ok(upstream_refused(&e, is_anthropic)));
    }
    let upstream = custom.unwrap_or(upstream_base);
    let client = if custom.is_some() {
        state.upstream_client().clone()
    } else {
        state.http_client().clone()
    };

    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let url = format!("{}{tail}{query}", upstream.trim_end_matches('/'));

    let method = reqwest::Method::from_bytes(req_method.as_str().as_bytes())
        .map_err(|_| Err(StatusCode::BAD_REQUEST))?;

    Ok(UpstreamTarget { client, url, method, access_token })
}

/// Preserve every client header verbatim except hop-by-hop + the bearer we
/// are swapping and the Host (reqwest sets it from the upstream URL).
fn forward_headers(
    inbound: &HeaderMap,
    acct: &Account,
    access_token: &str,
) -> Result<HeaderMap, StatusCode> {
    let mut headers = HeaderMap::new();
    for (name, value) in inbound {
        let n = name.as_str().to_ascii_lowercase();
        if skip_request_header(&n) {
            continue;
        }
        if let (Ok(hn), Ok(hv)) = (
            reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            headers.insert(hn, hv);
        }
    }
    // Gemini keys travel as `x-goog-api-key`, Anthropic API keys as `x-api-key`;
    // every other scheme is a bearer.
    let family = Family::from_provider(&acct.provider);
    let (name, value) = if family == Family::Google {
        (reqwest::header::HeaderName::from_static("x-goog-api-key"), access_token.to_owned())
    } else if acct.auth_scheme == "api_key" {
        (reqwest::header::HeaderName::from_static("x-api-key"), access_token.to_owned())
    } else {
        (reqwest::header::AUTHORIZATION, format!("Bearer {access_token}"))
    };
    headers.insert(
        name,
        reqwest::header::HeaderValue::from_str(&value)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    // ChatGPT-backed Codex requests must carry the account id upstream. Other
    // families store an unrelated id in this column (Fireworks: the billing
    // account slug), so the header is scoped rather than sent on every request.
    if family == Family::Openai
        && let Some(account_id) = acct.provider_account_id.as_deref()
        && let Ok(hv) = reqwest::header::HeaderValue::from_str(account_id)
    {
        headers.insert("chatgpt-account-id", hv);
    }
    Ok(headers)
}

/// Resolve the request/response observers and adjust the outbound headers
/// for them.
#[tracing::instrument(target = "otel", skip_all, fields(code.function.name = "observers"))]
async fn observers(
    state: &impl ProxyContext,
    auth: &Authenticated,
    headers: &mut HeaderMap,
    voice_candidate: bool,
) -> Result<Observers, StatusCode> {
    let Authenticated { session_token, acct } = auth;
    let family = Family::from_provider(&acct.provider);
    let fireworks = family == Family::Fireworks;
    let google = family == Family::Google;
    let affinity_session = if wants_affinity(family, acct.provider_settings.as_ref()) {
        session_id_for_token(state, session_token).await
    } else {
        None
    };
    if let Some(sid) = affinity_session.as_deref()
        && let Ok(hv) = reqwest::header::HeaderValue::from_str(sid)
    {
        headers.insert(SESSION_AFFINITY_HEADER, hv);
    }

    let langfuse = state.trace_sink().filter(|lf| lf.should_sample());
    let trace_session_id =
        if langfuse.is_some() { session_id_for_token(state, session_token).await } else { None };

    let tool_guard = guard_for(&guard_context(state), acct.id, session_token).await.map_err(|e| {
        tracing::warn!(account = %acct.id, error = %e, "tool policy unavailable, refusing unguarded");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    let voice_session = if voice_candidate && family == Family::Anthropic && state.any_voice_watch()
    {
        state.watched_session(session_id_for_token(state, session_token).await)
    } else {
        None
    };
    if tees_response(langfuse.is_some(), fireworks || google)
        || tool_guard.is_some()
        || voice_session.is_some()
    {
        headers.remove(reqwest::header::ACCEPT_ENCODING);
    }
    Ok(Observers {
        fireworks,
        google,
        affinity_session,
        langfuse,
        trace_session_id,
        tool_guard,
        voice_session,
    })
}

/// Stream the request body through without buffering (default), OR buffer it
/// once when something needs to read it. Either way the client's bytes are what
/// goes upstream. `path_model` is the model a Gemini call names in its path
/// instead of its body.
#[tracing::instrument(target = "otel", skip_all, fields(code.function.name = "prepare_body"))]
async fn prepare_body(
    state: &impl ProxyContext,
    auth: &Authenticated,
    admission: Admission,
    obs: &Observers,
    req: Request,
    is_anthropic: bool,
    path_model: Option<String>,
) -> Result<(reqwest::Body, RequestInfo), Reply> {
    let Authenticated { session_token, acct } = auth;
    let Admission { effective_limits, model_gate } = admission;
    let trace = RequestTrace::of(obs.langfuse.as_ref());
    if !must_buffer(trace, obs.fireworks, model_gate.is_some(), obs.voice_session.is_some()) {
        let body_stream = req.into_body().into_data_stream();
        if trace == RequestTrace::Shape {
            let (tee, shape) = ShapeTee::new(body_stream.boxed());
            let info = RequestInfo {
                request_redacted: true,
                request_shape: Some(shape),
                ..RequestInfo::streamed(path_model)
            };
            return Ok((reqwest::Body::wrap_stream(tee), info));
        }
        return Ok((reqwest::Body::wrap_stream(body_stream), RequestInfo::streamed(path_model)));
    }
    let bytes = axum::body::to_bytes(req.into_body(), MAX_BUFFERED_BODY)
        .await
        .map_err(|_| Err(StatusCode::PAYLOAD_TOO_LARGE))?;
    let summary = (trace == RequestTrace::Shape)
        .then(|| request_shape(&bytes))
        .flatten()
        .map(|shape| shape.summary);
    let (payload, parsed, request_model) = buffered_payload(bytes, path_model);
    let voice = obs
        .voice_session
        .clone()
        .zip(parsed.as_ref())
        .and_then(|(sid, body)| state.voice_from_request(sid, body));
    // The deferred half of the soft limit: a `weekly_model:` cap gates only
    // the model it names, so a spent weekly Fable budget must let an Opus
    // request through. Same `window_applies` the account election uses, so
    // the two can never disagree.
    if let Some(windows) = model_gate
        && let cctui_proto::api::soft_limit::Decision::Block { retry_after_secs, reason, key } =
            cctui_proto::api::soft_limit::evaluate_soft_limit(
                &windows,
                &effective_limits,
                request_model.as_deref(),
                Utc::now(),
            )
    {
        tracing::info!(
            account = %acct.id,
            model = request_model.as_deref().unwrap_or("unknown"),
            retry_after_secs,
            "soft limit hit: {reason}"
        );
        return Err(soft_limit_refusal(
            state,
            session_token,
            acct,
            is_anthropic,
            request_model.as_deref(),
            retry_after_secs,
            reason,
            durable_block_key(&acct.soft_limits, &effective_limits, &key),
        )
        .await);
    }
    Ok((
        payload,
        RequestInfo {
            traced_request: match trace {
                RequestTrace::Full => parsed,
                RequestTrace::Shape => summary,
                RequestTrace::Off => None,
            },
            request_redacted: trace == RequestTrace::Shape,
            request_shape: None,
            request_model,
            voice,
        },
    ))
}

/// The upstream call, up to its response headers, as an exported client span:
/// the server span's remainder is the gateway's own work.
async fn send_traced(
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, reqwest::Error> {
    use tracing::Instrument;
    let (client, request) = request.build_split();
    let request = request?;
    let method = request.method().as_str().to_owned();
    let host = request.url().host_str().unwrap_or_default().to_owned();
    let name = format!("{method} {host}");
    let span = tracing::info_span!(
        target: "otel",
        "gateway.upstream",
        otel.name = name.as_str(),
        otel.kind = "client",
        otel.status_code = tracing::field::Empty,
        otel.status_description = tracing::field::Empty,
        code.function.name = "send_upstream",
        http.request.method = method.as_str(),
        server.address = host.as_str(),
        url.path = request.url().path(),
        http.response.status_code = tracing::field::Empty,
    );
    let result = client.execute(request).instrument(span.clone()).await;
    record_upstream_outcome(&span, &result);
    result
}

fn record_upstream_outcome(
    span: &tracing::Span,
    result: &Result<reqwest::Response, reqwest::Error>,
) {
    match result {
        Ok(response) => {
            let status = response.status();
            span.record("http.response.status_code", u64::from(status.as_u16()));
            if status.is_client_error() || status.is_server_error() {
                span.record("otel.status_code", "error");
            }
        }
        Err(e) => {
            span.record("otel.status_code", "error");
            span.record("otel.status_description", tracing::field::display(e));
        }
    }
}

/// Send the request upstream and settle everything that depends only on the
/// response status: stats, credential/rate-limit outcomes, block clearing.
async fn send_upstream(
    state: &impl ProxyContext,
    auth: &Authenticated,
    request: reqwest::RequestBuilder,
    info: &RequestInfo,
    obs: &Observers,
    is_anthropic: bool,
) -> Result<(reqwest::Response, StatusCode), Reply> {
    let Authenticated { session_token, acct } = auth;
    let upstream = send_traced(request).await.map_err(|e| {
        tracing::error!(account = %acct.id, "gateway upstream error: {e}");
        Err(StatusCode::BAD_GATEWAY)
    })?;

    // Opportunistic stats: request count + response byte count (no buffering).
    let resp_len = i64::try_from(upstream.content_length().unwrap_or(0)).unwrap_or(i64::MAX);
    let acct_id = acct.id;
    let pool = state.proxy_pool().clone();
    tokio::spawn(async move {
        let _ = sqlx::query(
            "UPDATE account_providers SET request_count = request_count + 1, \
                    bytes_transferred = bytes_transferred + $2, last_used_at = now() \
             WHERE id = $1",
        )
        .bind(acct_id)
        .bind(resp_len)
        .execute(&pool)
        .await;
    });

    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    // The session token was accepted by cctui (we got this far), so a 401 from
    // the upstream provider means the account's OAuth credentials are bad, not
    // the cctui token. Replace the opaque upstream 401 with a labeled one so the
    // worker/operator re-authenticates the account rather than the session.
    if status == StatusCode::UNAUTHORIZED {
        tracing::warn!(account = %acct.id, stage = "provider-oauth", "gateway 401: upstream provider rejected account credentials");
        flag_account_reauth(state, acct.id, "upstream provider rejected account credentials");
        return Err(Ok(auth_error(AuthStage::ProviderOauth, is_anthropic)));
    }

    // Upstream says the account is rate-limited. Mirroring it verbatim strands
    // the session until the window resets (hours, for a spent 5h/weekly
    // window) — if a sibling account has allocation, rebind and send the
    // worker straight back around. A response was received but no body bytes
    // were forwarded yet, so discarding it is safe. Failovers are cooled down
    // per session, so a burst-RPM 429 doesn't ping-pong the binding.
    if status == StatusCode::TOO_MANY_REQUESTS
        && let Some(target) =
            pick_failover_target(state, session_token, acct.id, info.model().as_deref()).await
        && rebind_session(state, &target, acct.id).await
    {
        return Err(Ok(failover_retry_response(
            &target.account_name,
            &target.reason,
            is_anthropic,
        )));
    }

    // A successful upstream call clears any soft-limit block on this session:
    // after the user switches accounts (or a window resets) the next
    // 2xx dismisses the banner. Unconditional: another replica may hold the block.
    if status.is_success() {
        match &obs.trace_session_id {
            Some(sid) => clear_soft_limit_block(state, sid).await,
            None => clear_soft_limit_block_for_token(state, session_token).await,
        }
    }
    // Usage ticker. Out of band on purpose: it reaches the agent as its own
    // turn, so nothing here can reshape the request that was just forwarded.
    if status.is_success() {
        state.deliver_usage_notice(acct, session_token).await;
    }
    // A successful upstream call means the account's credentials are good again —
    // clear any reauth flag. Gated in-memory, so this is free unless the
    // account was actually flagged.
    if status.is_success() {
        clear_account_reauth(state, acct.id);
    }
    Ok((upstream, status))
}

/// Mirror status + headers back to the client untouched (retry-after, 429,
/// 529, SSE content-type — all verbatim) and stream the body, tool-guarded
/// and/or teed to the observers.
async fn relay_response(
    state: &impl ProxyContext,
    auth: &Authenticated,
    mut obs: Observers,
    mut info: RequestInfo,
    upstream: reqwest::Response,
    status: StatusCode,
    is_anthropic: bool,
) -> Reply {
    let Authenticated { session_token, acct } = auth;
    let tool_guard = obs.tool_guard.take();
    let mut builder = Response::builder().status(status);
    for (name, value) in upstream.headers() {
        let n = name.as_str().to_ascii_lowercase();
        if skip_response_header(&n) {
            continue;
        }
        if let (Ok(hn), Ok(hv)) = (
            axum::http::HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            builder = builder.header(hn, hv);
        }
    }
    // A metered family (Fireworks, Google) bills per token, so its usage must
    // be recorded from the response itself: the usage object (JSON body or
    // terminal SSE frame) plus, for Fireworks, the two headers the provider
    // bills against. Read the headers now, before the body is consumed.
    let metered = obs.fireworks || obs.google;
    let usage_session = if metered && status.is_success() {
        match obs.affinity_session.clone().or_else(|| obs.trace_session_id.clone()) {
            Some(sid) => Some(sid),
            None => session_id_for_token(state, session_token).await,
        }
    } else {
        None
    };
    let usage_headers = usage_session.as_ref().filter(|_| obs.fireworks).map(|_| {
        let header = |name: &str| {
            upstream.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned)
        };
        (header("fireworks-prompt-tokens"), header("fireworks-cached-prompt-tokens"))
    });

    let header = |name: http::header::HeaderName| {
        upstream.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned)
    };
    let (content_type, content_encoding) =
        (header(http::header::CONTENT_TYPE), header(http::header::CONTENT_ENCODING));
    let encoded =
        content_encoding.as_deref().is_some_and(|e| !e.trim().eq_ignore_ascii_case("identity"));
    let guarded = match tool_guard.filter(|_| status.is_success()) {
        None => None,
        Some(g) => match guardable(content_type.as_deref(), content_encoding.as_deref()) {
            Some(sse) => Some((g, sse)),
            None if encoded => {
                tracing::warn!(account = %acct.id, "encoded upstream response cannot be tool-guarded");
                return Ok(super::toolguard::unscannable_response(is_anthropic));
            }
            None => None,
        },
    };
    let resp_stream: BoxStream<'static, Result<Bytes, reqwest::Error>> = match guarded {
        Some((g, sse)) => {
            guard_stream(upstream.bytes_stream(), g, sse, guard_context(state)).boxed()
        }
        None => upstream.bytes_stream().boxed(),
    };
    let readable_sse = status.is_success()
        && !encoded
        && content_type.as_deref().is_some_and(|ct| ct.starts_with("text/event-stream"));
    let resp_stream = match info.voice.take() {
        Some(tee) => tee.apply(resp_stream, readable_sse),
        None => resp_stream,
    };

    // Fast path (nothing to observe): stream the response straight through.
    if obs.langfuse.is_none() && usage_session.is_none() {
        return builder.body(Body::from_stream(resp_stream)).map_err(|e| {
            tracing::error!("gateway response build error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        });
    }

    let tx = spawn_observer(state, acct, obs, info, usage_session, usage_headers);
    let resp_stream = resp_stream.map(move |chunk| {
        if let Ok(bytes) = &chunk {
            let _ = tx.send(bytes.clone());
        }
        chunk
    });
    builder.body(Body::from_stream(resp_stream)).map_err(|e| {
        tracing::error!("gateway response build error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// Observed path: tee the response body. Each chunk is forwarded to the client
/// verbatim and handed, refcounted, to an observer task over an unbounded
/// channel: no chunk is dropped and the client stream never waits, since the
/// task only appends or digests. When the upstream stream ends the task meters
/// the usage and reports the trace.
fn spawn_observer(
    state: &impl ProxyContext,
    acct: &Account,
    obs: Observers,
    info: RequestInfo,
    usage_session: Option<String>,
    usage_headers: Option<(Option<String>, Option<String>)>,
) -> tokio::sync::mpsc::UnboundedSender<Bytes> {
    let RequestInfo { traced_request, request_redacted, request_shape, request_model, .. } = info;
    let trace_session_id = obs.trace_session_id;
    let trace_account_id = acct.id.to_string();
    let trace_model = request_model.clone();
    let family = Family::from_provider(&acct.provider);
    let pool = state.proxy_pool().clone();
    // TPM accounting rides the same per-response usage the metering path captures
    // (Fireworks, Google): the running window total the next request gates against.
    let rate_windows = acct.rate_limits.tpm.is_some().then(|| state.rate_windows().clone());
    let rate_provider = acct.id;
    let langfuse = obs.langfuse;
    let mut digest = langfuse.as_ref().and_then(|sink| sink.response_digest(family));
    let keep_body = usage_session.is_some() || (langfuse.is_some() && digest.is_none());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
    tokio::spawn(async move {
        let mut buf = Vec::new();
        while let Some(chunk) = rx.recv().await {
            if let Some(digest) = digest.as_mut() {
                digest.feed(&chunk);
            }
            if keep_body {
                buf.extend_from_slice(&chunk);
            }
        }
        let captured = match family {
            Family::Google => crate::cost::parse_gemini_usage(&buf),
            Family::Fireworks => {
                let (prompt_hdr, cached_hdr) = usage_headers.unwrap_or_default();
                crate::cost::parse_fireworks_usage(
                    &buf,
                    prompt_hdr.as_deref(),
                    cached_hdr.as_deref(),
                )
            }
            Family::Anthropic | Family::Openai => None,
        };
        if let (Some(session_id), Some(captured)) = (usage_session, captured) {
            if let Some(windows) = &rate_windows {
                let u = &captured.usage;
                let total = u64::try_from(u.input).unwrap_or(0)
                    + u64::try_from(u.cached_input).unwrap_or(0)
                    + u64::try_from(u.output).unwrap_or(0);
                super::ratelimit::note_tokens(windows, rate_provider, total);
            }
            record_metered_usage(pool, family, session_id, request_model, captured).await;
        }
        if let Some(langfuse) = langfuse {
            let shape = request_shape.as_ref().and_then(|shape| shape.get());
            let request = traced_request.or_else(|| shape.map(|s| s.summary.clone()));
            let model = shape.and_then(|s| s.model.clone()).or(trace_model);
            let response_summary = digest.map(ResponseDigest::finish);
            let response = if response_summary.is_some() { Vec::new() } else { buf };
            langfuse
                .report(TraceCapture {
                    family,
                    session_id: trace_session_id,
                    account_id: trace_account_id,
                    model,
                    request,
                    request_redacted,
                    response,
                    response_summary,
                })
                .await;
        }
    });
    tx
}

/// Whether an inbound endpoint can supply live voice text.
#[must_use]
pub fn teeable_request(method: &axum::http::Method, tail: &str) -> bool {
    *method == axum::http::Method::POST && tail.trim_end_matches('/').ends_with("/v1/messages")
}

/// Linear proxy pipeline: authenticate, admit, resolve the upstream, forward,
/// relay the response.
pub async fn passthrough(
    state: impl ProxyContext,
    req: Request,
    family: Family,
    prefix: &str,
    upstream_base: &str,
) -> Result<Response, StatusCode> {
    let is_anthropic = family == Family::Anthropic;
    if declared_body_len(req.headers()).is_some_and(|n| n > MAX_BUFFERED_BODY) {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let auth = match authenticate(&state, req.headers(), is_anthropic).await {
        Ok(auth) => auth,
        Err(reply) => return reply,
    };
    let admission = match admit(&state, &auth, is_anthropic).await {
        Ok(admission) => admission,
        Err(reply) => return reply,
    };
    let (uri, req_method) = (req.uri().clone(), req.method().clone());
    let target = match resolve_upstream(
        &state,
        &auth.acct,
        &uri,
        &req_method,
        family,
        prefix,
        upstream_base,
        is_anthropic,
    )
    .await
    {
        Ok(target) => target,
        Err(reply) => return reply,
    };
    let mut headers = forward_headers(req.headers(), &auth.acct, &target.access_token)?;
    let voice_candidate =
        teeable_request(&req_method, uri.path().strip_prefix(prefix).unwrap_or_else(|| uri.path()));
    let obs = observers(&state, &auth, &mut headers, voice_candidate).await?;
    let path_model = (family == Family::Google)
        .then(|| uri.path().strip_prefix(prefix).and_then(google_path_model))
        .flatten();
    let (body, info) =
        match prepare_body(&state, &auth, admission, &obs, req, is_anthropic, path_model).await {
            Ok(prepared) => prepared,
            Err(reply) => return reply,
        };
    let UpstreamTarget { client, url, method, .. } = target;
    let request = client.request(method, &url).headers(headers).body(body);
    let (upstream, status) =
        match send_upstream(&state, &auth, request, &info, &obs, is_anthropic).await {
            Ok(sent) => sent,
            Err(reply) => return reply,
        };
    relay_response(&state, &auth, obs, info, upstream, status, is_anthropic).await
}

#[cfg(test)]
mod tests {
    use super::{
        Account, Family, MAX_BUFFERED_BODY, RateLimits, SESSION_AFFINITY_HEADER, buffered_payload,
        declared_body_len, forward_headers, session_token_from, skip_request_header,
        wants_affinity,
    };
    use axum::body::Bytes;
    use axum::http::HeaderMap;

    #[test]
    fn an_oversized_declared_body_is_rejected_before_it_is_read() {
        let with = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert(http::header::CONTENT_LENGTH, v.parse().unwrap());
            declared_body_len(&h)
        };
        assert_eq!(with("17"), Some(17));
        assert_eq!(with("nonsense"), None);
        assert_eq!(declared_body_len(&HeaderMap::new()), None);
        assert!(with("34000000").is_some_and(|n| n > MAX_BUFFERED_BODY));
        assert!(with("1000000").is_some_and(|n| n <= MAX_BUFFERED_BODY));
    }

    #[test]
    fn actor_authorization_dummy_is_stripped_before_forwarding() {
        assert!(skip_request_header("x-openai-actor-authorization"));
        assert!(!skip_request_header("chatgpt-account-id"));
    }

    fn static_account(auth_scheme: &str) -> Account {
        Account {
            id: uuid::Uuid::nil(),
            provider: "anthropic-compatible".into(),
            access_token: Some("sk-ant-api-test".into()),
            refresh_token: None,
            expires_at: None,
            provider_account_id: None,
            base_url: Some("https://api.anthropic.com".into()),
            auth_scheme: auth_scheme.into(),
            soft_limits: cctui_proto::api::soft_limit::SoftLimits::default(),
            usage_notices: cctui_proto::api::gateway::UsageNotices::default(),
            provider_settings: None,
            rate_limits: RateLimits::default(),
        }
    }

    /// An `api_key` account authenticates upstream with `x-api-key` and no
    /// bearer, replacing any client-sent key; bearer accounts are unchanged.
    #[test]
    fn api_key_accounts_forward_x_api_key_instead_of_a_bearer() {
        let mut inbound = HeaderMap::new();
        inbound.insert("x-api-key", "client-supplied".parse().unwrap());
        inbound.insert("anthropic-version", "2023-06-01".parse().unwrap());

        let out = forward_headers(&inbound, &static_account("api_key"), "sk-ant-api-test").unwrap();
        assert_eq!(out.get("x-api-key").unwrap(), "sk-ant-api-test");
        assert!(out.get(reqwest::header::AUTHORIZATION).is_none());
        assert_eq!(out.get("anthropic-version").unwrap(), "2023-06-01");

        let out = forward_headers(&inbound, &static_account("bearer"), "tok").unwrap();
        assert_eq!(out.get(reqwest::header::AUTHORIZATION).unwrap(), "Bearer tok");
    }

    /// The Gemini SDK carries the session token as `x-goog-api-key`; the
    /// gateway reads it from there, strips it, and sends the account's key
    /// upstream under the same name with no bearer and no `x-api-key`.
    #[test]
    fn google_accounts_swap_the_goog_api_key_header() {
        let mut inbound = HeaderMap::new();
        inbound.insert("x-goog-api-key", "cctui_s_tok".parse().unwrap());
        inbound.insert("x-goog-api-client", "google-genai-sdk/1.0".parse().unwrap());
        assert_eq!(session_token_from(&inbound).as_deref(), Some("cctui_s_tok"));
        assert!(skip_request_header("x-goog-api-key"));

        let mut acct = static_account("api_key");
        acct.provider = "google".into();
        acct.base_url = None;
        let out = forward_headers(&inbound, &acct, "AIza-real").unwrap();
        assert_eq!(out.get("x-goog-api-key").unwrap(), "AIza-real");
        assert!(out.get("x-api-key").is_none());
        assert!(out.get(reqwest::header::AUTHORIZATION).is_none());
        assert_eq!(out.get("x-goog-api-client").unwrap(), "google-genai-sdk/1.0");
    }

    #[test]
    fn the_bearer_wins_over_a_goog_key_and_blanks_are_no_token() {
        let mut both = HeaderMap::new();
        both.insert(axum::http::header::AUTHORIZATION, "Bearer tok".parse().unwrap());
        both.insert("x-goog-api-key", "other".parse().unwrap());
        assert_eq!(session_token_from(&both).as_deref(), Some("tok"));
        let mut blank = HeaderMap::new();
        blank.insert("x-goog-api-key", "".parse().unwrap());
        assert_eq!(session_token_from(&blank), None);
        assert_eq!(session_token_from(&HeaderMap::new()), None);
    }

    /// Per family, a body as its harness sends it, with key order and
    /// whitespace a re-serialization would not reproduce.
    fn family_bodies() -> [(Family, Option<&'static str>, &'static [u8]); 4] {
        [
            (
                Family::Anthropic,
                None,
                br#"{"model": "claude-opus-5", "messages":[{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/a","old_string":"x","new_string":"y"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}],"thinking":{"type":"adaptive","display":"omitted"}}"#,
            ),
            (
                Family::Openai,
                None,
                br#"{"model":"gpt-5.5","stream":true,"input":[{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"ls\"}","call_id":"c1"}],  "tools":[{"type":"function","name":"shell","parameters":{"type":"object","properties":{"z":{},"a":{}}}}]}"#,
            ),
            (
                Family::Fireworks,
                None,
                b"{\"model\":\"accounts/fireworks/models/kimi-k3\",\n \"messages\":[{\"role\":\"system\",\"content\":\"s\"},{\"role\":\"user\",\"content\":\"q\"}],\"tools\":[{\"type\":\"function\",\"function\":{\"name\":\"read\",\"parameters\":{\"properties\":{\"path\":{},\"limit\":{}}}}}],\"stream\":true}",
            ),
            (
                Family::Google,
                Some("gemini-2.5-pro"),
                br#"{"contents":[{"role":"user","parts":[{"text":"q"}]}], "systemInstruction":{"parts":[{"text":"s"}]},"generationConfig":{"topP":1,"maxOutputTokens":8}}"#,
            ),
        ]
    }

    /// Every family forwards the client's bytes unchanged, even when the body
    /// is buffered and parsed for metering, tracing or a model gate.
    #[test]
    fn every_family_forwards_the_request_bytes_verbatim() {
        for (family, path_model, raw) in family_bodies() {
            let original = Bytes::from_static(raw);
            let parsed = serde_json::from_slice::<serde_json::Value>(&original)
                .unwrap_or_else(|e| panic!("{family:?} fixture is not JSON: {e}"));
            assert_ne!(
                Bytes::from(parsed.to_string()),
                original,
                "{family:?} fixture must not survive a re-serialization"
            );
            let (payload, traced, model) =
                buffered_payload(original, path_model.map(str::to_owned));
            assert_eq!(payload.as_bytes(), Some(raw), "{family:?} body was rewritten");
            assert_eq!(traced, Some(parsed), "{family:?}");
            assert!(model.is_some(), "{family:?} request model");
        }
    }

    #[test]
    fn the_body_model_wins_over_the_path_model() {
        let (_, _, model) = buffered_payload(
            Bytes::from_static(br#"{"model":"kimi"}"#),
            Some("gemini-2.5-pro".into()),
        );
        assert_eq!(model.as_deref(), Some("kimi"));
    }

    #[test]
    fn a_non_json_body_is_forwarded_untouched() {
        let (payload, traced, model) = buffered_payload(Bytes::from_static(b"not json"), None);
        assert_eq!(payload.as_bytes(), Some(&b"not json"[..]));
        assert!(traced.is_none());
        assert!(model.is_none());
    }

    #[test]
    fn only_fireworks_carries_the_session_affinity_header() {
        let off = serde_json::json!({ "session_affinity": false });
        for family in [Family::Anthropic, Family::Openai, Family::Google] {
            assert!(!wants_affinity(family, None), "{family:?}");
        }
        assert!(wants_affinity(Family::Fireworks, None));
        assert!(!wants_affinity(Family::Fireworks, Some(&off)));
        assert_eq!(SESSION_AFFINITY_HEADER, "x-session-affinity");
    }
}

#[cfg(test)]
mod trace_shape_tests {
    use super::{
        Arc, MAX_BUFFERED_BODY, RequestShape, RequestTrace, ShapeTee, must_buffer, request_shape,
    };
    use axum::body::Bytes;
    use futures_util::StreamExt;
    use serde_json::{Value, json};

    const BODY: &[u8] = br#"{"model":"claude-opus-5","stream":true,"system":[{"type":"text","text":"s"}],"messages":[{"role":"user","content":"q é"}],"metadata":{"user_id":"u"},"max_tokens":64}"#;

    fn tee_of(
        chunks: Vec<Result<Bytes, axum::Error>>,
    ) -> (ShapeTee, Arc<std::sync::OnceLock<RequestShape>>) {
        ShapeTee::new(futures_util::stream::iter(chunks).boxed())
    }

    #[test]
    fn only_a_content_capturing_trace_holds_the_body_back() {
        assert!(!must_buffer(RequestTrace::Shape, false, false, false));
        assert!(!must_buffer(RequestTrace::Off, false, false, false));
        assert!(must_buffer(RequestTrace::Full, false, false, false));
        assert!(must_buffer(RequestTrace::Shape, true, false, false), "metering reads the body");
        assert!(must_buffer(RequestTrace::Shape, false, true, false), "a model gate reads it");
        assert!(must_buffer(RequestTrace::Shape, false, false, true), "voice reads it");
    }

    #[tokio::test]
    async fn a_content_free_trace_forwards_the_body_chunks_unchanged() {
        let chunks: Vec<Bytes> = BODY.chunks(7).map(Bytes::copy_from_slice).collect();
        let (tee, shape) = tee_of(chunks.iter().cloned().map(Ok).collect());
        assert!(shape.get().is_none(), "read only once the body has gone upstream");
        let forwarded: Vec<Bytes> = tee.map(Result::unwrap).collect().await;
        assert_eq!(forwarded, chunks);
        assert_eq!(forwarded.concat(), BODY);
        assert_eq!(shape.get(), request_shape(BODY).as_ref());
        let shape = shape.get().unwrap();
        assert_eq!(shape.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(
            shape.summary,
            json!({
                "model": "claude-opus-5",
                "stream": true,
                "max_tokens": 64,
                "messages_count": 1,
                "system_count": 1,
                "redacted": true,
            })
        );
    }

    #[tokio::test]
    async fn a_body_past_the_buffer_cap_still_streams_but_is_not_shaped() {
        static FILL: [u8; 1 << 20] = [b'a'; 1 << 20];
        let mut chunks = vec![Bytes::from_static(br#"{"model":"m","x":""#)];
        chunks.extend((0..=MAX_BUFFERED_BODY / FILL.len()).map(|_| Bytes::from_static(&FILL)));
        chunks.push(Bytes::from_static(br#""}"#));
        let total: usize = chunks.iter().map(Bytes::len).sum();
        let (tee, shape) = tee_of(chunks.into_iter().map(Ok).collect());
        let forwarded: usize = tee.map(|c| c.unwrap().len()).collect::<Vec<_>>().await.iter().sum();
        assert!(total > MAX_BUFFERED_BODY);
        assert_eq!(forwarded, total);
        assert!(shape.get().is_none());
    }

    #[tokio::test]
    async fn a_failed_upload_is_not_shaped() {
        let chunks = vec![Ok(Bytes::from_static(&BODY[..10])), Err(axum::Error::new("reset"))];
        let (tee, shape) = tee_of(chunks);
        let items: Vec<_> = tee.collect().await;
        assert_eq!(items.len(), 2);
        assert!(shape.get().is_none());
    }

    #[test]
    fn the_shape_scan_accepts_exactly_what_a_value_parse_accepts() {
        let deep = format!("{{\"tools\":{}{}}}", "[".repeat(200), "]".repeat(200));
        let cases: [&[u8]; 14] = [
            BODY,
            b"",
            b"[1,2]",
            b"\"str\"",
            b"null",
            br#"{"model":"m"} x"#,
            b"{\"messages\":[\"\xff\"]}",
            br#"{"messages":["\ud800"]}"#,
            br#"{"model":1e400}"#,
            br#"{"other":1e400}"#,
            br#"{"other":{"a":[1,{"b":"é\n"}]}}"#,
            br#"{"messages":[1],"messages":"x","model":"escaped"}"#,
            br#"{"tools":18446744073709551616,"stream":-0}"#,
            deep.as_bytes(),
        ];
        for case in cases {
            assert_eq!(
                request_shape(case).is_some(),
                serde_json::from_slice::<Value>(case).is_ok(),
                "{}",
                String::from_utf8_lossy(case)
            );
        }
        let dup = request_shape(br#"{"messages":[1],"messages":"x","model":"escaped"}"#);
        let dup = dup.unwrap();
        assert_eq!(dup.model.as_deref(), Some("escaped"));
        assert_eq!(dup.summary["messages_count"], json!(1));
        assert_eq!(request_shape(b"[1,2]").unwrap().summary, json!({"redacted": true}));
    }
}
