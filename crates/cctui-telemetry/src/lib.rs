//! Tracing subscriber with opt-in OTLP/HTTP trace export.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::propagation::TextMapPropagator;
use opentelemetry::trace::{TraceContextExt, TracerProvider as _};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::trace::span_processor_with_async_runtime::BatchSpanProcessor;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

#[cfg(feature = "http")]
pub mod http;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const TRACEPARENT: &str = "traceparent";
const TRACESTATE: &str = "tracestate";

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Flushes and shuts the tracer provider down on drop, waiting at most
/// [`SHUTDOWN_TIMEOUT`].
#[must_use = "dropping the guard shuts the trace exporter down"]
pub struct TelemetryGuard {
    provider: Option<SdkTracerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        let Some(provider) = self.provider.take() else {
            return;
        };
        ENABLED.store(false, Ordering::Relaxed);
        // The async batch processor ignores the shutdown timeout and blocks on
        // the runtime, so the wait is bounded here instead.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(provider.shutdown());
        });
        match rx.recv_timeout(SHUTDOWN_TIMEOUT) {
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::warn!(%err, "trace exporter shutdown failed"),
            Err(_) => tracing::warn!(?SHUTDOWN_TIMEOUT, "trace exporter shutdown timed out"),
        }
    }
}

/// Installs the global tracing subscriber: `EnvFilter` (`RUST_LOG`, else
/// `default_filter`) + fmt, plus the `tracing-opentelemetry` layer only when an OTLP
/// endpoint is configured and `OTEL_SDK_DISABLED` is not `true`. Registers the
/// W3C trace-context propagator globally.
pub fn init(
    service_name: &'static str,
    service_version: &'static str,
    default_filter: &str,
) -> TelemetryGuard {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let (provider, build_error) = if exporter_requested(|key| std::env::var(key).ok()) {
        build_provider(service_name, service_version)
            .map_or_else(|err| (None, Some(err)), |provider| (Some(provider), None))
    } else {
        (None, None)
    };
    let otel = provider.as_ref().map(|provider| {
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer(service_name))
            .with_filter(otel_filter(default_filter))
    });
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(env_filter(default_filter)))
        .with(otel)
        .try_init();
    if let Some(err) = build_error {
        tracing::warn!(%err, "OTLP trace exporter could not be built; tracing export disabled");
    }
    ENABLED.store(provider.is_some(), Ordering::Relaxed);
    if provider.is_some() {
        tracing::info!(service_name, "OTLP trace export enabled");
    }
    TelemetryGuard { provider }
}

/// Whether the OTLP exporter is active.
#[must_use]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The W3C carrier of the current tracing span's context, `None` when the
/// span carries no valid otel context.
#[must_use]
pub fn current_traceparent() -> Option<(String, Option<String>)> {
    let cx = tracing::Span::current().context();
    if !cx.span().span_context().is_valid() {
        return None;
    }
    let mut carrier: HashMap<String, String> = HashMap::with_capacity(2);
    TraceContextPropagator::new().inject_context(&cx, &mut carrier);
    let traceparent = carrier.remove(TRACEPARENT)?;
    let tracestate = carrier.remove(TRACESTATE).filter(|s| !s.is_empty());
    Some((traceparent, tracestate))
}

/// Sets `span`'s otel parent from a W3C `traceparent` (and optional
/// `tracestate`). No-op on invalid input.
pub fn set_parent(span: &tracing::Span, traceparent: &str, tracestate: Option<&str>) {
    let mut carrier: HashMap<String, String> = HashMap::with_capacity(2);
    carrier.insert(TRACEPARENT.to_owned(), traceparent.trim().to_owned());
    if let Some(tracestate) = tracestate {
        carrier.insert(TRACESTATE.to_owned(), tracestate.to_owned());
    }
    let cx = TraceContextPropagator::new().extract(&carrier);
    if cx.span().span_context().is_valid() {
        let _ = span.set_parent(cx);
    }
}

fn exporter_requested(var: impl Fn(&str) -> Option<String>) -> bool {
    let set = |key: &str| var(key).is_some_and(|v| !v.trim().is_empty());
    let disabled = var("OTEL_SDK_DISABLED").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
    !disabled && (set("OTEL_EXPORTER_OTLP_ENDPOINT") || set("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"))
}

fn build_provider(
    service_name: &'static str,
    service_version: &'static str,
) -> Result<SdkTracerProvider, opentelemetry_otlp::ExporterBuildError> {
    let exporter = opentelemetry_otlp::SpanExporter::builder().with_http().build()?;
    let processor =
        BatchSpanProcessor::builder(exporter, opentelemetry_sdk::runtime::Tokio).build();
    Ok(SdkTracerProvider::builder()
        .with_span_processor(processor)
        .with_resource(resource(service_name, service_version))
        .build())
}

fn resource(service_name: &'static str, service_version: &'static str) -> Resource {
    let mut builder =
        Resource::builder().with_attribute(KeyValue::new("service.version", service_version));
    if std::env::var_os("OTEL_SERVICE_NAME").is_none() {
        builder = builder.with_service_name(service_name);
    }
    if let Ok(environment) = std::env::var("DEPLOYMENT_ENVIRONMENT")
        && !environment.is_empty()
    {
        builder = builder.with_attribute(KeyValue::new("deployment.environment", environment));
    }
    builder.build()
}

fn env_filter(default_filter: &str) -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter))
}

/// The log filter, widened so the HTTP server spans this crate emits are
/// exported whatever the log targets are.
fn otel_filter(default_filter: &str) -> EnvFilter {
    env_filter(default_filter)
        .add_directive("cctui_telemetry=info".parse().expect("static directive parses"))
}

#[cfg(test)]
mod tests;
