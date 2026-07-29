//! At-rest vault encryption for `cctui-server`.
//!
//! Stored format: `v1:<hex(nonce ‖ ciphertext‖tag)>` (ChaCha20-Poly1305,
//! random 12-byte nonce per value). Values without the `v1:` marker are
//! legacy XOR-hex rows and must keep decrypting (lazy migration); legacy
//! values are pure hex, so they can never collide with the prefix.
//!
//! Key: `CCTUI_VAULT_KEY`, hex-encoded 32 bytes. Empty key = pass-through
//! (dev/test, matching the historical scheme); any other length is stretched
//! to 32 via SHA-256 so historical keys keep working.

pub mod redact;

use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use sha2::{Digest, Sha256};
use std::fmt::Write;

const V1_PREFIX: &str = "v1:";
const NONCE_LEN: usize = 12;

fn aead_key(key: &[u8]) -> Key {
    if key.len() == 32 {
        Key::clone_from_slice(key)
    } else {
        Key::clone_from_slice(&Sha256::digest(key))
    }
}

/// AEAD-encrypt `plaintext`, producing a `v1:`-prefixed self-describing
/// string. An empty key is a pass-through.
#[must_use]
pub fn encrypt(plaintext: &str, key: &[u8]) -> String {
    if key.is_empty() {
        return plaintext.to_string();
    }
    let cipher = ChaCha20Poly1305::new(&aead_key(key));
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .expect("ChaCha20-Poly1305 encryption of an in-memory buffer cannot fail");
    let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);
    format!("{V1_PREFIX}{}", hex::encode(blob))
}

/// Decrypt a stored value: `v1:` values via AEAD, anything else legacy XOR.
///
/// Returns `None` — with a logged error, never silent garbage — on malformed
/// input or a failed authentication tag (tampering / wrong key).
#[must_use]
pub fn decrypt(value: &str, key: &[u8]) -> Option<String> {
    if key.is_empty() {
        return Some(value.to_string());
    }
    if let Some(body) = value.strip_prefix(V1_PREFIX) {
        return decrypt_v1(body, key);
    }
    xor_deobfuscate(value, key)
}

fn decrypt_v1(body: &str, key: &[u8]) -> Option<String> {
    match try_decrypt_v1(body, key) {
        Ok(s) => Some(s),
        Err(reason) => {
            tracing::error!("vault decrypt failed: {reason}");
            None
        }
    }
}

fn try_decrypt_v1(body: &str, key: &[u8]) -> Result<String, String> {
    let blob = hex::decode(body).map_err(|e| format!("v1 value is not valid hex: {e}"))?;
    if blob.len() <= NONCE_LEN {
        return Err("v1 value too short".to_string());
    }
    let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
    let plaintext = ChaCha20Poly1305::new(&aead_key(key))
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| {
            "authentication failed (tampered value or wrong CCTUI_VAULT_KEY)".to_string()
        })?;
    String::from_utf8(plaintext).map_err(|e| format!("plaintext is not UTF-8: {e}"))
}

/// Legacy XOR-with-repeating-key + hex writer. Kept only so tests can fabricate
/// rows; production code must use [`encrypt`].
#[must_use]
pub fn legacy_xor_obfuscate(plaintext: &str, key: &[u8]) -> String {
    if key.is_empty() {
        return plaintext.to_string();
    }
    let mut result = String::new();
    for (i, b) in plaintext.bytes().enumerate() {
        let _ = write!(result, "{:02x}", b ^ key[i % key.len()]);
    }
    result
}

fn xor_deobfuscate(ciphertext: &str, key: &[u8]) -> Option<String> {
    if !ciphertext.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::new();
    for i in (0..ciphertext.len()).step_by(2) {
        bytes.push(u8::from_str_radix(&ciphertext[i..i + 2], 16).ok()?);
    }
    let result: Vec<u8> = bytes.iter().enumerate().map(|(i, &b)| b ^ key[i % key.len()]).collect();
    String::from_utf8(result).ok()
}

#[derive(Debug)]
pub enum KeyError {
    Unset,
    InvalidHex(hex::FromHexError),
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unset => f.write_str("CCTUI_VAULT_KEY is not set (hex-encoded 32-byte key)"),
            Self::InvalidHex(e) => write!(f, "CCTUI_VAULT_KEY is not valid hex: {e}"),
        }
    }
}

impl std::error::Error for KeyError {}

fn key_from_env(var: Result<String, std::env::VarError>) -> Result<Vec<u8>, KeyError> {
    let raw = var.map_err(|_| KeyError::Unset)?;
    hex::decode(raw).map_err(KeyError::InvalidHex)
}

/// Like [`vault_key`] but distinguishes unset from invalid hex, so a caller can
/// fail closed instead of silently degrading to pass-through.
pub fn vault_key_checked() -> Result<Vec<u8>, KeyError> {
    key_from_env(std::env::var("CCTUI_VAULT_KEY"))
}

/// The hex-decoded vault key from `CCTUI_VAULT_KEY`.
///
/// Unset yields an empty key (pass-through) so non-prod builds and tests don't
/// panic. Invalid hex also degrades to pass-through but is logged at `error` —
/// prefer [`vault_key_checked`] where storing values UNENCRYPTED is unacceptable.
#[must_use]
pub fn vault_key() -> Vec<u8> {
    match vault_key_checked() {
        Ok(key) => key,
        Err(KeyError::Unset) => Vec::new(),
        Err(KeyError::InvalidHex(e)) => {
            tracing::error!(
                "CCTUI_VAULT_KEY is set but not valid hex ({e}); falling back to pass-through — \
                 vault values will be stored UNENCRYPTED"
            );
            Vec::new()
        }
    }
}

/// Like [`vault_key`] but panics when `CCTUI_VAULT_KEY` is unset or not valid
/// hex — for binaries that must never fall back to pass-through.
#[must_use]
pub fn vault_key_required() -> Vec<u8> {
    match vault_key_checked() {
        Ok(key) => key,
        Err(e) => panic!("{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"test-key-32-bytes-test-key-32byt";

    #[test]
    fn aead_round_trip() {
        let secret = "sk-ant-supersecret-token";
        let enc = encrypt(secret, KEY);
        assert_ne!(enc, secret);
        assert_eq!(decrypt(&enc, KEY).as_deref(), Some(secret));
    }

    #[test]
    fn new_writes_carry_the_version_marker() {
        assert!(encrypt("x", KEY).starts_with("v1:"));
    }

    #[test]
    fn nonce_is_random_per_value() {
        assert_ne!(encrypt("same", KEY), encrypt("same", KEY));
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let enc = encrypt("secret", KEY);
        let (prefix, hex_body) = enc.split_at(V1_PREFIX.len());
        let mut bytes = hex::decode(hex_body).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        let tampered = format!("{prefix}{}", hex::encode(bytes));
        assert_eq!(decrypt(&tampered, KEY), None);
    }

    #[test]
    fn wrong_key_is_rejected() {
        let enc = encrypt("secret", KEY);
        assert_eq!(decrypt(&enc, b"another-32-byte-key-another-32by"), None);
    }

    #[test]
    fn legacy_xor_values_still_read() {
        let legacy = legacy_xor_obfuscate("legacy-refresh-token", KEY);
        assert!(!legacy.starts_with("v1:"));
        assert_eq!(decrypt(&legacy, KEY).as_deref(), Some("legacy-refresh-token"));
    }

    #[test]
    fn non_32_byte_key_round_trips() {
        let key = b"short-key";
        let enc = encrypt("secret", key);
        assert_eq!(decrypt(&enc, key).as_deref(), Some("secret"));
    }

    #[test]
    fn empty_key_is_passthrough() {
        assert_eq!(encrypt("x", &[]), "x");
        assert_eq!(decrypt("x", &[]).as_deref(), Some("x"));
    }

    #[test]
    fn malformed_values_degrade_to_none() {
        assert_eq!(decrypt("abc", KEY), None);
        assert_eq!(decrypt("zz", KEY), None);
        assert_eq!(decrypt("v1:zz", KEY), None);
        assert_eq!(decrypt("v1:00ff", KEY), None);
    }

    #[test]
    fn key_from_env_unset_is_error() {
        assert!(matches!(key_from_env(Err(std::env::VarError::NotPresent)), Err(KeyError::Unset)));
    }

    #[test]
    fn key_from_env_invalid_hex_is_error() {
        assert!(matches!(key_from_env(Ok("nothex-zz".to_owned())), Err(KeyError::InvalidHex(_))));
    }

    #[test]
    fn key_from_env_valid_hex_decodes() {
        assert_eq!(key_from_env(Ok("00ff".to_owned())).unwrap(), vec![0x00, 0xff]);
    }
}
