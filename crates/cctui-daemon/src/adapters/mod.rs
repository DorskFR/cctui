//! Compiled-in adapter registry.

pub mod agent_mcp;
pub mod claude_code;
pub mod codex;
#[cfg(test)]
mod conformance_tests;
pub mod gateway_env;
pub mod opencode;
pub(crate) mod pty_watch;
pub mod ring_view;
pub mod traffic_rings;
pub mod turn_end;
pub mod uploads;
pub mod version_gate;

use cctui_proto::adapter::AdapterEvent;
use tokio::sync::mpsc;

use crate::adapter_runtime::AdapterFactory;

/// Send an event the server is waiting on (a command result, a session end).
///
/// A failure means the supervisor is gone, so it can only be logged — but a
/// silent drop leaves the server's waiter to time out with no trace here.
pub async fn emit(events: &mpsc::Sender<AdapterEvent>, event: AdapterEvent) {
    if let Err(err) = events.send(event).await {
        tracing::warn!(event = ?err.0, "adapter event dropped: the supervisor is gone");
    }
}

#[must_use]
pub fn registry() -> Vec<Box<dyn AdapterFactory>> {
    vec![
        Box::new(claude_code::ClaudeCodeFactory),
        Box::new(codex::CodexFactory),
        Box::new(opencode::OpenCodeFactory),
    ]
}
