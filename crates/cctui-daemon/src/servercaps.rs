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

// Deliberately untested: every test of `set` mutates state the whole test
// binary reads, making any test that constructs an adapter order-dependent. The
// lookup itself is `cctui_proto::capability::has`, which is tested; adapters
// take the flag as a parameter so they are tested without this global.
