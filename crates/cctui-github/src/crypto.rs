//! At-rest credential encryption for GitHub connectors.
//!
//! This mirrors `cctui-server`'s `crate::crypto` XOR-vault pattern byte for byte
//! and reads the **same** `CCTUI_VAULT_KEY`, so a connector credential is
//! protected exactly like an OAuth-account refresh token. It is duplicated here
//! (rather than imported) only because the GitHub crate must not depend on
//! `cctui-server` — the server depends on it, so importing would cycle.
//!
//! The plaintext credential is encrypted on create and **never** decrypted by
//! any code in this story: the API masks it from the ciphertext directly. A
//! later webhook/reconcile story is the first consumer of [`deobfuscate`].

use std::fmt::Write;

/// XOR-obfuscate a plaintext with `key`, hex-encoding the result. An empty key
/// (e.g. unset in tests) is a pass-through, matching the server's behaviour.
#[must_use]
pub fn obfuscate(plaintext: &str, key: &[u8]) -> String {
    if key.is_empty() {
        return plaintext.to_string();
    }
    let mut result = String::new();
    for (i, b) in plaintext.bytes().enumerate() {
        let _ = write!(result, "{:02x}", b ^ key[i % key.len()]);
    }
    result
}

/// Inverse of [`obfuscate`]. Returns `None` on malformed (non-even-length or
/// non-hex / non-UTF-8) ciphertext.
#[must_use]
pub fn deobfuscate(ciphertext: &str, key: &[u8]) -> Option<String> {
    if key.is_empty() {
        return Some(ciphertext.to_string());
    }
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

/// The hex-decoded vault key from `CCTUI_VAULT_KEY` (same key the server uses).
/// An unset/empty env var yields an empty key (pass-through) so non-prod builds
/// and tests don't panic; production always sets it.
#[must_use]
pub fn vault_key() -> Vec<u8> {
    let Ok(k) = std::env::var("CCTUI_VAULT_KEY") else {
        return Vec::new();
    };
    let mut bytes = Vec::new();
    for i in (0..k.len()).step_by(2) {
        match u8::from_str_radix(&k[i..(i + 2).min(k.len())], 16) {
            Ok(b) => bytes.push(b),
            Err(_) => return Vec::new(),
        }
    }
    bytes
}

/// A non-secret, recognisable fragment of a credential for display. Keeps a few
/// leading and trailing chars so an operator can tell connectors apart while the
/// bulk of the secret stays hidden — `github_pat_…wxyz`. The full credential is
/// never recoverable from this mask.
#[must_use]
pub fn credential_preview(plaintext: &str) -> String {
    let n = plaintext.chars().count();
    if n <= 8 {
        return "•".repeat(n);
    }
    let head: String = plaintext.chars().take(4).collect();
    let tail: String = plaintext.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obfuscate_roundtrips() {
        let key = b"0123456789abcdef";
        let secret = "github_pat_11ABCDEF_supersecrettoken";
        let enc = obfuscate(secret, key);
        assert_ne!(enc, secret, "ciphertext must differ from plaintext");
        assert_eq!(deobfuscate(&enc, key).as_deref(), Some(secret));
    }

    #[test]
    fn empty_key_is_passthrough() {
        assert_eq!(obfuscate("x", &[]), "x");
        assert_eq!(deobfuscate("x", &[]).as_deref(), Some("x"));
    }

    #[test]
    fn preview_hides_the_secret() {
        let secret = "github_pat_11ABCDEFwxyz";
        let p = credential_preview(secret);
        assert!(p.contains('…'));
        assert!(p.len() < secret.len());
        assert!(!secret.contains(&p));
        // Short secrets fully masked.
        assert_eq!(credential_preview("abcd"), "••••");
    }

    #[test]
    fn deobfuscate_rejects_malformed() {
        let key = b"key";
        assert_eq!(deobfuscate("abc", key), None); // odd length
        assert_eq!(deobfuscate("zz", key), None); // non-hex
    }
}
