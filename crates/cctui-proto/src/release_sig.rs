//! Release-asset authenticity: every published binary ships a detached
//! minisign signature (`<asset>.minisig`) made with the release key below.

use minisign_verify::{PublicKey, Signature};

pub const RELEASE_PUBLIC_KEY: &str = include_str!("../../../keys/release.minisign.pub");

pub const SIG_SUFFIX: &str = ".minisig";

/// Verify `bytes` against a `.minisig` document using the embedded release key.
pub fn verify(bytes: &[u8], minisig: &str) -> Result<(), String> {
    verify_with(RELEASE_PUBLIC_KEY, bytes, minisig)
}

pub fn verify_with(public_key: &str, bytes: &[u8], minisig: &str) -> Result<(), String> {
    let pk = PublicKey::decode(public_key.trim()).map_err(|e| format!("release key: {e}"))?;
    let sig = Signature::decode(minisig).map_err(|e| format!("malformed signature: {e}"))?;
    pk.verify(bytes, &sig, false).map_err(|e| format!("signature verification failed: {e}"))
}

/// `true` when `candidate` is strictly older than `running`. Unparseable
/// versions count as older so they are never installed silently.
#[must_use]
pub fn is_downgrade(running: &str, candidate: &str) -> bool {
    match (semver::Version::parse(running), semver::Version::parse(candidate)) {
        (Ok(r), Ok(c)) => c < r,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOREIGN_SIG: &str = "untrusted comment: signature from minisign secret key
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=
trusted comment: timestamp:1633700835\tfile:test\tprehashed
wLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==
";

    #[test]
    fn embedded_key_decodes() {
        PublicKey::decode(RELEASE_PUBLIC_KEY.trim()).expect("release key must parse");
    }

    #[test]
    fn rejects_garbage_and_foreign_signatures() {
        assert!(verify(b"binary", "not a signature").is_err());
        assert!(verify(b"binary", FOREIGN_SIG).is_err());
    }

    #[test]
    fn downgrade_detection() {
        assert!(is_downgrade("0.20.0", "0.19.9"));
        assert!(!is_downgrade("0.20.0", "0.20.0"));
        assert!(!is_downgrade("0.20.0", "0.20.1"));
        assert!(is_downgrade("0.20.0", "garbage"));
    }
}
