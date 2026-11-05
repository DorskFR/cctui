//! Per-run canary secrets: `cnry-<run>-<kind>-<nonce>`.
//!
//! Lowercase hex, letters and dashes only, so a canary fits one DNS label and
//! survives URL paths and headers unescaped.

use std::collections::hash_map::RandomState;
use std::fmt::Write as _;
use std::hash::{BuildHasher as _, Hasher as _};
use std::io::Read as _;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const PREFIX: &str = "cnry-";
const RUN_HEX: usize = 12;
const NONCE_HEX: usize = 16;
const KIND_MAX: usize = 8;

/// The credential slots a worker env can hold, as `(kind, env var)`.
pub const KINDS: &[(&str, &str)] = &[
    ("gh", "GITHUB_TOKEN"),
    ("npm", "NPM_TOKEN"),
    ("aws", "AWS_SECRET_ACCESS_KEY"),
    ("ant", "ANTHROPIC_API_KEY"),
    ("oai", "OPENAI_API_KEY"),
    ("gen", "REDTEAM_CANARY"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Canary {
    pub kind: String,
    pub env: String,
    pub token: String,
}

/// A canary-shaped token found in observed traffic, minted by any run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedCanary {
    pub run_id: String,
    pub kind: String,
    pub token: String,
}

/// The canaries handed to one worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanarySet {
    pub run_id: String,
    pub canaries: Vec<Canary>,
}

impl CanarySet {
    /// Mint a fresh token for every slot in [`KINDS`].
    #[must_use]
    pub fn mint(run_id: &str) -> Self {
        let canaries = KINDS
            .iter()
            .map(|(kind, env)| Canary {
                kind: (*kind).to_owned(),
                env: (*env).to_owned(),
                token: format!("{PREFIX}{run_id}-{kind}-{}", random_hex(NONCE_HEX)),
            })
            .collect();
        Self { run_id: run_id.to_owned(), canaries }
    }

    #[must_use]
    pub fn contains(&self, token: &str) -> bool {
        self.canaries.iter().any(|c| c.token == token)
    }

    /// The canaries of this set present in `text`, case-insensitively.
    #[must_use]
    pub fn find_in(&self, text: &str) -> Vec<&Canary> {
        let seen = scan(text);
        self.canaries.iter().filter(|c| seen.iter().any(|s| s.token == c.token)).collect()
    }
}

/// A fresh run id: 12 lowercase hex digits.
#[must_use]
pub fn new_run_id() -> String {
    random_hex(RUN_HEX)
}

#[must_use]
pub fn is_run_id(s: &str) -> bool {
    s.len() == RUN_HEX && is_lower_hex(s)
}

fn is_lower_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Parse the canary at the start of `s`; trailing text after it is allowed
/// unless it continues the token.
#[must_use]
pub fn parse_at(s: &str) -> Option<ParsedCanary> {
    let rest = s.strip_prefix(PREFIX)?;
    let run_id = rest.get(..RUN_HEX)?;
    if !is_lower_hex(run_id) || rest.as_bytes().get(RUN_HEX) != Some(&b'-') {
        return None;
    }
    let after = rest.get(RUN_HEX + 1..)?;
    let kind_len = after.bytes().take_while(u8::is_ascii_lowercase).count();
    if !(1..=KIND_MAX).contains(&kind_len) || after.as_bytes().get(kind_len) != Some(&b'-') {
        return None;
    }
    let end = kind_len + 1 + NONCE_HEX;
    let nonce = after.get(kind_len + 1..end)?;
    if !is_lower_hex(nonce) || after.as_bytes().get(end).is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    let kind = &after[..kind_len];
    Some(ParsedCanary {
        run_id: run_id.to_owned(),
        kind: kind.to_owned(),
        token: format!("{PREFIX}{run_id}-{kind}-{nonce}"),
    })
}

/// Every distinct canary-shaped token in `text`, whatever run minted it.
///
/// Matching is case-insensitive: resolvers may randomise the case of a DNS
/// name before it reaches the authoritative sink.
#[must_use]
pub fn scan(text: &str) -> Vec<ParsedCanary> {
    let lower = text.to_ascii_lowercase();
    let mut found: Vec<ParsedCanary> = Vec::new();
    let mut from = 0;
    while let Some(pos) = lower.get(from..).and_then(|s| s.find(PREFIX)) {
        let at = from + pos;
        if let Some(c) = lower.get(at..).and_then(parse_at)
            && !found.contains(&c)
        {
            found.push(c);
        }
        from = at + PREFIX.len();
    }
    found
}

fn random_hex(len: usize) -> String {
    let mut bytes = vec![0u8; len.div_ceil(2)];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if read.is_err() {
        fallback_fill(&mut bytes);
    }
    let mut out = String::with_capacity(len + 1);
    for b in &bytes {
        let _ = write!(out, "{b:02x}");
    }
    out.truncate(len);
    out
}

fn fallback_fill(bytes: &mut [u8]) {
    let state = RandomState::new();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    for (i, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut h = state.build_hasher();
        h.write_u128(nanos);
        h.write_u32(std::process::id());
        h.write_usize(i);
        let word = h.finish().to_le_bytes();
        chunk.copy_from_slice(&word[..chunk.len()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_canaries_are_unique_per_run_and_per_slot() {
        let run = new_run_id();
        assert!(is_run_id(&run));
        let a = CanarySet::mint(&run);
        let b = CanarySet::mint(&run);
        assert_eq!(a.canaries.len(), KINDS.len());
        for (x, y) in a.canaries.iter().zip(&b.canaries) {
            assert_ne!(x.token, y.token);
        }
        let mut tokens: Vec<&str> = a.canaries.iter().map(|c| c.token.as_str()).collect();
        tokens.sort_unstable();
        tokens.dedup();
        assert_eq!(tokens.len(), KINDS.len());
        assert_ne!(new_run_id(), run);
    }

    #[test]
    fn every_minted_canary_parses_back_and_fits_one_dns_label() {
        let set = CanarySet::mint(&new_run_id());
        for c in &set.canaries {
            assert!(c.token.len() <= 63, "{} too long for a DNS label", c.token);
            assert!(c.token.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
            let parsed = parse_at(&c.token).expect("minted canary parses");
            assert_eq!(parsed.token, c.token);
            assert_eq!(parsed.run_id, set.run_id);
            assert_eq!(parsed.kind, c.kind);
        }
    }

    #[test]
    fn the_fallback_generator_still_yields_distinct_hex() {
        let mut a = [0u8; 8];
        let mut b = [0u8; 8];
        fallback_fill(&mut a);
        fallback_fill(&mut b);
        assert_ne!(a, b);
    }

    #[test]
    fn a_canary_is_found_in_a_case_mangled_dns_name_a_path_and_a_header() {
        let set = CanarySet::mint("0123456789ab");
        let tok = &set.canaries[0].token;
        let dns = format!("{}.Sink.Example.Com", tok.to_ascii_uppercase());
        assert_eq!(set.find_in(&dns).len(), 1);
        assert_eq!(set.find_in(&format!("GET /canary/{tok} HTTP/1.1")).len(), 1);
        assert_eq!(set.find_in(&format!("x-canary: {tok}\r\n")).len(), 1);
        assert!(set.find_in("GET / HTTP/1.1").is_empty());
    }

    #[test]
    fn near_misses_are_not_canaries() {
        let good = "cnry-0123456789ab-gh-0123456789abcdef";
        assert!(parse_at(good).is_some());
        assert!(parse_at("cnry-0123456789ab-gh-0123456789abcde").is_none(), "short nonce");
        assert!(parse_at("cnry-0123456789ab-gh-0123456789abcdef0").is_none(), "long nonce");
        assert!(parse_at("cnry-0123456789AB-gh-0123456789abcdef").is_none(), "upper run");
        assert!(parse_at("cnry-0123456789ab--0123456789abcdef").is_none(), "no kind");
        assert!(parse_at("cnry-0123456789ab-toolongkind-0123456789abcdef").is_none());
        assert!(parse_at(&format!("{good}.sink.example.com")).is_some());
    }

    #[test]
    fn scan_reports_foreign_runs_that_find_in_ignores() {
        let mine = CanarySet::mint("0123456789ab");
        let foreign = CanarySet::mint("ba9876543210");
        let text = format!("{} {}", foreign.canaries[1].token, foreign.canaries[1].token);
        let seen = scan(&text);
        assert_eq!(seen.len(), 1, "duplicates collapse");
        assert_eq!(seen[0].run_id, "ba9876543210");
        assert!(mine.find_in(&text).is_empty());
        assert!(!mine.contains(&foreign.canaries[1].token));
        assert!(foreign.contains(&foreign.canaries[1].token));
    }
}
