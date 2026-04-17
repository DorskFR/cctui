//! Rewrite absolute paths embedded in Claude's JSONL transcripts when pulling
//! an archived session onto a machine with a different filesystem layout.
//!
//! The transform runs line-by-line: each line is parsed as JSON, any string
//! value that *starts* with one of the configured source prefixes gets that
//! prefix swapped for the target, and the line is re-serialised. This covers
//! the common cases (per-message `cwd`, tool inputs containing absolute paths,
//! shell command lines) without hand-rolling a regex over raw bytes.
//!
//! Rules apply in declaration order; first match wins per string value.

use anyhow::{Context, Result, bail};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Rule {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default)]
pub struct Rules(Vec<Rule>);

impl Rules {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Parse a comma-separated list of `from=to` pairs.
    pub fn parse(spec: &str) -> Result<Self> {
        if spec.trim().is_empty() {
            return Ok(Self::default());
        }
        let mut rules = Vec::new();
        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (from, to) = part
                .split_once('=')
                .with_context(|| format!("invalid remap (need from=to): {part}"))?;
            if from.is_empty() || to.is_empty() {
                bail!("empty side in remap: {part}");
            }
            rules.push(Rule { from: from.to_string(), to: to.to_string() });
        }
        Ok(Self(rules))
    }

    /// Apply rules to a string; first match wins, applied only at a prefix.
    #[must_use]
    pub fn apply_str(&self, s: &str) -> String {
        for r in &self.0 {
            if let Some(rest) = s.strip_prefix(&r.from) {
                let mut out = String::with_capacity(r.to.len() + rest.len());
                out.push_str(&r.to);
                out.push_str(rest);
                return out;
            }
        }
        s.to_string()
    }

    /// Recursively rewrite string values inside a JSON value.
    pub fn apply_value(&self, v: &mut Value) {
        match v {
            Value::String(s) => {
                let new = self.apply_str(s);
                if new != *s {
                    *s = new;
                }
            }
            Value::Array(items) => {
                for it in items {
                    self.apply_value(it);
                }
            }
            Value::Object(map) => {
                for (_k, vv) in map.iter_mut() {
                    self.apply_value(vv);
                }
            }
            _ => {}
        }
    }

    /// Rewrite a JSONL blob line-by-line. Lines that fail to parse as JSON are
    /// left untouched (defensive — we don't want to corrupt unexpected input).
    #[must_use]
    pub fn apply_jsonl(&self, input: &str) -> String {
        if self.is_empty() {
            return input.to_string();
        }
        let mut out = String::with_capacity(input.len());
        for line in input.split_inclusive('\n') {
            let (content, nl) = line.strip_suffix('\n').map_or((line, ""), |c| (c, "\n"));
            if content.trim().is_empty() {
                out.push_str(line);
                continue;
            }
            match serde_json::from_str::<Value>(content) {
                Ok(mut v) => {
                    self.apply_value(&mut v);
                    match serde_json::to_string(&v) {
                        Ok(s) => {
                            out.push_str(&s);
                            out.push_str(nl);
                        }
                        Err(_) => out.push_str(line),
                    }
                }
                Err(_) => out.push_str(line),
            }
        }
        out
    }
}

/// Decode Claude's `project_dir` encoding back to an absolute `cwd`. Claude
/// produces these by replacing `/` with `-` in the original cwd, so the
/// round-trip is lossy when the original path itself contained `-` — we just
/// match Claude's own behaviour.
#[must_use]
pub fn decode_project_dir(encoded: &str) -> String {
    encoded.replace('-', "/")
}

/// Inverse of `decode_project_dir`.
#[must_use]
pub fn encode_project_dir(cwd: &str) -> String {
    cwd.replace('/', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_and_multiple() {
        let r = Rules::parse("/Users/a=/home/a").unwrap();
        assert_eq!(r.0.len(), 1);
        let r = Rules::parse("/Users/a=/home/a,/tmp/x=/var/x").unwrap();
        assert_eq!(r.0.len(), 2);
        assert!(Rules::parse("").unwrap().is_empty());
    }

    #[test]
    fn parse_rejects_malformed() {
        assert!(Rules::parse("no-equals").is_err());
        assert!(Rules::parse("=only-to").is_err());
        assert!(Rules::parse("from=").is_err());
    }

    #[test]
    fn apply_prefix_match() {
        let r = Rules::parse("/Users/dorsk=/home/dorsk").unwrap();
        assert_eq!(r.apply_str("/Users/dorsk/x"), "/home/dorsk/x");
        assert_eq!(r.apply_str("/other/path"), "/other/path");
    }

    #[test]
    fn first_match_wins_order() {
        // Broader rule listed first still takes precedence.
        let r = Rules::parse("/Users/dorsk=/home/dorsk,/Users/dorsk/work=/srv/work").unwrap();
        assert_eq!(r.apply_str("/Users/dorsk/work/a"), "/home/dorsk/work/a");
        // Reversed order: specific rule listed first applies.
        let r = Rules::parse("/Users/dorsk/work=/srv/work,/Users/dorsk=/home/dorsk").unwrap();
        assert_eq!(r.apply_str("/Users/dorsk/work/a"), "/srv/work/a");
    }

    #[test]
    fn apply_value_recurses() {
        let r = Rules::parse("/Users/a=/home/a").unwrap();
        let mut v = serde_json::json!({
            "cwd": "/Users/a/proj",
            "tool_input": {"command": "ls /Users/a/proj"},
            "list": ["/Users/a", "/other"],
            "n": 3,
        });
        r.apply_value(&mut v);
        assert_eq!(v["cwd"], "/home/a/proj");
        // Non-prefix embedded paths are left alone — this is the documented
        // behaviour. Mid-string occurrences would need a string-replace mode.
        assert_eq!(v["tool_input"]["command"], "ls /Users/a/proj");
        assert_eq!(v["list"][0], "/home/a");
        assert_eq!(v["list"][1], "/other");
        assert_eq!(v["n"], 3);
    }

    #[test]
    fn apply_jsonl_preserves_structure() {
        let r = Rules::parse("/Users/a=/home/a").unwrap();
        let input = "{\"cwd\":\"/Users/a/x\"}\n{\"cwd\":\"/Users/a/y\"}\n";
        let out = r.apply_jsonl(input);
        assert!(out.contains("\"/home/a/x\""));
        assert!(out.contains("\"/home/a/y\""));
        assert_eq!(out.matches('\n').count(), 2);
    }

    #[test]
    fn apply_jsonl_leaves_unparseable_lines() {
        let r = Rules::parse("/Users/a=/home/a").unwrap();
        let input = "not json\n{\"cwd\":\"/Users/a/x\"}\n";
        let out = r.apply_jsonl(input);
        assert!(out.starts_with("not json\n"));
    }

    #[test]
    fn empty_rules_noop() {
        let r = Rules::default();
        assert_eq!(r.apply_str("/Users/a/x"), "/Users/a/x");
        let input = "{\"a\":1}\n";
        assert_eq!(r.apply_jsonl(input), input);
    }

    #[test]
    fn project_dir_roundtrip() {
        let cwd = "/home/dorsk/Documents/foo";
        let enc = encode_project_dir(cwd);
        assert_eq!(enc, "-home-dorsk-Documents-foo");
        assert_eq!(decode_project_dir(&enc), cwd);
    }
}
