//! What the connected server advertised at `daemon_auth`.
//!
//! Process-global because the adapters that need it are reached through
//! channels, not a config struct, and it is a property of the single server
//! connection this process has. Empty until auth lands and after a server
//! downgrade re-auths, which is the safe reading: a capability we are unsure of
//! is one we must not use.

use std::sync::RwLock;

static ADVERTISED: RwLock<Vec<String>> = RwLock::new(Vec::new());

pub fn set(capabilities: &[String]) {
    if let Ok(mut slot) = ADVERTISED.write() {
        *slot = capabilities.to_vec();
    }
}

/// Whether the server accepts `capability`. A poisoned lock reads as "no".
#[must_use]
pub fn server_supports(capability: &str) -> bool {
    ADVERTISED.read().is_ok_and(|c| cctui_proto::capability::has(c.as_slice(), capability))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_proto::capability::TURN_END;

    /// One global, so this is deliberately a single test: a second one would
    /// race it.
    #[test]
    fn an_unset_registry_grants_nothing_and_a_downgrade_takes_it_away() {
        assert!(!server_supports(TURN_END));
        set(&[TURN_END.to_owned()]);
        assert!(server_supports(TURN_END));
        assert!(!server_supports("never_advertised"));
        set(&[]);
        assert!(!server_supports(TURN_END), "an older server must lose the capability again");
    }
}
