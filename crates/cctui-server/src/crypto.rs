pub use cctui_crypto::{decrypt, encrypt};

/// The server must never fall back to pass-through: an unset/invalid
/// `CCTUI_VAULT_KEY` is a startup error, not a plaintext vault.
#[must_use]
pub fn vault_key() -> Vec<u8> {
    cctui_crypto::vault_key_required()
}
