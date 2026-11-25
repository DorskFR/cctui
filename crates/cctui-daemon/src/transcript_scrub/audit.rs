use std::collections::BTreeMap;

use cctui_crypto::redact::{self, CategorySamples, CompiledPatterns};
use cctui_proto::api::privacy_scan::ScanSample;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{Harness, Report, Target};

const MAX_FINDINGS: usize = 1000;
const FINGERPRINT_HEX: usize = 8;
const MARKER_OPEN: &str = "[REDACTED:";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub category: String,
    pub fingerprint: String,
    pub harness: Harness,
    pub session: String,
    pub cwd: Option<String>,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub occurrences: usize,
}

#[derive(Debug)]
pub struct FileCtx {
    harness: Harness,
    session: String,
    cwd: Option<String>,
    timestamp: Option<String>,
}

impl FileCtx {
    pub fn new(target: &Target) -> Self {
        Self { harness: target.harness, session: target.session(), cwd: None, timestamp: None }
    }

    pub const fn detached() -> Self {
        Self { harness: Harness::ClaudeCode, session: String::new(), cwd: None, timestamp: None }
    }

    /// Codex names the cwd once, in its `session_meta` payload, so it carries
    /// over to the lines after it.
    pub fn observe(&mut self, line: &Value) {
        let cwd = line.get("cwd").or_else(|| line.get("payload").and_then(|p| p.get("cwd")));
        if let Some(cwd) = cwd.and_then(Value::as_str) {
            cwd.clone_into(self.cwd.get_or_insert_with(String::new));
        }
        self.timestamp = line.get("timestamp").and_then(Value::as_str).map(str::to_owned);
    }
}

#[derive(Debug, Default)]
pub struct Audit {
    findings: BTreeMap<(String, String, String), Finding>,
    samples: BTreeMap<String, CategorySamples>,
    truncated: bool,
}

impl Audit {
    pub fn sample(&mut self, value: &Value, patterns: &CompiledPatterns) {
        redact::collect_samples(value, patterns, &mut self.samples);
    }

    pub fn record(&mut self, before: &Value, after: &Value, ctx: &FileCtx) {
        let mut masked = Vec::new();
        masked_leaves(before, after, &mut masked);
        for (category, value) in masked {
            self.note(category, &fingerprint(&value), ctx);
        }
    }

    fn note(&mut self, category: String, fingerprint: &str, ctx: &FileCtx) {
        let key = (category, fingerprint.to_owned(), ctx.session.clone());
        if !self.findings.contains_key(&key) && self.findings.len() >= MAX_FINDINGS {
            self.truncated = true;
            return;
        }
        let ts = ctx.timestamp.clone();
        let entry =
            self.findings.entry(key).or_insert_with_key(|(category, fingerprint, _)| Finding {
                category: category.clone(),
                fingerprint: fingerprint.clone(),
                harness: ctx.harness,
                session: ctx.session.clone(),
                cwd: ctx.cwd.clone(),
                first_seen: ts.clone(),
                last_seen: ts.clone(),
                occurrences: 0,
            });
        entry.occurrences += 1;
        if let Some(ts) = ts {
            if entry.first_seen.as_ref().is_none_or(|f| ts < *f) {
                entry.first_seen = Some(ts.clone());
            }
            if entry.last_seen.as_ref().is_none_or(|l| ts > *l) {
                entry.last_seen = Some(ts);
            }
        }
        if entry.cwd.is_none() {
            entry.cwd.clone_from(&ctx.cwd);
        }
    }

    pub fn into_report(self, report: &mut Report) {
        report.findings = self.findings.into_values().collect();
        report.findings_truncated = self.truncated;
        report.samples = self
            .samples
            .into_iter()
            .map(|(cat, s)| {
                let samples = s
                    .samples
                    .into_iter()
                    .map(|s| ScanSample {
                        text: s.text,
                        context: s.context,
                        value_follows: s.value_follows,
                    })
                    .collect();
                (cat, samples)
            })
            .collect();
    }
}

pub fn fingerprint(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    hex::encode(digest)[..FINGERPRINT_HEX].to_owned()
}

pub fn masked_leaves(before: &Value, after: &Value, out: &mut Vec<(String, String)>) {
    match (before, after) {
        (Value::String(b), Value::String(a)) if a != b => masked_spans(b, a, out),
        (Value::Array(b), Value::Array(a)) => {
            for (b, a) in b.iter().zip(a) {
                masked_leaves(b, a, out);
            }
        }
        (Value::Object(b), Value::Object(a)) => {
            for (k, b) in b {
                if let Some(a) = a.get(k) {
                    masked_leaves(b, a, out);
                }
            }
        }
        _ => {}
    }
}

/// Recover the masked spans of `original` by aligning it with `redacted` on
/// the literal text between placeholders. Adjacent placeholders cannot be told
/// apart and are reported as one span under the first one's category.
pub fn masked_spans(original: &str, redacted: &str, out: &mut Vec<(String, String)>) {
    let (literals, markers) = split_markers(redacted);
    let Some(mut tail) = original.strip_prefix(literals[0]) else { return };
    let mut k = 0;
    while k < markers.len() {
        let mut last = k;
        while last + 1 < markers.len() && literals[last + 1].is_empty() {
            last += 1;
        }
        let next = literals[last + 1];
        let end = if next.is_empty() {
            tail.len()
        } else {
            let Some(end) = tail.find(next) else { return };
            end
        };
        let value = &tail[..end];
        let already_masked = last == k && value == markers[k].0;
        if !value.is_empty() && !already_masked {
            out.push((markers[k].1.to_owned(), value.to_owned()));
        }
        tail = &tail[end + next.len()..];
        k = last + 1;
    }
}

/// `literals` always has one more entry than `markers`.
fn split_markers(s: &str) -> (Vec<&str>, Vec<(&str, &str)>) {
    let mut literals = Vec::new();
    let mut markers = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find(MARKER_OPEN) {
        let Some(len) = rest[start..].find(']') else { break };
        let marker = &rest[start..=start + len];
        let inner = &marker[MARKER_OPEN.len()..marker.len() - 1];
        let category = inner.split(':').next().unwrap_or(inner);
        literals.push(&rest[..start]);
        markers.push((marker, category));
        rest = &rest[start + len + 1..];
    }
    literals.push(rest);
    (literals, markers)
}
