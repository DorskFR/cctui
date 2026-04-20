//! Small helpers shared by the server, channel, admin, and TUI crates.

use sha2::{Digest, Sha256};

#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Best-effort system hostname: `$HOSTNAME`, then the `hostname(1)` command,
/// falling back to `"unknown"`.
#[must_use]
pub fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Skill-name validation shared by the server's skill store and the admin CLI.
/// Allows ASCII alphanumerics, `-`, `_`, `.`; disallows empty, leading `.`,
/// and names longer than 128 bytes.
#[must_use]
pub fn is_valid_skill_name(s: &str) -> bool {
    if s.is_empty() || s.len() > 128 || s.starts_with('.') {
        return false;
    }
    s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn skill_name_rules() {
        assert!(is_valid_skill_name("foo"));
        assert!(is_valid_skill_name("foo.bar-baz_1"));
        assert!(!is_valid_skill_name(""));
        assert!(!is_valid_skill_name(".hidden"));
        assert!(!is_valid_skill_name("a/b"));
        assert!(!is_valid_skill_name(&"x".repeat(129)));
    }
}
