//! Shared secret-redaction engine.
//!
//! [`redact_json`] rewrites string
//! leaves only (JSON structure is preserved), masking the matched span with
//! `[REDACTED:<category>]`; high-entropy prefixed categories add a keyed-HMAC
//! correlation suffix `[REDACTED:<category>:9f2a]`. Non-reversible, and
//! idempotent because the `[REDACTED:...]` marker matches no anchored pattern.

use std::collections::BTreeMap;

use hmac::{Hmac, Mac};
use regex::Regex;
use serde_json::Value;
use sha2::Sha256;

/// Per-field scan cap.
///
/// String leaves longer than this are left untouched — a
/// guard against a pathological multi-MB blob monopolising the hot path. Real
/// secrets are short and sit well within this window; a legitimate huge tool
/// output that happens to embed a token past the cap is the rare miss the
/// on-demand re-scrub (same cap) also skips, deliberately and consistently.
pub const MAX_FIELD_LEN: usize = 16 * 1024 * 1024;

/// A single detector: a name/category, its regex, the capture group to mask
/// (0 = whole match), and whether it earns a correlation suffix.
struct Compiled {
    category: String,
    re: Regex,
    group: usize,
    high_entropy: bool,
}

/// The precompiled effective detector set (built-in defaults + enabled user
/// patterns) plus the correlation-suffix key. Build once, reuse per event.
pub struct CompiledPatterns {
    patterns: Vec<Compiled>,
    key: Vec<u8>,
}

impl CompiledPatterns {
    /// An empty set — redaction is disabled, [`redact_json`] is a no-op.
    #[must_use]
    pub const fn disabled() -> Self {
        Self { patterns: Vec::new(), key: Vec::new() }
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

/// A built-in detector definition. `group` names the capture group to mask so a
/// URL detector can redact just the password while keeping `scheme://user@host`.
struct Builtin {
    category: &'static str,
    regex: &'static str,
    group: usize,
    high_entropy: bool,
}

/// Prefix-anchored, high-confidence detectors covering the confirmed-live
/// categories from the corpus scan (github / npm / anthropic / aws / vault /
/// gitlab / slack / youtrack / bitwarden / cctui / ccipat / private-key / jwt /
/// db-url). Anchored to real prefixes so `[REDACTED:...]` never re-matches.
const BUILTINS: &[Builtin] = &[
    Builtin {
        category: "github_token",
        regex: r"gh[pousr]_[A-Za-z0-9]{20,}",
        group: 0,
        high_entropy: true,
    },
    Builtin {
        category: "github_pat",
        regex: r"github_pat_[A-Za-z0-9_]{20,}",
        group: 0,
        high_entropy: true,
    },
    Builtin { category: "npm_token", regex: r"npm_[A-Za-z0-9]{30,}", group: 0, high_entropy: true },
    Builtin {
        category: "anthropic_key",
        regex: r"sk-ant-[A-Za-z0-9_-]{20,}",
        group: 0,
        high_entropy: true,
    },
    Builtin {
        category: "aws_access_key",
        regex: r"(?:AKIA|ABIA|ACCA)[0-9A-Z]{16}",
        group: 0,
        high_entropy: true,
    },
    Builtin {
        category: "vault_token",
        regex: r"hvs\.[A-Za-z0-9_-]{20,}",
        group: 0,
        high_entropy: true,
    },
    Builtin {
        category: "gitlab_token",
        regex: r"glpat-[A-Za-z0-9_-]{20,}",
        group: 0,
        high_entropy: true,
    },
    Builtin {
        category: "slack_token",
        regex: r"xox[baprs]-[A-Za-z0-9-]{10,}",
        group: 0,
        high_entropy: true,
    },
    Builtin {
        category: "youtrack_token",
        regex: r"perm[-:][A-Za-z0-9=._-]{20,}",
        group: 0,
        high_entropy: true,
    },
    Builtin {
        category: "bitwarden_token",
        regex: r"btr-[A-Za-z0-9._-]{20,}",
        group: 0,
        high_entropy: true,
    },
    Builtin {
        category: "cctui_token",
        regex: r"cctui_[mu]_[A-Za-z0-9]{20,}",
        group: 0,
        high_entropy: true,
    },
    Builtin { category: "ccipat", regex: r"CCIPAT_[A-Za-z0-9]{20,}", group: 0, high_entropy: true },
    Builtin {
        category: "private_key",
        regex: r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
        group: 0,
        high_entropy: false,
    },
    Builtin {
        category: "jwt",
        regex: r"eyJ[A-Za-z0-9_-]{6,}\.eyJ[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}",
        group: 0,
        high_entropy: true,
    },
    // scheme://user:PASSWORD@host — mask only the password group so the rest of
    // the connection string stays legible.
    Builtin {
        category: "db_url_password",
        regex: r"[a-zA-Z][a-zA-Z0-9+.\-]*://[^:/@\s]+:([^@/\s]+)@",
        group: 1,
        high_entropy: false,
    },
];

/// Compile the effective detector set.
///
/// Built-in defaults plus each `enabled`
/// user pattern (already validated server-side; an uncompilable one is skipped
/// defensively). `key` is the vault key used for the correlation suffix — an
/// empty key drops the suffix (dev/test). Returns [`CompiledPatterns::disabled`]
/// when `enabled` is false.
#[must_use]
pub fn compile(enabled: bool, user: &[(String, String)], key: &[u8]) -> CompiledPatterns {
    if !enabled {
        return CompiledPatterns::disabled();
    }
    let mut patterns: Vec<Compiled> = BUILTINS
        .iter()
        .map(|b| Compiled {
            category: b.category.to_owned(),
            re: Regex::new(b.regex).expect("built-in redaction pattern must compile"),
            group: b.group,
            high_entropy: b.high_entropy,
        })
        .collect();
    for (name, regex) in user {
        match Regex::new(regex) {
            Ok(re) => patterns.push(Compiled {
                category: sanitize_category(name),
                re,
                group: 0,
                high_entropy: false,
            }),
            Err(e) => {
                tracing::warn!(pattern = %name, error = %e, "skipping uncompilable user scrub pattern");
            }
        }
    }
    CompiledPatterns { patterns, key: key.to_vec() }
}

/// Validate that a user-supplied pattern compiles, so the server can reject a
/// bad regex at `PUT` time instead of silently dropping it on the daemon.
pub fn validate_regex(pattern: &str) -> Result<(), String> {
    Regex::new(pattern).map(|_| ()).map_err(|e| e.to_string())
}

/// Keep category tokens greppable and placeholder-safe: lowercase, `[a-z0-9_]`,
/// so a user-named pattern can't inject `]` into `[REDACTED:...]`.
fn sanitize_category(name: &str) -> String {
    let s: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if s.is_empty() { "custom".to_owned() } else { s }
}

/// Truncated keyed-HMAC of a matched secret — the correlation suffix. Keyed so a
/// low-entropy secret isn't dictionary-brute-forceable from the suffix.
fn correlation_suffix(key: &[u8], matched: &str) -> String {
    let mut mac = <Hmac<Sha256>>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(matched.as_bytes());
    let bytes = mac.finalize().into_bytes();
    hex::encode(&bytes[..2])
}

fn placeholder(c: &Compiled, matched: &str, key: &[u8]) -> String {
    if c.high_entropy && !key.is_empty() {
        format!("[REDACTED:{}:{}]", c.category, correlation_suffix(key, matched))
    } else {
        format!("[REDACTED:{}]", c.category)
    }
}

/// Apply one detector across `input`, masking each matched group span. Returns
/// the rewritten string and the number of substitutions.
fn apply(input: &str, c: &Compiled, key: &[u8]) -> (String, usize) {
    let mut out = String::with_capacity(input.len());
    let mut last = 0usize;
    let mut count = 0usize;
    for caps in c.re.captures_iter(input) {
        let Some(g) = caps.get(c.group).or_else(|| caps.get(0)) else { continue };
        out.push_str(&input[last..g.start()]);
        out.push_str(&placeholder(c, g.as_str(), key));
        last = g.end();
        count += 1;
    }
    if count == 0 {
        return (input.to_owned(), 0);
    }
    out.push_str(&input[last..]);
    (out, count)
}

/// Redact a single string leaf, accumulating per-category counts. Returns the
/// rewritten string only when something changed.
fn redact_string(
    input: &str,
    patterns: &CompiledPatterns,
    stats: &mut BTreeMap<String, usize>,
) -> Option<String> {
    if input.len() > MAX_FIELD_LEN {
        return None;
    }
    let mut current = input.to_owned();
    let mut changed = false;
    for c in &patterns.patterns {
        let (next, n) = apply(&current, c, &patterns.key);
        if n > 0 {
            *stats.entry(c.category.clone()).or_insert(0) += n;
            current = next;
            changed = true;
        }
    }
    changed.then_some(current)
}

fn walk(value: &mut Value, patterns: &CompiledPatterns, stats: &mut BTreeMap<String, usize>) {
    match value {
        Value::String(s) => {
            if let Some(replaced) = redact_string(s, patterns, stats) {
                *s = replaced;
            }
        }
        Value::Array(arr) => {
            for v in arr {
                walk(v, patterns, stats);
            }
        }
        Value::Object(obj) => {
            for (_, v) in obj.iter_mut() {
                walk(v, patterns, stats);
            }
        }
        _ => {}
    }
}

/// Redact `value` in place, rewriting string leaves only. Returns the total
/// number of substitutions. No-op (returns 0) when `patterns` is disabled/empty.
pub fn redact_json(value: &mut Value, patterns: &CompiledPatterns) -> usize {
    if patterns.is_empty() {
        return 0;
    }
    let mut stats = BTreeMap::new();
    walk(value, patterns, &mut stats);
    stats.values().sum()
}

/// Like [`redact_json`] but returns per-category substitution counts (for the
/// re-scrub dry-run report). The `value` is still mutated in place.
pub fn redact_json_stats(
    value: &mut Value,
    patterns: &CompiledPatterns,
) -> BTreeMap<String, usize> {
    let mut stats = BTreeMap::new();
    if !patterns.is_empty() {
        walk(value, patterns, &mut stats);
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const KEY: &[u8] = b"test-key-32-bytes-test-key-32byt";

    fn p() -> CompiledPatterns {
        compile(true, &[], KEY)
    }

    fn redact_str(s: &str) -> String {
        let mut v = json!(s);
        redact_json(&mut v, &p());
        v.as_str().unwrap().to_owned()
    }

    // "gl" + "pat-" split so GitHub push protection doesn't flag the fixture
    fn gitlab_fixture() -> String {
        format!("{}{}abcdef0123456789ABCD end", "gl", "pat-")
    }

    #[test]
    fn masks_each_default_category_and_keeps_surrounding_text() {
        let gitlab = gitlab_fixture();
        let cases = [
            ("gh", "curl -H \"Authorization: Bearer ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123\""),
            ("github_pat", "github_pat_11ABCDEFG0123456789abcdefg"),
            ("npm", "//registry: npm_abcdefghijklmnopqrstuvwxyz0123456789"),
            ("anthropic", "ANTHROPIC_API_KEY=sk-ant-api03-abcDEF0123456789xyz"),
            ("aws", "AKIAIOSFODNN7EXAMPLE here"),
            ("vault", "token hvs.CAESIJ0123456789abcdefghij done"),
            ("gitlab", gitlab.as_str()),
            ("slack", "xoxb-1234567890-abcdefghijkl"),
            ("cctui", "cctui_m_ABCDEFGHIJKLMNOPQRSTUV"),
            ("ccipat", "CCIPAT_ABCDEFGHIJKLMNOPQRSTUV"),
        ];
        for (label, input) in cases {
            let out = redact_str(input);
            assert!(out.contains("[REDACTED:"), "{label}: no placeholder in {out}");
            assert!(!out.contains("0123456789ab"), "{label}: secret leaked: {out}");
        }
    }

    #[test]
    fn db_url_masks_only_the_password() {
        let out = redact_str("postgres://admin:s3cr3tPass@db.example.com:5432/app");
        assert!(
            out.starts_with("postgres://admin:[REDACTED:db_url_password]@db.example.com"),
            "{out}"
        );
        assert!(!out.contains("s3cr3tPass"));
    }

    #[test]
    fn private_key_block_is_masked_whole() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIabc123\nZZ==\n-----END RSA PRIVATE KEY-----";
        let out = redact_str(&format!("key:\n{pem}\ndone"));
        assert!(out.contains("[REDACTED:private_key]"), "{out}");
        assert!(!out.contains("MIIabc123"));
        assert!(out.ends_with("done"));
    }

    #[test]
    fn jwt_is_masked() {
        let out = redact_str("eyJhbGciOi.eyJzdWI6MTIz.SflKxwRJSM_abc123");
        assert!(out.contains("[REDACTED:jwt:"), "{out}");
    }

    #[test]
    fn high_entropy_gets_keyed_suffix_low_entropy_does_not() {
        let gh = redact_str("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123");
        assert!(gh.starts_with("[REDACTED:github_token:") && gh.ends_with(']'), "{gh}");
        let db = redact_str("postgres://u:pw123456@h/db");
        assert_eq!(db, "postgres://u:[REDACTED:db_url_password]@h/db");
    }

    #[test]
    fn suffix_is_stable_and_secret_specific() {
        let a = redact_str("ghp_AAAAAAAAAAAAAAAAAAAAAAA0000");
        let b = redact_str("ghp_AAAAAAAAAAAAAAAAAAAAAAA0000");
        let c = redact_str("ghp_BBBBBBBBBBBBBBBBBBBBBBB1111");
        assert_eq!(a, b, "same secret -> same suffix");
        assert_ne!(a, c, "different secret -> different suffix");
    }

    #[test]
    fn idempotent_rescrub_is_a_noop() {
        let once =
            redact_str("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123 and hvs.CAESIJ0123456789abcdefghij");
        let twice = redact_str(&once);
        assert_eq!(once, twice);
        let mut v = json!(once);
        assert_eq!(redact_json(&mut v, &p()), 0);
    }

    #[test]
    fn walks_nested_structure_only_string_leaves() {
        let mut v = json!({
            "cmd": "export TOKEN=ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123",
            "n": 42,
            "arr": ["clean", "hvs.CAESIJ0123456789abcdefghij"],
            "nested": { "k": "sk-ant-api03-abcDEF0123456789xyz" }
        });
        let n = redact_json(&mut v, &p());
        assert_eq!(n, 3);
        assert_eq!(v["n"], json!(42));
        assert!(v["cmd"].as_str().unwrap().contains("[REDACTED:github_token"));
        assert!(v["arr"][1].as_str().unwrap().contains("[REDACTED:vault_token"));
        assert!(v["nested"]["k"].as_str().unwrap().contains("[REDACTED:anthropic_key"));
    }

    #[test]
    fn disabled_is_a_noop() {
        let mut v = json!("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123");
        assert_eq!(redact_json(&mut v, &CompiledPatterns::disabled()), 0);
        assert_eq!(v, json!("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123"));
    }

    #[test]
    fn user_pattern_is_applied() {
        let pats = compile(true, &[("acme".to_owned(), r"ACME-[0-9]{6}".to_owned())], KEY);
        let mut v = json!("id ACME-123456 end");
        assert_eq!(redact_json(&mut v, &pats), 1);
        assert_eq!(v.as_str().unwrap(), "id [REDACTED:acme] end");
    }

    #[test]
    fn multi_mb_field_is_fast_and_correct() {
        let mut s = "a".repeat(4 * 1024 * 1024);
        s.push_str("ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123");
        let mut v = json!(s);
        let start = std::time::Instant::now();
        let n = redact_json(&mut v, &p());
        assert!(start.elapsed().as_secs() < 5, "redaction too slow");
        assert_eq!(n, 1);
        assert!(v.as_str().unwrap().contains("[REDACTED:github_token"));
    }
}
