//! What a server understands, advertised in
//! [`DaemonAuthResponse::capabilities`](crate::api::DaemonAuthResponse::capabilities).
//!
//! Serde's internally-tagged enums are closed on the wire: an
//! [`AdapterEvent`](crate::adapter::AdapterEvent) kind a server does not know
//! fails the whole [`DaemonFrameUp`](crate::ws::DaemonFrameUp) parse, which for
//! a batched frame discards its siblings too. So a newer daemon must learn what
//! the server accepts before it sends a new kind. Silence means "old server":
//! send only the kinds that predate this negotiation.

/// [`AdapterEvent::TurnEnd`](crate::adapter::AdapterEvent::TurnEnd).
pub const TURN_END: &str = "turn_end";

/// Everything this build understands, for a server to advertise.
pub const ALL: &[&str] = &[TURN_END];

/// Whether `advertised` contains `capability`.
#[must_use]
pub fn has(advertised: &[String], capability: &str) -> bool {
    advertised.iter().any(|c| c == capability)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_servers_silence_grants_nothing() {
        assert!(!has(&[], TURN_END));
        assert!(!has(&["something_else".to_owned()], TURN_END));
    }

    #[test]
    fn an_advertised_capability_is_found() {
        assert!(has(&[TURN_END.to_owned()], TURN_END));
        let all: Vec<String> = ALL.iter().map(|c| (*c).to_owned()).collect();
        assert!(has(&all, TURN_END));
    }
}
