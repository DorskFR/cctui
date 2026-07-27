//! Markdown parsers: prompt step definitions and the shared guard-rules file.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

/// Maximum number of `[llmjudge]` questions per step.
///
/// BINEVAL questions are atomic yes/no decompositions of the acceptance
/// conditions; past a dozen the block stops being a gate and starts being a
/// rubric, so parsing fails loudly rather than silently truncating.
pub const MAX_JUDGE_QUESTIONS: usize = 12;

/// A single binary acceptance question in a step's `[llmjudge]` block.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct JudgeQuestion {
    /// The atomic yes/no question. Must be answerable 1 (verifiably yes) or 0.
    pub question: String,
    /// Optional violation example (`question :: violation example`) anchoring
    /// what a 0 looks like.
    pub violation: String,
}

/// A prompt-markdown parse error (malformed `[llmjudge]` block, …). Carries the
/// step number the error was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub step: u32,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Step {}: {}", self.step, self.message)
    }
}

impl std::error::Error for ParseError {}

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
    /// The authoritative prose instructions of the step (every line after the
    /// heading that is not a `[...]` annotation), trimmed. Re-injected verbatim
    /// on transition and on the `SessionStart`/compact hook so a long or diluted
    /// session re-anchors on the trusted next-step prompt rather than its own
    /// drifting summary (CCT-440).
    pub body: String,
    /// Optional `[gate]: <shell command>` — a deterministic completion check the
    /// guard runs (in its `--gate-cwd`) before allowing the transition *out* of
    /// this step. Non-zero exit refuses the transition. Empty ⇒ no gate (the
    /// transition is trusted, as before). This is how finalize-type transitions
    /// require machine-checkable proof instead of the agent's assertion (CCT-440).
    pub gate: String,
    /// Opt-in `[compact]` marker. When set, the step's re-injection text also
    /// carries a "compact your working context" directive; when unset (the
    /// default) re-injection only re-anchors on the authoritative step body and
    /// leaves the session's accumulated context alone. Compaction is lossy and
    /// counter-productive on large-context models, so it is off unless a step
    /// explicitly asks for it (CCT-450). Bare `[compact]` ⇒ on; `[compact]: false`
    /// (or `no`/`off`/`0`) ⇒ off; `[compact]: true` ⇒ on.
    pub compact: bool,
    /// Optional `[llmjudge]` block (CCT-516): binary acceptance questions the
    /// judge must all answer 1 before the transition *out* of this step is
    /// allowed. Runs after the deterministic `[gate]`, judges the semantic
    /// acceptance conditions in a clean context, and fails closed — a partial
    /// score, a malformed verdict, or a missing judge command all refuse the
    /// transition. Empty ⇒ no judge.
    pub llmjudge: Vec<JudgeQuestion>,
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

struct Fence {
    ch: char,
    len: usize,
    /// Info string (fence language) — surfaced so a future opt-in can special-case
    /// a specific language (CCT-619) while every other fence stays inert prose.
    info: String,
}

/// If `stripped` (whitespace-trimmed) is a code-fence delimiter, return its
/// [`Fence`]: a run of at least three `` ` `` or `~`. For backtick fences the
/// info string may not contain a backtick (`CommonMark`).
fn fence_marker(stripped: &str) -> Option<Fence> {
    let ch = stripped.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = stripped.chars().take_while(|c| *c == ch).count();
    if len < 3 {
        return None;
    }
    let info = stripped[len..].trim().to_string();
    if ch == '`' && info.contains('`') {
        return None;
    }
    Some(Fence { ch, len, info })
}

/// Parse one `[llmjudge]` list item (the text after the leading `-`) into a
/// [`JudgeQuestion`]: `question` or `question :: violation example`. An empty
/// question is a parse error.
fn parse_judge_question(item: &str, step: u32) -> Result<JudgeQuestion, ParseError> {
    let item = item.trim();
    if item.is_empty() {
        return Err(ParseError { step, message: "[llmjudge] question line is empty".to_string() });
    }
    let (question, violation) =
        item.split_once("::").map_or((item, ""), |(q, v)| (q.trim_end(), v.trim_start()));
    if question.is_empty() {
        return Err(ParseError { step, message: "[llmjudge] question text is empty".to_string() });
    }
    Ok(JudgeQuestion { question: question.to_string(), violation: violation.to_string() })
}

/// A bare `[llmjudge]` block with no `- <question>` line is malformed.
fn close_judge(cur: u32, steps: &BTreeMap<u32, Step>) -> Result<(), ParseError> {
    if steps.get(&cur).is_some_and(|s| s.llmjudge.is_empty()) {
        return Err(ParseError {
            step: cur,
            message: "[llmjudge] must be immediately followed by at least one `- <question>` line"
                .to_string(),
        });
    }
    Ok(())
}

/// Advance code-fence state for one line, treating fenced content as prose body.
/// Returns `true` if the line was consumed (the caller skips policy parsing).
fn absorb_fence(
    stripped: &str,
    fence: &mut Option<Fence>,
    current: Option<u32>,
    bodies: &mut BTreeMap<u32, Vec<String>>,
    judge_open: &mut bool,
    steps: &BTreeMap<u32, Step>,
) -> Result<bool, ParseError> {
    let push = |bodies: &mut BTreeMap<u32, Vec<String>>| {
        if let Some(cur) = current
            && let Some(body) = bodies.get_mut(&cur)
        {
            body.push(stripped.to_string());
        }
    };
    if let Some(open) = fence {
        let closes = fence_marker(stripped)
            .is_some_and(|f| f.ch == open.ch && f.len >= open.len && f.info.is_empty());
        if closes {
            *fence = None;
        } else {
            push(bodies);
        }
        return Ok(true);
    }
    if let Some(f) = fence_marker(stripped) {
        if *judge_open && let Some(cur) = current {
            close_judge(cur, steps)?;
            *judge_open = false;
        }
        push(bodies);
        *fence = Some(f);
        return Ok(true);
    }
    Ok(false)
}

/// Parse step definitions from prompt markdown.
///
/// Looks for headings matching `# Step N` (1–6 `#`, case-insensitive) and
/// collects the `[allowed]`/`[disallowed]`/`[transition]`/`[network]`/`[gate]`/
/// `[compact]`/`[llmjudge]` lines that follow, until the next step heading.
///
/// A `[llmjudge]` block is the bare annotation immediately followed by one
/// `- question` line per binary acceptance question (optionally
/// `- question :: violation example`). A malformed block — inline value, no
/// questions, an empty question, a duplicate block, or more than
/// [`MAX_JUDGE_QUESTIONS`] questions — is a parse error (CCT-516).
pub fn parse_steps(markdown: &str) -> Result<BTreeMap<u32, Step>, ParseError> {
    let mut steps: BTreeMap<u32, Step> = BTreeMap::new();
    // Accumulate each step's prose body lines separately; joined + trimmed once
    // the step is closed (next heading or end of input).
    let mut bodies: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    let mut current: Option<u32> = None;
    // Steps whose `[llmjudge]` block has been seen (duplicate detection).
    let mut judged: HashSet<u32> = HashSet::new();
    // A `[llmjudge]` block is open and collecting `- question` lines.
    let mut judge_open = false;
    let mut fence: Option<Fence> = None;

    for line in markdown.split('\n') {
        let stripped = line.trim();

        if absorb_fence(stripped, &mut fence, current, &mut bodies, &mut judge_open, &steps)? {
            continue;
        }

        if let Some((num, title)) = parse_step_heading(stripped) {
            if judge_open && let Some(cur) = current {
                close_judge(cur, &steps)?;
            }
            judge_open = false;
            current = Some(num);
            steps.insert(num, Step { title, ..Step::default() });
            bodies.insert(num, Vec::new());
            continue;
        }

        let Some(cur) = current else { continue };

        let lower = stripped.to_ascii_lowercase();
        let value =
            || stripped.split_once(':').map_or(String::new(), |(_, v)| v.trim().to_string());

        if judge_open {
            if let Some(item) = stripped.strip_prefix('-') {
                let question = parse_judge_question(item, cur)?;
                if let Some(step) = steps.get_mut(&cur) {
                    if step.llmjudge.len() >= MAX_JUDGE_QUESTIONS {
                        return Err(ParseError {
                            step: cur,
                            message: format!(
                                "[llmjudge] has more than {MAX_JUDGE_QUESTIONS} questions — \
                                 decompose into fewer, more atomic conditions"
                            ),
                        });
                    }
                    step.llmjudge.push(question);
                }
                continue;
            }
            // Any non-list line closes the block; an empty block is malformed.
            close_judge(cur, &steps)?;
            judge_open = false;
        }

        if let Some(step) = steps.get_mut(&cur) {
            if lower.starts_with("[llmjudge]") {
                if !judged.insert(cur) {
                    return Err(ParseError {
                        step: cur,
                        message: "duplicate [llmjudge] block".to_string(),
                    });
                }
                if !value().is_empty() {
                    return Err(ParseError {
                        step: cur,
                        message: "[llmjudge] takes no inline value — list one `- <question>` \
                                  (optionally `- <question> :: <violation example>`) per line \
                                  below it"
                            .to_string(),
                    });
                }
                judge_open = true;
            } else if lower.starts_with("[allowed]") {
                step.allowed = value();
            } else if lower.starts_with("[disallowed]") {
                step.disallowed = value();
            } else if lower.starts_with("[transition]") {
                step.transition = value();
            } else if lower.starts_with("[network]") {
                step.network = value();
            } else if lower.starts_with("[gate]") {
                step.gate = value();
            } else if lower.starts_with("[compact]") {
                // Bare `[compact]` (no value) opts in; an explicit value lets a
                // prompt template toggle it off without deleting the line.
                let v = value();
                step.compact = v.is_empty()
                    || matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "on" | "1");
            } else if let Some(body) = bodies.get_mut(&cur) {
                // Any non-annotation line is part of the prose body.
                body.push(stripped.to_string());
            }
        }
    }

    if judge_open && let Some(cur) = current {
        close_judge(cur, &steps)?;
    }

    for (num, lines) in bodies {
        if let Some(step) = steps.get_mut(&num) {
            step.body = lines.join("\n").trim().to_string();
        }
    }

    Ok(steps)
}

/// Every `# Step N` heading number in document order, duplicates preserved.
///
/// Code fences are skipped. [`parse_steps`] keys steps by number and silently
/// collapses a repeated `# Step N`; the linter needs the raw sequence to flag
/// that overwrite.
#[must_use]
pub fn step_heading_numbers(markdown: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut fence: Option<Fence> = None;
    for line in markdown.split('\n') {
        let stripped = line.trim();
        if let Some(open) = &fence {
            let closes = fence_marker(stripped)
                .is_some_and(|f| f.ch == open.ch && f.len >= open.len && f.info.is_empty());
            if closes {
                fence = None;
            }
            continue;
        }
        if let Some(f) = fence_marker(stripped) {
            fence = Some(f);
            continue;
        }
        if let Some((num, _)) = parse_step_heading(stripped) {
            out.push(num);
        }
    }
    out
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

/// Parse a layered stack of guard-rules files into one merged map.
///
/// Files are applied **in order**, so an operator-supplied base can come first
/// and a context pack's `guard-rules.md` second: the pack reuses the base's
/// definitions, **overrides** a set with `[name]: …` (replace), or **extends**
/// one with `[name]+: …` (append). Missing files are skipped (the pack need not
/// ship every layer). See [`parse_guard_rules_into`] for the merge semantics.
pub fn parse_guard_rules_files<P: AsRef<Path>>(
    paths: &[P],
) -> std::io::Result<HashMap<String, Vec<String>>> {
    let mut tool_sets: HashMap<String, Vec<String>> = HashMap::new();
    for path in paths {
        if !path.as_ref().exists() {
            continue;
        }
        let text = std::fs::read_to_string(path)?;
        parse_guard_rules_into(&text, &mut tool_sets);
    }
    Ok(tool_sets)
}

/// Parse guard-rules content from a string (see [`parse_guard_rules`]).
#[must_use]
pub fn parse_guard_rules_str(text: &str) -> HashMap<String, Vec<String>> {
    let mut tool_sets: HashMap<String, Vec<String>> = HashMap::new();
    parse_guard_rules_into(text, &mut tool_sets);
    tool_sets
}

/// Apply one guard-rules document onto an existing set map (the layering core).
///
/// - `[name]: a, b` **replaces** `name` (last definition wins — override).
/// - `[name]+: a, b` **appends** to whatever `name` already holds (extend); if
///   `name` is unknown it is created, so `+` is safe even with no base layer.
///
/// Loading a base layer then a pack layer into the same map therefore lets the
/// pack reuse, extend, or overwrite the base set-by-set.
#[allow(clippy::implicit_hasher)]
pub fn parse_guard_rules_into(text: &str, tool_sets: &mut HashMap<String, Vec<String>>) {
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        if let Some((name, members, extend)) = parse_set_definition(stripped) {
            if extend {
                tool_sets.entry(name).or_default().extend(members);
            } else {
                tool_sets.insert(name, members);
            }
        }
    }
}

/// Parse `[name]: a, b, c` → `(name, [a, b, c], false)` (replace) or
/// `[name]+: a, b, c` → `(name, [a, b, c], true)` (extend/append). Mirrors the
/// Python `^\[([a-zA-Z0-9_-]+)\]\s*:\s*(.*)` regex, with an optional `+` before
/// the colon marking an extend.
fn parse_set_definition(line: &str) -> Option<(String, Vec<String>, bool)> {
    let rest = line.strip_prefix('[')?;
    let close = rest.find(']')?;
    let name = &rest[..close];
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return None;
    }
    // After ']' must come optional whitespace, an optional `+` (extend), optional
    // whitespace, then ':'.
    let after = rest[close + 1..].trim_start();
    let (after, extend) =
        after.strip_prefix('+').map_or((after, false), |r| (r.trim_start(), true));
    let members_str = after.strip_prefix(':')?;
    let members = members_str
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    Some((name.to_string(), members, extend))
}

#[cfg(test)]
mod fence_tests {
    use super::*;

    #[test]
    fn fenced_directives_are_body_not_policy() {
        let md = "\
# Step 1: Do it
[allowed]: Read

Example of the format:
```
[allowed]: Bash
[denied]: Write
# Step 2: fake
```
Carry on.
";
        let steps = parse_steps(md).unwrap();
        assert_eq!(steps.len(), 1);
        let s = &steps[&1];
        assert_eq!(s.allowed, "Read");
        assert_eq!(s.disallowed, "");
        assert!(s.body.contains("[allowed]: Bash"));
        assert!(s.body.contains("# Step 2: fake"));
    }

    #[test]
    fn unfenced_directives_still_parse() {
        let md = "\
# Step 1
[allowed]: Read
[disallowed]: Write
";
        let steps = parse_steps(md).unwrap();
        assert_eq!(steps[&1].allowed, "Read");
        assert_eq!(steps[&1].disallowed, "Write");
    }

    #[test]
    fn tilde_and_info_string_fences() {
        let md = "\
# Step 1
[allowed]: Read
~~~text
[disallowed]: Write
~~~
```yaml
[transition]: 2
```
";
        let steps = parse_steps(md).unwrap();
        assert_eq!(steps[&1].allowed, "Read");
        assert_eq!(steps[&1].disallowed, "");
        assert_eq!(steps[&1].transition, "");
    }

    #[test]
    fn indented_fence_is_recognized() {
        let md = "\
# Step 1
[allowed]: Read
   ```
   [disallowed]: Write
   ```
";
        let steps = parse_steps(md).unwrap();
        assert_eq!(steps[&1].allowed, "Read");
        assert_eq!(steps[&1].disallowed, "");
    }

    #[test]
    fn backtick_fence_not_closed_by_tilde() {
        let md = "\
# Step 1
```
[disallowed]: Write
~~~
[transition]: 2
```
";
        let steps = parse_steps(md).unwrap();
        assert_eq!(steps[&1].disallowed, "");
        assert_eq!(steps[&1].transition, "");
    }

    #[test]
    fn unclosed_fence_at_eof_is_prose() {
        let md = "\
# Step 1
[allowed]: Read
```
[disallowed]: Write
[transition]: 2
";
        let steps = parse_steps(md).unwrap();
        assert_eq!(steps[&1].allowed, "Read");
        assert_eq!(steps[&1].disallowed, "");
        assert_eq!(steps[&1].transition, "");
        assert!(steps[&1].body.contains("[disallowed]: Write"));
    }

    #[test]
    fn longer_close_fence_closes_shorter_open() {
        let md = "\
# Step 1
```
[disallowed]: Write
````
[transition]: 2
";
        let steps = parse_steps(md).unwrap();
        assert_eq!(steps[&1].disallowed, "");
        assert_eq!(steps[&1].transition, "2");
    }
}

#[cfg(test)]
mod layering_tests {
    use super::*;

    #[test]
    fn base_then_pack_override_and_extend() {
        let base = "\
# operator base
[net-dev]: npm.example:443, crates.example:443
[code-read]: Read, Grep
[net-model]: api.base:443
";
        // (inline `#` comments are not stripped by the parser — full-line only)
        let pack = "\
# pack layer
[net-dev]+: extra.example:443
[net-model]: api.pack:443
[net-pack]: only.pack:443
";
        let mut sets: HashMap<String, Vec<String>> = HashMap::new();
        parse_guard_rules_into(base, &mut sets);
        parse_guard_rules_into(pack, &mut sets);

        // extend appends, preserving base members
        assert_eq!(
            sets.get("net-dev").unwrap(),
            &vec![
                "npm.example:443".to_string(),
                "crates.example:443".to_string(),
                "extra.example:443".to_string(),
            ]
        );
        // override replaces wholesale
        assert_eq!(sets.get("net-model").unwrap(), &vec!["api.pack:443".to_string()]);
        // untouched base set survives
        assert_eq!(sets.get("code-read").unwrap(), &vec!["Read".to_string(), "Grep".to_string()]);
        // new set added
        assert_eq!(sets.get("net-pack").unwrap(), &vec!["only.pack:443".to_string()]);
    }

    #[test]
    fn extend_with_no_base_creates_set() {
        let mut sets: HashMap<String, Vec<String>> = HashMap::new();
        parse_guard_rules_into("[fresh]+: a, b\n", &mut sets);
        assert_eq!(sets.get("fresh").unwrap(), &vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn plain_definition_is_not_extend() {
        assert_eq!(
            parse_set_definition("[x]: a, b"),
            Some(("x".to_string(), vec!["a".to_string(), "b".to_string()], false))
        );
        assert_eq!(
            parse_set_definition("[x]+: a"),
            Some(("x".to_string(), vec!["a".to_string()], true))
        );
        assert_eq!(
            parse_set_definition("[x] + : a"),
            Some(("x".to_string(), vec!["a".to_string()], true))
        );
    }
}
