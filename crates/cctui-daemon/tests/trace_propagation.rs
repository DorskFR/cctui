//! Its own binary: it installs the global exporter.
#![allow(unsafe_code)]

use cctui_daemon::adapter_runtime::command_span;
use cctui_proto::ws::DaemonFrameDown;

const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const SERVER_SPAN_ID: &str = "00f067aa0ba902b7";

fn trace_id_of(span: &tracing::Span) -> (String, String) {
    let (traceparent, _) = span
        .in_scope(cctui_telemetry::current_traceparent)
        .expect("daemon.command has an otel context");
    let parts: Vec<&str> = traceparent.split('-').collect();
    assert_eq!(parts.len(), 4, "{traceparent}");
    (parts[1].to_owned(), parts[2].to_owned())
}

fn command_frame(trace: &str) -> DaemonFrameDown {
    serde_json::from_str(&format!(
        r#"{{"type":"command","adapter_id":"codex","command":{{"kind":"kill","local_id":"s1"}}{trace}}}"#
    ))
    .expect("command frame parses")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_traced_command_opens_daemon_command_under_the_server_span() {
    // SAFETY: set before any other thread of this binary reads the environment.
    unsafe {
        std::env::set_var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "http://127.0.0.1:9/v1/traces");
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _telemetry = cctui_telemetry::init("cctui-daemon-test", "0", "cctui_daemon=info");
    assert!(cctui_telemetry::enabled());

    let traced = command_frame(&format!(
        r#","trace":{{"traceparent":"00-{TRACE_ID}-{SERVER_SPAN_ID}-01"}}"#
    ));
    let DaemonFrameDown::Command { adapter_id, command, trace } = traced else {
        panic!("expected Command")
    };
    assert!(trace.is_some());
    let span = command_span(&adapter_id, &command, trace.as_ref());
    let (trace_id, span_id) = trace_id_of(&span);
    assert_eq!(trace_id, TRACE_ID);
    assert_ne!(span_id, SERVER_SPAN_ID, "daemon.command is a child, not the server span");

    let DaemonFrameDown::Command { adapter_id, command, trace } = command_frame("") else {
        panic!("expected Command")
    };
    assert!(trace.is_none());
    let root = command_span(&adapter_id, &command, None);
    let (root_trace_id, _) = trace_id_of(&root);
    assert_ne!(root_trace_id, TRACE_ID, "an untraced command starts its own trace");
}
