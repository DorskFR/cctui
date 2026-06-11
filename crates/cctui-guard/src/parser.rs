//! Markdown parsers: prompt step definitions and the shared guard-rules file.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

/// A single parsed workflow step.
///
/// Mirrors the dict produced by `parse_steps` in the Python daemon: every field
/// holds the raw (unexpanded) string from the prompt markdown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Step {
    pub title: String,
    pub allowed: String,
    pub disallowed: String,
    pub transition: String,
    pub network: String,
}

/// Strip a leading run of `#` characters, then the rest of an ASCII-whitespace
/// prefix, returning the remainder. Returns `None` if there is no `#` prefix or
/// no whitespace after it (i.e. not a heading).
fn heading_body(line: &str) -> Option<&str> {
    let after_hashes = line.trim_start_matches('#');
    let hash_count = line.len() - after_hashes.len();
    if !(1..=6).contains(&hash_count) {
        return None;
    }
    // Require at least one whitespace char between the hashes and the body.
    if !after_hashes.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    Some(after_hashes.trim_start())
}

/// If `line` is a step heading, return `(step_number, title)`.
///
/// Replicates the Python `step_pattern` (`^#{1,6}\s+step\s+(\d+)`) and the title
/// extraction (`[:\s]*` consumed after the number), case-insensitively.
fn parse_step_heading(line: &str) -> Option<(u32, String)> {
    let body = heading_body(line)?;
    // body must start (case-insensitively) with "step" followed by whitespace.
    let lower = body.to_ascii_lowercase();
    let rest = lower.strip_prefix("step")?;
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    // Work on the original-cased body for the title.
    let after_step = &body["step".len()..];
    let after_step = after_step.trim_start();

    // Leading digits = step number.
    let digits_end = after_step
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map_or(after_step.len(), |(i, _)| i);
    if digits_end == 0 {
        return None;
    }
    let num: u32 = after_step[..digits_end].parse().ok()?;

    // Title: everything after the number, with leading ':' and whitespace
    // stripped, then trimmed. Matches Python `[:\s]*(.*)` + `.strip()`.
    let title = after_step[digits_end..]
        .trim_start_matches(|c: char| c == ':' || c.is_whitespace())
        .trim()
        .to_string();
    Some((num, title))
}

/// Parse step definitions from prompt markdown.
///
/// Looks for headings matching `# Step N` (1–6 `#`, case-insensitive) and
/// collects the `[allowed]`/`[disallowed]`/`[transition]`/`[network]` lines that
/// follow, until the next step heading.
#[must_use]
pub fn parse_steps(markdown: &str) -> BTreeMap<u32, Step> {
    let mut steps: BTreeMap<u32, Step> = BTreeMap::new();
    let mut current: Option<u32> = None;

    for line in markdown.split('\n') {
        let stripped = line.trim();

        if let Some((num, title)) = parse_step_heading(stripped) {
            current = Some(num);
            steps.insert(num, Step { title, ..Step::default() });
            continue;
        }

        let Some(cur) = current else { continue };

        let lower = stripped.to_ascii_lowercase();
        let value =
            || stripped.split_once(':').map_or(String::new(), |(_, v)| v.trim().to_string());

        if let Some(step) = steps.get_mut(&cur) {
            if lower.starts_with("[allowed]") {
                step.allowed = value();
            } else if lower.starts_with("[disallowed]") {
                step.disallowed = value();
            } else if lower.starts_with("[transition]") {
                step.transition = value();
            } else if lower.starts_with("[network]") {
                step.network = value();
            }
        }
    }

    steps
}

/// Parse a transition string into `(step_numbers, allows_exit)`.
///
/// `"2, Exit"` → `([2], true)`, `"Step 9, Step 11"` → `([9, 11], false)`,
/// `"Exit"` → `([], true)`.
#[must_use]
pub fn parse_transitions(transition: &str) -> (Vec<u32>, bool) {
    let has_exit = transition.to_ascii_lowercase().contains("exit");
    let numbers = extract_numbers(transition);
    (numbers, has_exit)
}

/// Extract every run of ASCII digits as a `u32`, in order (Python `\d+`).
fn extract_numbers(s: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            cur.push(c);
        } else if !cur.is_empty() {
            if let Ok(n) = cur.parse() {
                out.push(n);
            }
            cur.clear();
        }
    }
    if !cur.is_empty()
        && let Ok(n) = cur.parse()
    {
        out.push(n);
    }
    out
}

/// Parse a comma-separated rule string into keywords, expanding tool-set
/// references recursively.
///
/// Returns `["*"]` for the wildcard, `[]` for empty, otherwise the trimmed
/// keywords with any set names expanded to their members.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn parse_keywords(rule: &str, tool_sets: &HashMap<String, Vec<String>>) -> Vec<String> {
    let stripped = rule.trim();
    if stripped.is_empty() {
        return Vec::new();
    }
    if stripped == "*" {
        return vec!["*".to_string()];
    }

    let mut result = Vec::new();
    for kw in stripped.split(',') {
        let kw = kw.trim();
        if kw.is_empty() {
            continue;
        }
        let mut seen = HashSet::new();
        expand_set(kw, tool_sets, &mut seen, &mut result);
    }
    result
}

/// Recursively expand a tool-set name into its members. Pushes `[name]` if it is
/// not a known set. Circular references are broken via the `seen` set.
pub(crate) fn expand_set(
    name: &str,
    tool_sets: &HashMap<String, Vec<String>>,
    seen: &mut HashSet<String>,
    out: &mut Vec<String>,
) {
    match tool_sets.get(name) {
        Some(members) if !seen.contains(name) => {
            seen.insert(name.to_string());
            for member in members {
                expand_set(member, tool_sets, seen, out);
            }
        }
        _ => out.push(name.to_string()),
    }
}

/// Parse a guard-rules file into a map of set name → member keywords.
///
/// Skips blank lines and `#` comments. A definition is `[name]: a, b, c` where
/// `name` matches `[a-zA-Z0-9_-]+`.
pub fn parse_guard_rules(path: impl AsRef<Path>) -> std::io::Result<HashMap<String, Vec<String>>> {
    let text = std::fs::read_to_string(path)?;
    Ok(parse_guard_rules_str(&text))
}

/// Parse guard-rules content from a string (see [`parse_guard_rules`]).
#[must_use]
pub fn parse_guard_rules_str(text: &str) -> HashMap<String, Vec<String>> {
    let mut tool_sets: HashMap<String, Vec<String>> = HashMap::new();

    for line in text.lines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        if let Some((name, members)) = parse_set_definition(stripped) {
            tool_sets.insert(name, members);
        }
    }

    tool_sets
}

/// Parse `[name]: a, b, c` → `(name, [a, b, c])`, mirroring the Python
/// `^\[([a-zA-Z0-9_-]+)\]\s*:\s*(.*)` regex.
fn parse_set_definition(line: &str) -> Option<(String, Vec<String>)> {
    let rest = line.strip_prefix('[')?;
    let close = rest.find(']')?;
    let name = &rest[..close];
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return None;
    }
    // After ']' must come optional whitespace then ':'.
    let after = rest[close + 1..].trim_start();
    let members_str = after.strip_prefix(':')?;
    let members = members_str
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    Some((name.to_string(), members))
}
