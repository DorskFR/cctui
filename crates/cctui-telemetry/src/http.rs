use std::time::Duration;

use axum::extract::MatchedPath;
use http::{Request, Response};
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::{DefaultOnRequest, MakeSpan, OnResponse, TraceLayer};
use tracing::Span;
use tracing::field::Empty;

const UNTRACED: &[&str] = &["/health", "/ready", "/readyz", "/metrics"];

pub type HttpTraceLayer = TraceLayer<
    SharedClassifier<ServerErrorsAsFailures>,
    ServerSpan,
    DefaultOnRequest,
    RecordStatus,
>;

/// Apply with `Router::layer` so `MatchedPath` is already set.
#[must_use]
pub fn trace_layer() -> HttpTraceLayer {
    TraceLayer::new_for_http().make_span_with(ServerSpan).on_response(RecordStatus)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ServerSpan;

impl<B> MakeSpan<B> for ServerSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        let path = request.uri().path();
        if UNTRACED.contains(&path) {
            return Span::none();
        }
        let method = request.method().as_str();
        let route = request.extensions().get::<MatchedPath>().map(MatchedPath::as_str);
        let name = route.map_or_else(|| method.to_owned(), |route| format!("{method} {route}"));
        let span = tracing::info_span!(
            "http.request",
            otel.name = %name,
            otel.kind = "server",
            otel.status_code = Empty,
            http.request.method = method,
            http.route = route,
            url.path = path,
            http.response.status_code = Empty,
        );
        if let Some(traceparent) = header(request, crate::TRACEPARENT) {
            crate::set_parent(&span, traceparent, header(request, crate::TRACESTATE));
        }
        span
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RecordStatus;

impl<B> OnResponse<B> for RecordStatus {
    fn on_response(self, response: &Response<B>, _latency: Duration, span: &Span) {
        let status = response.status();
        span.record("http.response.status_code", u64::from(status.as_u16()));
        if status.is_server_error() {
            span.record("otel.status_code", "error");
        }
    }
}

fn header<'a, B>(request: &'a Request<B>, name: &str) -> Option<&'a str> {
    request.headers().get(name).and_then(|value| value.to_str().ok())
}
