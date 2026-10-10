//! Compiled-in adapter registry.

pub mod acp;
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

/// A turn another agent sent through cctui (direct, room or cctuiverse). It must
/// never answer, decline or dismiss a form the human has open.
#[must_use]
pub fn is_peer_envelope(text: &str) -> bool {
    let head = text.trim_start().get(..32).unwrap_or(text.trim_start()).to_ascii_lowercase();
    ["<cross-session-message", "<cctui-room", "<cctuiverse-"]
        .iter()
        .any(|t| head.starts_with(t))
}

#[must_use]
pub fn registry() -> Vec<Box<dyn AdapterFactory>> {
    let mut factories: Vec<Box<dyn AdapterFactory>> = vec![
        Box::new(claude_code::ClaudeCodeFactory),
        Box::new(codex::CodexFactory),
        Box::new(opencode::OpenCodeFactory),
    ];
    factories.extend(acp::factories());
    factories
}

#[cfg(test)]
mod peer_envelope_tests {
    use super::is_peer_envelope;

    #[test]
    fn peer_envelopes_are_recognised_and_human_text_is_not() {
        for t in [
            "<cross-session-message from=\"s1\" from-name=\"a\">\nhi\n</cross-session-message>",
            "  <CCTUI-ROOM name=\"ops\" from=\"a\">\nhi\n</cctui-room>",
            "<cctuiverse-linked peer=\"bob\" id=\"remote:x\">\n…\n</cctuiverse-linked>",
        ] {
            assert!(is_peer_envelope(t), "{t}");
        }
        for t in ["yes", "option 2", "see <cross-session-message> below", "", "<cctuiverse"] {
            assert!(!is_peer_envelope(t), "{t}");
        }
    }
}
