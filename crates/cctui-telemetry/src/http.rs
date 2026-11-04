use std::time::Duration;

use axum::extract::MatchedPath;
use http::{Request, Response};
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::{
    DefaultOnBodyChunk, DefaultOnEos, DefaultOnRequest, MakeSpan, OnFailure, OnResponse, TraceLayer,
};
use tracing::Span;
use tracing::field::Empty;

const UNTRACED: &[&str] = &["/health", "/ready", "/readyz", "/metrics", "/api/v1/otlp/v1/traces"];

pub type HttpTraceLayer = TraceLayer<
    SharedClassifier<ServerErrorsAsFailures>,
    ServerSpan,
    DefaultOnRequest,
    RecordStatus,
    DefaultOnBodyChunk,
    DefaultOnEos,
    RecordFailure,
>;

/// Apply with `Router::layer` so `MatchedPath` is already set.
#[must_use]
pub fn trace_layer() -> HttpTraceLayer {
    TraceLayer::new_for_http()
        .make_span_with(ServerSpan)
        .on_response(RecordStatus)
        .on_failure(RecordFailure)
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
        // No matched route: stray traffic, not worth an exported span.
        let Some(route) = request.extensions().get::<MatchedPath>().map(MatchedPath::as_str) else {
            tracing::debug!(method, path, host = header(request, "host"), "unrouted request");
            return Span::none();
        };
        let span = tracing::info_span!(
            "http.request",
            otel.name = %format_args!("{method} {route}"),
            otel.kind = "server",
            otel.status_code = Empty,
            otel.status_description = Empty,
            error.type = Empty,
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
        if status.is_client_error() || status.is_server_error() {
            span.record("otel.status_code", "error");
            span.record("error.type", status.as_str());
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RecordFailure;

impl<C: std::fmt::Display> OnFailure<C> for RecordFailure {
    fn on_failure(&mut self, failure: C, latency: Duration, span: &Span) {
        span.record("otel.status_code", "error");
        span.record("otel.status_description", tracing::field::display(&failure));
        tracing::error!(
            parent: span,
            classification = %failure,
            latency_ms = u64::try_from(latency.as_millis()).unwrap_or(u64::MAX),
            "response failed"
        );
    }
}

fn header<'a, B>(request: &'a Request<B>, name: &str) -> Option<&'a str> {
    request.headers().get(name).and_then(|value| value.to_str().ok())
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use axum::http::StatusCode;
    use axum::routing::get;
    use opentelemetry::trace::{SpanKind, Status, TracerProvider as _};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tracing_subscriber::layer::SubscriberExt;

    const STRAY: &str = "/v2/suumo/tags/list";

    fn app() -> axum::Router {
        axum::Router::new()
            .route("/ok", get(|| async { "ok" }))
            .route("/boom", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
            .route("/err", get(|| async { Err::<&str, _>(StatusCode::BAD_REQUEST) }))
            .layer(super::trace_layer())
    }

    async fn serve() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app()).await.unwrap() });
        addr
    }

    async fn status_of(addr: SocketAddr, path: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: stray.example\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response.lines().next().unwrap_or_default().to_owned()
    }

    async fn server_spans(exporter: &InMemorySpanExporter, expected: usize) -> Vec<SpanData> {
        let spans = || {
            exporter
                .get_finished_spans()
                .unwrap()
                .into_iter()
                .filter(|s| s.span_kind == SpanKind::Server)
                .collect::<Vec<_>>()
        };
        for _ in 0..200 {
            if spans().len() >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        spans()
    }

    fn attribute(span: &SpanData, key: &str) -> Option<String> {
        span.attributes
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| kv.value.as_str().into_owned())
    }

    fn by_route<'a>(spans: &'a [SpanData], route: &str) -> &'a SpanData {
        spans
            .iter()
            .find(|s| attribute(s, "http.route").as_deref() == Some(route))
            .unwrap_or_else(|| panic!("no span for {route}"))
    }

    #[tokio::test]
    async fn unrouted_requests_export_nothing_and_errors_mark_the_span() {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder().with_simple_exporter(exporter.clone()).build();
        let _default = tracing::subscriber::set_default(
            tracing_subscriber::registry()
                .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test"))),
        );
        let addr = serve().await;

        assert!(status_of(addr, STRAY).await.contains("404"));
        assert!(status_of(addr, "/").await.contains("404"));
        assert!(status_of(addr, "/ok").await.contains("200"));
        assert!(status_of(addr, "/boom").await.contains("500"));
        assert!(status_of(addr, "/err").await.contains("400"));

        let spans = server_spans(&exporter, 3).await;
        assert_eq!(spans.len(), 3, "one span per routed request only");
        assert!(spans.iter().all(|s| attribute(s, "url.path").as_deref() != Some(STRAY)));
        assert!(spans.iter().all(|s| attribute(s, "url.path").as_deref() != Some("/")));

        assert_eq!(by_route(&spans, "/ok").status, Status::Unset);
        let boom = by_route(&spans, "/boom");
        assert!(matches!(boom.status, Status::Error { .. }), "{:?}", boom.status);
        assert_eq!(attribute(boom, "error.type").as_deref(), Some("500"));
        let err = by_route(&spans, "/err");
        assert!(matches!(err.status, Status::Error { .. }), "{:?}", err.status);
        assert_eq!(attribute(err, "http.response.status_code").as_deref(), Some("400"));
    }
}
