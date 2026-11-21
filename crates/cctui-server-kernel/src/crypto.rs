use std::sync::OnceLock;

pub use cctui_crypto::{decrypt, encrypt};

static KEY: OnceLock<Vec<u8>> = OnceLock::new();

/// Pin the key resolved at startup (possibly a tolerated legacy key).
pub fn install_vault_key(key: Vec<u8>) {
    let _ = KEY.set(key);
}

/// The startup-resolved key; before [`install_vault_key`] an unset or
/// invalid `CCTUI_VAULT_KEY` panics rather than degrading to pass-through.
#[must_use]
pub fn vault_key() -> Vec<u8> {
    KEY.get().cloned().unwrap_or_else(cctui_crypto::vault_key_required)
}
