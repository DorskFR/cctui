//! Shared cctui wrapper validation and neutralization.
pub use cctui_server_core::envelope_guard::*;

#[cfg(test)]
mod tests {
    #[test]
    fn remote_envelope_is_classified_as_a_peer_turn() {
        let text = super::remote_envelope(uuid::Uuid::nil(), "fixture", "nonce", "hello");
        let stored = serde_json::json!({"type":"text", "content": format!("▷ User: {text}")});
        assert_eq!(crate::normalize::client_category(&stored).0, "peer");
    }
}
