use std::collections::HashMap;

use opentelemetry::trace::{SpanId, TraceId, TracerProvider as _};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use tracing_subscriber::layer::SubscriberExt;

use super::{current_traceparent, enabled, exporter_requested, init, set_parent, widen};

const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const PARENT_ID: &str = "00f067aa0ba902b7";

fn traceparent() -> String {
    format!("00-{TRACE_ID}-{PARENT_ID}-01")
}

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> =
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    move |key| map.get(key).cloned()
}

#[test]
fn exporter_needs_an_endpoint_and_no_sdk_disable() {
    assert!(!exporter_requested(env(&[])));
    assert!(!exporter_requested(env(&[("OTEL_EXPORTER_OTLP_ENDPOINT", " ")])));
    assert!(exporter_requested(env(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318")])));
    assert!(exporter_requested(env(&[(
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "http://127.0.0.1:4318/v1/traces"
    )])));
    assert!(!exporter_requested(env(&[
        ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318"),
        ("OTEL_SDK_DISABLED", "TRUE"),
    ])));
    assert!(exporter_requested(env(&[
        ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318"),
        ("OTEL_SDK_DISABLED", "false"),
    ])));
}

#[tokio::test]
async fn init_without_endpoint_creates_no_exporter() {
    let requested = exporter_requested(|key| std::env::var(key).ok());
    let guard = init("cctui-telemetry-test", "0.0.0", "info");
    assert_eq!(guard.provider.is_some(), requested);
    assert_eq!(enabled(), requested);
    if !requested {
        assert!(guard.provider.is_none(), "no provider, so no exporter task or HTTP client");
        assert!(!enabled());
    }
    drop(guard);
    assert!(!enabled());
}

#[test]
fn set_parent_then_current_traceparent_roundtrips() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder().with_simple_exporter(exporter.clone()).build();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
    let own_span_id = tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!("child");
        set_parent(&span, &traceparent(), Some("vendor=abc"));
        let (carried, state) = span.in_scope(current_traceparent).expect("span has a context");
        let parts: Vec<&str> = carried.split('-').collect();
        assert_eq!(parts.len(), 4, "{carried}");
        assert_eq!(parts[0], "00");
        assert_eq!(parts[1], TRACE_ID);
        assert_ne!(parts[2], PARENT_ID, "the carrier names this span, not its parent");
        assert_eq!(parts[3], "01");
        assert_eq!(state.as_deref(), Some("vendor=abc"));
        parts[2].to_owned()
    });
    let spans = exporter.get_finished_spans().expect("exporter readable");
    let child = spans.iter().find(|s| s.name == "child").expect("child span exported");
    assert_eq!(child.span_context.trace_id(), TraceId::from_hex(TRACE_ID).unwrap());
    assert_eq!(child.parent_span_id, SpanId::from_hex(PARENT_ID).unwrap());
    assert_eq!(child.span_context.span_id(), SpanId::from_hex(&own_span_id).unwrap());
}

#[test]
fn invalid_traceparent_leaves_the_span_a_root() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder().with_simple_exporter(exporter.clone()).build();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!("root");
        set_parent(&span, "not-a-traceparent", None);
        let (carried, _) = span.in_scope(current_traceparent).expect("span has a context");
        assert!(!carried.contains(TRACE_ID));
    });
    let spans = exporter.get_finished_spans().expect("exporter readable");
    let root = spans.iter().find(|s| s.name == "root").expect("root span exported");
    assert_eq!(root.parent_span_id, SpanId::INVALID);
}

#[test]
fn current_traceparent_is_none_without_an_otel_layer() {
    tracing::subscriber::with_default(tracing_subscriber::registry(), || {
        let span = tracing::info_span!("plain");
        assert!(span.in_scope(current_traceparent).is_none());
        set_parent(&span, &traceparent(), None);
        assert!(span.in_scope(current_traceparent).is_none());
    });
    assert!(current_traceparent().is_none());
}

#[test]
fn exporter_filter_admits_request_and_frame_spans_under_a_narrow_log_filter() {
    let subscriber =
        tracing_subscriber::registry().with(widen(tracing_subscriber::EnvFilter::new("error")));
    tracing::subscriber::with_default(subscriber, || {
        assert!(tracing::enabled!(target: "cctui_telemetry::http", tracing::Level::INFO));
        assert!(tracing::enabled!(target: "cctui_server_core::bus", tracing::Level::INFO));
        assert!(!tracing::enabled!(target: "cctui_server_core::bus", tracing::Level::DEBUG));
    });
}
