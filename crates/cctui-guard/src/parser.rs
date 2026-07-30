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
    /// `max-visits:` from the `guard` fenced block: max times the step may be
    /// entered before a transition into it is denied. `None` ⇒ unbounded.
    pub max_visits: Option<u32>,
    /// Per-target gates from the `guard` block's `transitions:` list, keyed by
    /// target step; each runs only for that target, after the step-level `[gate]`.
    pub transition_gates: BTreeMap<u32, String>,
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

/// Is `info` a `guard`-language fence (opting the block into policy parsing,
/// not prose)? Matches `guard` optionally followed by more words (`guard yaml`).
fn is_guard_fence(info: &str) -> bool {
    info.split_whitespace().next() == Some("guard")
}

/// One `{to: N, gate: <cmd>}` entry of a `guard` block's `transitions:` list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuardTransition {
    pub to: u32,
    pub gate: Option<String>,
}

/// The typed payload of a `guard` fenced block.
///
/// Structure that does not fit the bracket-line syntax — per-target transition
/// gates and the `max-visits` re-entry bound. Compiled into the same IR as the
/// bracket lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuardBlock {
    pub max_visits: Option<u32>,
    pub transitions: Vec<GuardTransition>,
    pub exit: bool,
}

/// Split `s` on `delim`, ignoring delimiters inside quotes or `[]`/`{}` nesting.
fn split_top_level(s: &str, delim: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        if let Some(q) = quote {
            buf.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                buf.push(c);
            }
            '[' | '{' => {
                depth += 1;
                buf.push(c);
            }
            ']' | '}' => {
                depth -= 1;
                buf.push(c);
            }
            _ if c == delim && depth == 0 => {
                out.push(buf.trim().to_string());
                buf.clear();
            }
            _ => buf.push(c),
        }
    }
    if !buf.trim().is_empty() {
        out.push(buf.trim().to_string());
    }
    out
}

/// Strip one layer of matching single/double quotes from a scalar, else return
/// it trimmed.
fn unquote(s: &str) -> String {
    let s = s.trim();
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// Set a `to`/`gate` field on the transition item under construction from a
/// `key: value` pair; `{}` wrappers and multiple comma-separated pairs are
/// accepted so both flow (`{to: 3, gate: x}`) and block items resolve here.
fn guard_item_fields(
    raw: &str,
    to: &mut Option<u32>,
    gate: &mut Option<String>,
    step: u32,
) -> Result<(), ParseError> {
    let inner = raw.trim().trim_start_matches('{').trim_end_matches('}');
    for pair in split_top_level(inner, ',') {
        let (key, value) = pair.split_once(':').ok_or_else(|| ParseError {
            step,
            message: format!("guard block: transition field `{pair}` is not `key: value`"),
        })?;
        match key.trim() {
            "to" => {
                *to = Some(value.trim().parse().map_err(|_| ParseError {
                    step,
                    message: format!("guard block: `to` is not a step number: {}", value.trim()),
                })?);
            }
            "gate" => {
                let g = unquote(value);
                *gate = if g.trim().is_empty() { None } else { Some(g) };
            }
            other => {
                return Err(ParseError {
                    step,
                    message: format!("guard block: unknown transition field `{other}`"),
                });
            }
        }
    }
    Ok(())
}

/// Parse a flow-style `transitions:` value: `[{to: 3, gate: "make test"}, {to: 5}]`.
fn parse_flow_transitions(raw: &str, step: u32) -> Result<Vec<GuardTransition>, ParseError> {
    let s = raw.trim();
    let inner =
        s.strip_prefix('[').and_then(|r| r.strip_suffix(']')).ok_or_else(|| ParseError {
            step,
            message: "guard block: flow `transitions` must be a `[ ... ]` list".to_string(),
        })?;
    let mut out = Vec::new();
    for group in split_top_level(inner, ',') {
        let (mut to, mut gate) = (None, None);
        guard_item_fields(&group, &mut to, &mut gate, step)?;
        let to = to.ok_or_else(|| ParseError {
            step,
            message: "guard block: a transition item is missing `to`".to_string(),
        })?;
        out.push(GuardTransition { to, gate });
    }
    Ok(out)
}

/// Parse a `guard` fenced block into a [`GuardBlock`].
///
/// A restricted YAML subset: top-level `max-visits: N`, `exit: true`, and
/// `transitions:` as either a flow `[{to, gate}, ...]` list or a block list of
/// `- to: N` items with optional `gate:` continuation lines. Unknown keys and
/// malformed values fail loudly.
pub fn parse_guard_block(content: &str, step: u32) -> Result<GuardBlock, ParseError> {
    let lines: Vec<&str> = content.lines().collect();
    let base = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    let mut block = GuardBlock::default();
    let mut in_block_list = false;
    let (mut cur_to, mut cur_gate, mut building) = (None, None, false);

    let flush = |block: &mut GuardBlock,
                 to: &mut Option<u32>,
                 gate: &mut Option<String>,
                 building: &mut bool|
     -> Result<(), ParseError> {
        if *building {
            let n = to.ok_or_else(|| ParseError {
                step,
                message: "guard block: a transition item is missing `to`".to_string(),
            })?;
            block.transitions.push(GuardTransition { to: n, gate: gate.take() });
            *to = None;
            *building = false;
        }
        Ok(())
    };

    for raw in lines {
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        let dedented = if raw.len() >= base { &raw[base..] } else { raw.trim_start() };
        let indent = dedented.len() - dedented.trim_start().len();
        let stripped = dedented.trim();

        if indent == 0 {
            flush(&mut block, &mut cur_to, &mut cur_gate, &mut building)?;
            in_block_list = false;
            let (key, value) = stripped.split_once(':').ok_or_else(|| ParseError {
                step,
                message: format!("guard block: `{stripped}` is not `key: value`"),
            })?;
            let value = value.trim();
            match key.trim() {
                "max-visits" | "max_visits" => {
                    block.max_visits = Some(value.parse().map_err(|_| ParseError {
                        step,
                        message: format!("guard block: max-visits is not a number: {value}"),
                    })?);
                }
                "exit" => {
                    block.exit =
                        matches!(value.to_ascii_lowercase().as_str(), "true" | "yes" | "1");
                }
                "transitions" => {
                    if value.is_empty() {
                        in_block_list = true;
                    } else if value.starts_with('[') {
                        block.transitions = parse_flow_transitions(value, step)?;
                    } else {
                        return Err(ParseError {
                            step,
                            message: "guard block: `transitions` must be a `[...]` flow list or a \
                                      block list of `- to:` items"
                                .to_string(),
                        });
                    }
                }
                other => {
                    return Err(ParseError {
                        step,
                        message: format!("guard block: unknown key `{other}`"),
                    });
                }
            }
        } else if in_block_list {
            if let Some(rest) = stripped.strip_prefix('-') {
                flush(&mut block, &mut cur_to, &mut cur_gate, &mut building)?;
                building = true;
                guard_item_fields(rest, &mut cur_to, &mut cur_gate, step)?;
            } else if building {
                guard_item_fields(stripped, &mut cur_to, &mut cur_gate, step)?;
            } else {
                return Err(ParseError {
                    step,
                    message: format!("guard block: stray line in `transitions`: {stripped}"),
                });
            }
        } else {
            return Err(ParseError {
                step,
                message: format!("guard block: unexpected indented line: {stripped}"),
            });
        }
    }
    flush(&mut block, &mut cur_to, &mut cur_gate, &mut building)?;
    Ok(block)
}

/// Merge a parsed [`GuardBlock`] onto a step: its `to` targets union into the
/// `[transition]` string, its per-target gates and `max-visits` bound are
/// recorded on the step.
fn apply_guard_block(step: &mut Step, block: GuardBlock) {
    if let Some(mv) = block.max_visits {
        step.max_visits = Some(mv);
    }
    let (mut nums, mut exit) = parse_transitions(&step.transition);
    for t in block.transitions {
        if !nums.contains(&t.to) {
            nums.push(t.to);
        }
        if let Some(g) = t.gate {
            step.transition_gates.insert(t.to, g);
        }
    }
    exit |= block.exit;
    let mut parts: Vec<String> = nums.iter().map(u32::to_string).collect();
    if exit {
        parts.push("Exit".to_string());
    }
    step.transition = parts.join(", ");
}

/// A step's `[…]` policy annotation. `[llmjudge]` opens a block instead of
/// setting a field, and so takes no inline value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Annotation {
    Field(Field),
    LlmJudge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Allowed,
    Disallowed,
    Transition,
    Network,
    Gate,
    Compact,
}

impl Annotation {
    fn of(lower: &str) -> Option<Self> {
        const TAGS: &[(&str, Annotation)] = &[
            ("[llmjudge]", Annotation::LlmJudge),
            ("[allowed]", Annotation::Field(Field::Allowed)),
            ("[disallowed]", Annotation::Field(Field::Disallowed)),
            ("[transition]", Annotation::Field(Field::Transition)),
            ("[network]", Annotation::Field(Field::Network)),
            ("[gate]", Annotation::Field(Field::Gate)),
            ("[compact]", Annotation::Field(Field::Compact)),
        ];
        TAGS.iter().find(|(tag, _)| lower.starts_with(tag)).map(|&(_, ann)| ann)
    }
}

fn annotation_value(stripped: &str) -> String {
    stripped.split_once(':').map_or(String::new(), |(_, v)| v.trim().to_string())
}

/// Bare `[compact]` opts in; an explicit value lets a prompt template toggle it
/// off without deleting the line.
fn compact_flag(value: &str) -> bool {
    value.is_empty() || matches!(value.to_ascii_lowercase().as_str(), "true" | "yes" | "on" | "1")
}

/// The [`parse_steps`] line-by-line state: the steps built so far plus the
/// per-step prose/`guard`-block buffers and the code-fence / `[llmjudge]` block
/// cursors.
#[derive(Default)]
struct ParseState {
    steps: BTreeMap<u32, Step>,
    /// Each step's prose body lines; joined + trimmed once parsing ends.
    bodies: BTreeMap<u32, Vec<String>>,
    /// Raw lines of each step's `guard` fenced block, parsed at the end.
    guard_raw: BTreeMap<u32, Vec<String>>,
    current: Option<u32>,
    /// Steps whose `[llmjudge]` block has been seen (duplicate detection).
    judged: HashSet<u32>,
    /// A `[llmjudge]` block is open and collecting `- question` lines.
    judge_open: bool,
    fence: Option<Fence>,
}

impl ParseState {
    fn feed(&mut self, raw: &str) -> Result<(), ParseError> {
        if self.absorb_fence(raw)? {
            return Ok(());
        }
        let stripped = raw.trim();

        if let Some((num, title)) = parse_step_heading(stripped) {
            return self.open_step(num, title);
        }

        let Some(cur) = self.current else { return Ok(()) };

        if self.judge_open {
            if let Some(item) = stripped.strip_prefix('-') {
                return self.push_judge_question(cur, item);
            }
            // Any non-list line closes the block; an empty block is malformed.
            self.close_judge(cur)?;
            self.judge_open = false;
        }

        let lower = stripped.to_ascii_lowercase();
        let Some(ann) = Annotation::of(&lower) else {
            self.push_body(cur, stripped);
            return Ok(());
        };
        self.apply_annotation(cur, ann, stripped)
    }

    /// Advance code-fence state for one line. Ordinary fenced content is prose
    /// body; a `guard`-language fence buffers its raw lines for later
    /// [`parse_guard_block`] and is kept out of the body. `true` ⇒ the line was
    /// consumed and skips policy parsing.
    fn absorb_fence(&mut self, raw: &str) -> Result<bool, ParseError> {
        let stripped = raw.trim();
        if let Some(open) = &self.fence {
            let closes = fence_marker(stripped)
                .is_some_and(|f| f.ch == open.ch && f.len >= open.len && f.info.is_empty());
            let guard = is_guard_fence(&open.info);
            if closes {
                self.fence = None;
            } else if let Some(cur) = self.current {
                if guard {
                    self.guard_raw.entry(cur).or_default().push(raw.to_string());
                } else {
                    self.push_body(cur, stripped);
                }
            }
            return Ok(true);
        }
        if let Some(f) = fence_marker(stripped) {
            if self.judge_open
                && let Some(cur) = self.current
            {
                self.close_judge(cur)?;
                self.judge_open = false;
            }
            if !is_guard_fence(&f.info)
                && let Some(cur) = self.current
            {
                self.push_body(cur, stripped);
            }
            self.fence = Some(f);
            return Ok(true);
        }
        Ok(false)
    }

    fn open_step(&mut self, num: u32, title: String) -> Result<(), ParseError> {
        if self.judge_open
            && let Some(cur) = self.current
        {
            self.close_judge(cur)?;
        }
        self.judge_open = false;
        self.current = Some(num);
        self.steps.insert(num, Step { title, ..Step::default() });
        self.bodies.insert(num, Vec::new());
        Ok(())
    }

    /// Any non-annotation line is part of the prose body.
    fn push_body(&mut self, cur: u32, text: &str) {
        if let Some(body) = self.bodies.get_mut(&cur) {
            body.push(text.to_string());
        }
    }

    fn apply_annotation(
        &mut self,
        cur: u32,
        ann: Annotation,
        stripped: &str,
    ) -> Result<(), ParseError> {
        let value = annotation_value(stripped);
        let field = match ann {
            Annotation::LlmJudge => return self.open_judge(cur, &value),
            Annotation::Field(field) => field,
        };
        let Some(step) = self.steps.get_mut(&cur) else { return Ok(()) };
        match field {
            Field::Allowed => step.allowed = value,
            Field::Disallowed => step.disallowed = value,
            Field::Transition => step.transition = value,
            Field::Network => step.network = value,
            Field::Gate => step.gate = value,
            Field::Compact => step.compact = compact_flag(&value),
        }
        Ok(())
    }

    fn open_judge(&mut self, cur: u32, value: &str) -> Result<(), ParseError> {
        if !self.judged.insert(cur) {
            return Err(ParseError {
                step: cur,
                message: "duplicate [llmjudge] block".to_string(),
            });
        }
        if !value.is_empty() {
            return Err(ParseError {
                step: cur,
                message: "[llmjudge] takes no inline value — list one `- <question>` (optionally \
                          `- <question> :: <violation example>`) per line below it"
                    .to_string(),
            });
        }
        self.judge_open = true;
        Ok(())
    }

    fn push_judge_question(&mut self, cur: u32, item: &str) -> Result<(), ParseError> {
        let question = parse_judge_question(item, cur)?;
        if let Some(step) = self.steps.get_mut(&cur) {
            if step.llmjudge.len() >= MAX_JUDGE_QUESTIONS {
                return Err(ParseError {
                    step: cur,
                    message: format!(
                        "[llmjudge] has more than {MAX_JUDGE_QUESTIONS} questions — decompose \
                         into fewer, more atomic conditions"
                    ),
                });
            }
            step.llmjudge.push(question);
        }
        Ok(())
    }

    /// A bare `[llmjudge]` block with no `- <question>` line is malformed.
    fn close_judge(&self, cur: u32) -> Result<(), ParseError> {
        if self.steps.get(&cur).is_some_and(|s| s.llmjudge.is_empty()) {
            return Err(ParseError {
                step: cur,
                message:
                    "[llmjudge] must be immediately followed by at least one `- <question>` line"
                        .to_string(),
            });
        }
        Ok(())
    }

    fn finish(mut self) -> Result<BTreeMap<u32, Step>, ParseError> {
        if self.judge_open
            && let Some(cur) = self.current
        {
            self.close_judge(cur)?;
        }
        finalize_bodies(&mut self.steps, self.bodies);
        apply_guard_blocks(&mut self.steps, self.guard_raw)?;
        Ok(self.steps)
    }
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
/// [`MAX_JUDGE_QUESTIONS`] questions — is a parse error.
pub fn parse_steps(markdown: &str) -> Result<BTreeMap<u32, Step>, ParseError> {
    let mut state = ParseState::default();
    for line in markdown.split('\n') {
        state.feed(line)?;
    }
    state.finish()
}

/// Join each step's accumulated prose lines into its trimmed `body`.
fn finalize_bodies(steps: &mut BTreeMap<u32, Step>, bodies: BTreeMap<u32, Vec<String>>) {
    for (num, lines) in bodies {
        if let Some(step) = steps.get_mut(&num) {
            step.body = lines.join("\n").trim().to_string();
        }
    }
}

/// Parse each step's buffered `guard` fenced block and merge it onto the step.
fn apply_guard_blocks(
    steps: &mut BTreeMap<u32, Step>,
    guard_raw: BTreeMap<u32, Vec<String>>,
) -> Result<(), ParseError> {
    for (num, lines) in guard_raw {
        let block = parse_guard_block(&lines.join("\n"), num)?;
        if let Some(step) = steps.get_mut(&num) {
            apply_guard_block(step, block);
        }
    }
    Ok(())
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

/// Document-level header names that are directives, not tool-set definitions,
/// and so are excluded when reading inline sets from a prompt.
const RESERVED_HEADERS: &[&str] = &["guard", "network-default", "rules"];

/// Every set definition in a guard-rules document, in authored order, as
/// `(name, members, extend)`. The ordered form the layered set resolver applies
/// (last writer wins) while tracking provenance.
#[must_use]
pub fn rules_definitions(text: &str) -> Vec<(String, Vec<String>, bool)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        if let Some(def) = parse_set_definition(stripped) {
            out.push(def);
        }
    }
    out
}

/// Inline tool-set / network-set definitions authored in the prompt itself.
///
/// Read from the document prelude (above the first heading, alongside `[guard]`
/// / `[network-default]`); reserved directive headers are skipped.
#[must_use]
pub fn parse_prompt_sets(markdown: &str) -> Vec<(String, Vec<String>, bool)> {
    let mut out = Vec::new();
    for line in markdown.lines() {
        let stripped = line.trim();
        if stripped.starts_with('#') {
            break;
        }
        if let Some((name, members, extend)) = parse_set_definition(stripped)
            && !RESERVED_HEADERS.contains(&name.as_str())
        {
            out.push((name, members, extend));
        }
    }
    out
}

/// The `[rules]: <path>` import directives in the prompt prelude, in order.
/// Each path is resolved relative to the prompt file by the set resolver.
#[must_use]
pub fn parse_rules_imports(markdown: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in markdown.lines() {
        let stripped = line.trim();
        if stripped.starts_with('#') {
            break;
        }
        if stripped.to_ascii_lowercase().starts_with("[rules]")
            && let Some((_, value)) = stripped.split_once(':')
        {
            let path = value.trim();
            if !path.is_empty() {
                out.push(path.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod prompt_sets_tests {
    use super::*;

    #[test]
    fn inline_sets_parse_and_skip_reserved_headers() {
        let md = "\
[guard]: v1
[network-default]: deny
[rules]: ./net-common.md
[net-yt]: yt.example.com:443
[code-read]+: Read, Grep

# Step 1
[allowed]: code-read
[transition]: Exit
";
        let sets = parse_prompt_sets(md);
        assert_eq!(
            sets,
            vec![
                ("net-yt".to_string(), vec!["yt.example.com:443".to_string()], false),
                ("code-read".to_string(), vec!["Read".to_string(), "Grep".to_string()], true),
            ]
        );
    }

    #[test]
    fn only_prelude_sets_are_read_not_step_bodies() {
        let md = "\
[net-a]: a.example:443

# Step 1
[net-b]: b.example:443
[transition]: Exit
";
        let sets = parse_prompt_sets(md);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].0, "net-a");
    }

    #[test]
    fn rules_imports_collected_in_order() {
        let md = "\
[rules]: ./base.md
[rules]: ./team.md

# Step 1
[transition]: Exit
";
        assert_eq!(parse_rules_imports(md), vec!["./base.md".to_string(), "./team.md".to_string()]);
    }

    #[test]
    fn no_prelude_directives_when_first_line_is_a_heading() {
        let md = "# Step 1\n[net-a]: a.example:443\n[transition]: Exit\n";
        assert!(parse_prompt_sets(md).is_empty());
        assert!(parse_rules_imports(md).is_empty());
    }
}

#[cfg(test)]
mod guard_block_tests {
    use super::*;

    #[test]
    fn flow_transitions_and_max_visits() {
        let md = "\
# Step 1
[transition]: Exit
```guard
max-visits: 2
transitions: [{to: 3, gate: \"make test\"}, {to: 5}]
```

# Step 3
[transition]: Exit

# Step 5
[transition]: Exit
";
        let steps = parse_steps(md).unwrap();
        let s = &steps[&1];
        assert_eq!(s.max_visits, Some(2));
        assert_eq!(s.transition_gates.get(&3).map(String::as_str), Some("make test"));
        assert!(!s.transition_gates.contains_key(&5));
        let (nums, exit) = parse_transitions(&s.transition);
        assert_eq!(nums, vec![3, 5]);
        assert!(exit, "the bracket-line Exit survives the merge");
    }

    #[test]
    fn block_style_transitions() {
        let md = "\
# Step 1
```guard
transitions:
  - to: 2
    gate: make check
  - to: 4
max-visits: 5
```

# Step 2
[transition]: Exit

# Step 4
[transition]: Exit
";
        let steps = parse_steps(md).unwrap();
        let s = &steps[&1];
        assert_eq!(s.max_visits, Some(5));
        assert_eq!(s.transition_gates.get(&2).map(String::as_str), Some("make check"));
        assert_eq!(parse_transitions(&s.transition).0, vec![2, 4]);
    }

    #[test]
    fn guard_fence_is_not_prose_body() {
        let md = "\
# Step 1
Do the thing.
```guard
max-visits: 1
```
";
        let steps = parse_steps(md).unwrap();
        assert_eq!(steps[&1].body, "Do the thing.");
        assert!(!steps[&1].body.contains("max-visits"));
    }

    #[test]
    fn guard_yaml_info_string_is_recognized() {
        let md = "\
# Step 1
```guard yaml
max-visits: 4
```
";
        assert_eq!(parse_steps(md).unwrap()[&1].max_visits, Some(4));
    }

    #[test]
    fn plain_fence_stays_prose() {
        let md = "\
# Step 1
```yaml
max-visits: 9
```
";
        let steps = parse_steps(md).unwrap();
        assert_eq!(steps[&1].max_visits, None);
        assert!(steps[&1].body.contains("max-visits: 9"));
    }

    #[test]
    fn quoted_gate_may_contain_commas() {
        let md = "\
# Step 1
[transition]: Exit
```guard
transitions: [{to: 2, gate: \"make a, make b\"}]
```

# Step 2
[transition]: Exit
";
        let steps = parse_steps(md).unwrap();
        assert_eq!(steps[&1].transition_gates.get(&2).map(String::as_str), Some("make a, make b"));
    }

    #[test]
    fn bad_max_visits_is_parse_error() {
        let md = "\
# Step 1
```guard
max-visits: soon
```
";
        assert!(parse_steps(md).is_err());
    }

    #[test]
    fn unknown_guard_key_is_parse_error() {
        let md = "\
# Step 1
```guard
max-vists: 3
```
";
        assert!(parse_steps(md).is_err());
    }

    #[test]
    fn transition_item_without_to_is_error() {
        let md = "\
# Step 1
```guard
transitions: [{gate: make test}]
```
";
        assert!(parse_steps(md).is_err());
    }
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
