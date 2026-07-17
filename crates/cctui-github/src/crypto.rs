pub use cctui_crypto::{decrypt, encrypt, vault_key};

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
    fn encrypt_roundtrips() {
        let key = b"0123456789abcdef0123456789abcdef";
        let secret = "github_pat_11ABCDEF_supersecrettoken";
        let enc = encrypt(secret, key);
        assert_ne!(enc, secret, "ciphertext must differ from plaintext");
        assert!(enc.starts_with("v1:"), "new writes carry the version marker");
        assert_eq!(decrypt(&enc, key).as_deref(), Some(secret));
    }

    #[test]
    fn legacy_xor_values_still_read() {
        let key = b"0123456789abcdef";
        let secret = "github_pat_legacy_token";
        let legacy = cctui_crypto::legacy_xor_obfuscate(secret, key);
        assert_eq!(decrypt(&legacy, key).as_deref(), Some(secret));
    }

    #[test]
    fn empty_key_is_passthrough() {
        assert_eq!(encrypt("x", &[]), "x");
        assert_eq!(decrypt("x", &[]).as_deref(), Some("x"));
    }

    #[test]
    fn preview_hides_the_secret() {
        let secret = "github_pat_11ABCDEFwxyz";
        let p = credential_preview(secret);
        assert!(p.contains('…'));
        assert!(p.len() < secret.len());
        assert!(!secret.contains(&p));
        assert_eq!(credential_preview("abcd"), "••••");
    }

    #[test]
    fn decrypt_rejects_malformed() {
        let key = b"key";
        assert_eq!(decrypt("abc", key), None);
        assert_eq!(decrypt("zz", key), None);
    }
}
