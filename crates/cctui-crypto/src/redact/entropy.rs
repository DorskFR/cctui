//! Post-match checks for the run detectors, whose regexes alone would match
//! every long identifier, path and digest.

use super::floor_boundary;

pub const MIN_BASE64_LEN: usize = 40;
pub const MIN_BASE64_ENTROPY: f64 = 4.5;
pub const MIN_BASE64_DIGITS: usize = 2;
pub const MIN_BASE64_CASE: usize = 4;
pub const MIN_HEX_LEN: usize = 48;
pub const MIN_HEX_ENTROPY: f64 = 3.4;
pub const DIGEST_HEX_LENS: [usize; 5] = [40, 56, 64, 96, 128];
/// More `/`, `-` or `_` than one per this many chars reads as a path or identifier.
pub const MAX_SEPARATOR_RATIO: usize = 12;
/// `ABCDEF`, `012345`: an alphabet constant or a fixture, never random data.
pub const MAX_ASCENDING_RUN: usize = 6;
const SRI_PREFIXES: [&str; 4] = ["sha1-", "sha256-", "sha384-", "sha512-"];
const DIGEST_LABELS: [&str; 3] = ["sha", "md5", "blake"];
const DIGEST_LABEL_WINDOW: usize = 12;
/// PEM body lines are at most this wide; only such runs can sit in a PEM block.
const PEM_LINE_MAX: usize = 76;
const PEM_LOOKBACK: usize = 4096;

pub fn shannon_entropy(s: &[u8]) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    for &b in s {
        counts[usize::from(b)] += 1;
    }
    let n = s.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / n;
            -p * p.log2()
        })
        .sum()
}

fn has_ascending_run(s: &[u8]) -> bool {
    let mut run = 1;
    for w in s.windows(2) {
        run = if w[1] == w[0].wrapping_add(1) { run + 1 } else { 1 };
        if run >= MAX_ASCENDING_RUN {
            return true;
        }
    }
    false
}

/// A line of a public PEM block (certificate, public key): a private one is
/// already masked whole by the `private_key` detector before this runs.
fn in_pem_block(hay: &str, start: usize, end: usize) -> bool {
    if end - start > PEM_LINE_MAX {
        return false;
    }
    let window = &hay[floor_boundary(hay, start.saturating_sub(PEM_LOOKBACK))..start];
    window.rfind("-----BEGIN ").is_some_and(|i| !window[i..].contains("-----END "))
}

/// A long, mixed-case, random-looking base64 or base64url run.
pub fn base64_secret(hay: &str, start: usize, end: usize) -> bool {
    let run = hay[start..end].trim_end_matches('=');
    let b = run.as_bytes();
    if b.len() < MIN_BASE64_LEN || SRI_PREFIXES.iter().any(|p| run.starts_with(p)) {
        return false;
    }
    let count = |f: fn(&u8) -> bool| b.iter().filter(|c| f(c)).count();
    let standard = count(|c| matches!(*c, b'+' | b'/')) > 0;
    let url_safe = count(|c| matches!(*c, b'-' | b'_')) > 0;
    if standard && url_safe {
        return false;
    }
    if count(|c| matches!(*c, b'/' | b'-' | b'_')) * MAX_SEPARATOR_RATIO > b.len() {
        return false;
    }
    if b.iter().all(|c| c.is_ascii_hexdigit() || *c == b'-') {
        return false;
    }
    count(u8::is_ascii_digit) >= MIN_BASE64_DIGITS
        && count(u8::is_ascii_uppercase) >= MIN_BASE64_CASE
        && count(u8::is_ascii_lowercase) >= MIN_BASE64_CASE
        && !has_ascending_run(b)
        && shannon_entropy(b) >= MIN_BASE64_ENTROPY
        && !in_pem_block(hay, start, end)
}

/// A long random hex run that is not digest-sized and not labelled as a digest.
pub fn hex_secret(hay: &str, start: usize, end: usize) -> bool {
    let b = hay[start..end].as_bytes();
    if b.len() < MIN_HEX_LEN || DIGEST_HEX_LENS.contains(&b.len()) {
        return false;
    }
    let label = hay[floor_boundary(hay, start.saturating_sub(DIGEST_LABEL_WINDOW))..start]
        .to_ascii_lowercase();
    !DIGEST_LABELS.iter().any(|l| label.contains(l))
        && !has_ascending_run(b)
        && shannon_entropy(b) >= MIN_HEX_ENTROPY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entropy_of_a_constant_is_zero_and_of_uniform_bytes_is_log2_n() {
        assert!(shannon_entropy(b"aaaaaaaa").abs() < f64::EPSILON);
        assert!((shannon_entropy(b"0123456789abcdef") - 4.0).abs() < 1e-9);
    }

    #[test]
    fn ascending_runs_are_detected_only_past_the_threshold() {
        assert!(has_ascending_run(b"xxABCDEFxx"));
        assert!(!has_ascending_run(b"xxABCDExx"));
    }
}
