//! `cctui-daemon hook-shim no-direct-harness`: the `PreToolUse` shell guard
//! every session inherits. A harness started from a shell runs outside the
//! session's hook policy, so the model is pointed at `CctuiAgent` instead.
//!
//! It cannot see through variables or generated code: this is steering, the
//! exec-interception layer is the enforcement.
//!
//! Exit 0 allows. Exit 2 denies with the reason on stderr; so does anything the
//! shim cannot read or parse, since harnesses fail open on every other code.

use std::io::Read as _;

use serde_json::Value;

pub const BLOCK: i32 = 2;
const MAX_DEPTH: usize = 8;
const HARNESSES: &[&str] = &["claude", "codex", "opencode", "gemini"];
const PACKAGES: &[&str] =
    &["@anthropic-ai/claude-code", "@openai/codex", "opencode-ai", "@google/gemini-cli"];
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish"];
const KEYWORDS: &[&str] = &["if", "then", "else", "elif", "do", "while", "until", "!", "{"];
const WRAPPERS: &[&str] = &[
    "sudo", "doas", "env", "exec", "command", "builtin", "nohup", "nice", "time", "timeout",
    "stdbuf", "xargs", "setsid", "unbuffer", "watch",
];
const INFO_FLAGS: &[&str] = &["--version", "-v", "-V", "--help", "-h"];

#[must_use]
pub fn run() -> i32 {
    let mut stdin = String::new();
    let (code, message) = match std::io::stdin().read_to_string(&mut stdin) {
        Ok(_) => decide(&stdin),
        Err(err) => (BLOCK, Some(format!("cctui: cannot read the hook payload: {err}"))),
    };
    if let Some(message) = message {
        eprintln!("{message}");
    }
    code
}

#[must_use]
pub fn decide(payload: &str) -> (i32, Option<String>) {
    let Ok(payload) = serde_json::from_str::<Value>(payload) else {
        return (BLOCK, Some("cctui: the hook payload is not JSON".to_owned()));
    };
    let Some(command) = shell_command(&payload) else {
        return (BLOCK, Some("cctui: the hook payload carries no shell command".to_owned()));
    };
    match launched_harness(&command) {
        Ok(None) => (0, None),
        Ok(Some(harness)) => (
            BLOCK,
            Some(format!(
                "cctui: do not start `{harness}` from a shell. A harness launched this way runs \
                 outside this session's hook policy and is not tracked by cctui. Use the \
                 CctuiAgent tool to start a child agent instead."
            )),
        ),
        Err(Unparseable) => (
            BLOCK,
            Some(
                "cctui: this shell command could not be checked for a direct harness launch \
                 (unbalanced quotes or nesting too deep). Rewrite it more simply."
                    .to_owned(),
            ),
        ),
    }
}

fn shell_command(payload: &Value) -> Option<String> {
    let input = payload.get("tool_input")?;
    ["command", "cmd"].iter().find_map(|key| match input.get(key)? {
        Value::String(command) => Some(command.clone()),
        Value::Array(argv) => {
            let words: Option<Vec<String>> =
                argv.iter().map(|w| w.as_str().map(single_quoted)).collect();
            words.map(|w| w.join(" "))
        }
        _ => None,
    })
}

fn single_quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

#[derive(Debug, PartialEq, Eq)]
pub struct Unparseable;

/// The harness `command` would start, if any.
///
/// # Errors
/// The command does not lex (unbalanced quotes, unterminated substitution) or
/// nests deeper than the shim follows.
pub fn launched_harness(command: &str) -> Result<Option<String>, Unparseable> {
    check(command, 0)
}

fn check(script: &str, depth: usize) -> Result<Option<String>, Unparseable> {
    if depth > MAX_DEPTH {
        return Err(Unparseable);
    }
    let chars: Vec<char> = script.chars().collect();
    let mut lexer = Lexer::new(&chars, depth);
    lexer.lex(None)?;
    for (i, cmd) in lexer.commands.iter().enumerate() {
        let previous = i.checked_sub(1).and_then(|p| lexer.commands.get(p));
        if let Some(hit) = check_simple(cmd, previous, depth)? {
            return Ok(Some(hit));
        }
    }
    for cmd in &lexer.substituted {
        if let Some(hit) = check_simple(cmd, None, depth)? {
            return Ok(Some(hit));
        }
    }
    Ok(None)
}

fn check_simple(
    cmd: &Simple,
    previous: Option<&Simple>,
    depth: usize,
) -> Result<Option<String>, Unparseable> {
    let Some((program, args)) = program_and_args(&cmd.words) else { return Ok(None) };
    if program.mentions_harness {
        return Ok(Some("a harness".to_owned()));
    }
    let name = basename(&program.text);
    if HARNESSES.contains(&name) && !only_info_flags(args) {
        return Ok(Some(name.to_owned()));
    }
    if SHELLS.contains(&name) {
        return check_shell(cmd, args, previous, depth);
    }
    if name == "eval" {
        let script: Vec<&str> = args.iter().map(|w| w.text.as_str()).collect();
        return check(&script.join(" "), depth + 1);
    }
    Ok(runner_package(name, args).filter(|p| is_harness_package(p)).map(str::to_owned))
}

/// A shell runs a `-c` script, or reads its stdin: a heredoc, or whatever the
/// previous pipeline stage prints.
fn check_shell(
    cmd: &Simple,
    args: &[Word],
    previous: Option<&Simple>,
    depth: usize,
) -> Result<Option<String>, Unparseable> {
    if let Some(script) = shell_script(args) {
        return check(script, depth + 1);
    }
    if !args.iter().all(|a| a.text.starts_with('-')) {
        return Ok(None);
    }
    for body in &cmd.bodies {
        if let Some(hit) = check(body, depth + 1)? {
            return Ok(Some(hit));
        }
    }
    if cmd.piped
        && let Some(previous) = previous
    {
        for word in previous.words.iter().skip(1) {
            if let Some(hit) = check(&word.text, depth + 1)? {
                return Ok(Some(hit));
            }
        }
    }
    Ok(None)
}

fn program_and_args(words: &[Word]) -> Option<(&Word, &[Word])> {
    let mut i = 0;
    while let Some(word) = words.get(i) {
        let text = word.text.as_str();
        if is_assignment(text) || KEYWORDS.contains(&text) {
            i += 1;
            continue;
        }
        let name = basename(text);
        if word.mentions_harness || !WRAPPERS.contains(&name) {
            return Some((word, &words[i + 1..]));
        }
        if name == "command" && words.get(i + 1).is_some_and(|w| w.text == "-v" || w.text == "-V") {
            return None;
        }
        i = skip_wrapper_options(words, i + 1, wrapper_arg_flags(name));
    }
    None
}

fn skip_wrapper_options(words: &[Word], mut i: usize, takes_arg: &[&str]) -> usize {
    while let Some(word) = words.get(i) {
        let text = word.text.as_str();
        if text == "--" {
            return i + 1;
        }
        if text.starts_with('-') {
            i += if takes_arg.contains(&text) { 2 } else { 1 };
        } else if is_assignment(text) || is_duration(text) {
            i += 1;
        } else {
            break;
        }
    }
    i
}

fn wrapper_arg_flags(wrapper: &str) -> &'static [&'static str] {
    match wrapper {
        "sudo" | "doas" => &["-u", "-g", "-C", "-D", "-h", "-p", "-r", "-t", "-U"],
        "xargs" => &["-I", "-n", "-P", "-L", "-s", "-d", "-E", "-a"],
        "env" => &["-u", "-C"],
        "timeout" => &["-k", "-s"],
        "nice" => &["-n"],
        _ => &[],
    }
}

fn shell_script(args: &[Word]) -> Option<&str> {
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let text = arg.text.as_str();
        if text == "--" || !(text.starts_with('-') || text.starts_with('+')) {
            return None;
        }
        if text.starts_with("--") {
            continue;
        }
        if matches!(text, "-o" | "+o" | "-O" | "+O") {
            it.next();
        } else if text.trim_start_matches(['-', '+']).contains('c') {
            return it.next().map(|w| w.text.as_str());
        }
    }
    None
}

fn runner_package<'w>(name: &str, args: &'w [Word]) -> Option<&'w str> {
    let rest = match name {
        "npx" | "bunx" | "pnpx" => args,
        "npm" | "pnpm" | "yarn" | "bun" => match args.split_first() {
            Some((sub, rest)) if matches!(sub.text.as_str(), "exec" | "x" | "dlx") => rest,
            _ => return None,
        },
        _ => return None,
    };
    rest.iter().map(|w| w.text.as_str()).find(|a| !a.starts_with('-'))
}

fn is_harness_package(package: &str) -> bool {
    let bare = package.strip_prefix('@').map_or_else(
        || package.split('@').next().unwrap_or(package),
        |scoped| scoped.split_once('@').map_or(package, |(name, _)| &package[..=name.len()]),
    );
    PACKAGES.contains(&bare) || HARNESSES.contains(&basename(bare))
}

fn only_info_flags(args: &[Word]) -> bool {
    !args.is_empty() && args.iter().all(|a| INFO_FLAGS.contains(&a.text.as_str()))
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else { return false };
    let name = name.strip_suffix('+').unwrap_or(name);
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn is_duration(word: &str) -> bool {
    let digits = word.trim_end_matches(['s', 'm', 'h', 'd']);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
}

#[derive(Debug, Default)]
struct Word {
    text: String,
    /// A substitution inside this word names a harness: `$(which claude) -p x`.
    mentions_harness: bool,
}

#[derive(Debug, Default)]
struct Simple {
    words: Vec<Word>,
    bodies: Vec<String>,
    /// Reads the previous command's stdout.
    piped: bool,
}

struct Heredoc {
    delimiter: String,
    strip_tabs: bool,
    owner: usize,
}

struct Lexer<'a> {
    chars: &'a [char],
    pos: usize,
    depth: usize,
    commands: Vec<Simple>,
    substituted: Vec<Simple>,
    current: Simple,
    word: Option<Word>,
    redirect_target: bool,
    heredocs: Vec<Heredoc>,
}

impl<'a> Lexer<'a> {
    fn new(chars: &'a [char], depth: usize) -> Self {
        Self::at(chars, 0, depth)
    }

    fn at(chars: &'a [char], pos: usize, depth: usize) -> Self {
        Self {
            chars,
            pos,
            depth,
            commands: Vec::new(),
            substituted: Vec::new(),
            current: Simple::default(),
            word: None,
            redirect_target: false,
            heredocs: Vec::new(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn word(&mut self) -> &mut Word {
        self.word.get_or_insert_with(Word::default)
    }

    fn end_word(&mut self) {
        if let Some(word) = self.word.take() {
            if self.redirect_target {
                self.redirect_target = false;
            } else {
                self.current.words.push(word);
            }
        }
    }

    fn end_command(&mut self, piped_next: bool) {
        self.end_word();
        let done = std::mem::take(&mut self.current);
        if !done.words.is_empty() || !done.bodies.is_empty() {
            self.commands.push(done);
        }
        self.current.piped = piped_next;
    }

    /// Lex up to `close` (consumed) or the end of input.
    fn lex(&mut self, close: Option<char>) -> Result<(), Unparseable> {
        while let Some(c) = self.peek() {
            if Some(c) == close {
                self.pos += 1;
                self.end_command(false);
                return Ok(());
            }
            self.step(c)?;
        }
        if close.is_some() {
            return Err(Unparseable);
        }
        self.end_command(false);
        Ok(())
    }

    fn step(&mut self, c: char) -> Result<(), Unparseable> {
        match c {
            ' ' | '\t' => {
                self.pos += 1;
                self.end_word();
            }
            '\n' => {
                self.pos += 1;
                self.end_command(false);
                self.read_heredocs();
            }
            '&' if self.peek_at(1) == Some('>') => self.redirect(),
            ';' | '(' | ')' | '&' => {
                self.pos += 1;
                self.end_command(false);
            }
            '|' => {
                let or = self.peek_at(1) == Some('|');
                self.pos += if or { 2 } else { 1 };
                self.end_command(!or);
            }
            '<' if self.peek_at(1) == Some('<') && self.peek_at(2) != Some('<') => {
                self.heredoc()?;
            }
            '<' | '>' => self.redirect(),
            '#' if self.word.is_none() => self.skip_comment(),
            '\'' => self.single_quoted()?,
            '"' => self.double_quoted()?,
            '\\' => self.escaped(),
            '$' if self.peek_at(1) == Some('(') => {
                self.pos += 2;
                self.substitution(')')?;
            }
            '`' => {
                self.pos += 1;
                self.substitution('`')?;
            }
            _ => {
                self.pos += 1;
                self.word().text.push(c);
            }
        }
        Ok(())
    }

    fn redirect(&mut self) {
        self.end_word();
        self.pos += 1;
        while matches!(self.peek(), Some('>' | '&' | '<')) {
            self.pos += 1;
        }
        let fd_dup = self.peek().is_some_and(|c| c.is_ascii_digit() || c == '-')
            && self.chars.get(self.pos.saturating_sub(1)) == Some(&'&');
        if fd_dup {
            while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '-') {
                self.pos += 1;
            }
        } else {
            self.redirect_target = true;
        }
    }

    fn skip_comment(&mut self) {
        while self.peek().is_some_and(|c| c != '\n') {
            self.pos += 1;
        }
    }

    fn escaped(&mut self) {
        self.pos += 1;
        match self.peek() {
            Some('\n') => self.pos += 1,
            Some(c) => {
                self.pos += 1;
                self.word().text.push(c);
            }
            None => {}
        }
    }

    fn single_quoted(&mut self) -> Result<(), Unparseable> {
        self.pos += 1;
        self.word();
        while let Some(c) = self.peek() {
            self.pos += 1;
            if c == '\'' {
                return Ok(());
            }
            self.word().text.push(c);
        }
        Err(Unparseable)
    }

    fn double_quoted(&mut self) -> Result<(), Unparseable> {
        self.pos += 1;
        self.word();
        while let Some(c) = self.peek() {
            match c {
                '"' => {
                    self.pos += 1;
                    return Ok(());
                }
                '\\' => {
                    self.pos += 1;
                    if let Some(next) = self.peek() {
                        self.pos += 1;
                        self.word().text.push(next);
                    }
                }
                '$' if self.peek_at(1) == Some('(') => {
                    self.pos += 2;
                    self.substitution(')')?;
                }
                '`' => {
                    self.pos += 1;
                    self.substitution('`')?;
                }
                _ => {
                    self.pos += 1;
                    self.word().text.push(c);
                }
            }
        }
        Err(Unparseable)
    }

    fn substitution(&mut self, close: char) -> Result<(), Unparseable> {
        if self.depth >= MAX_DEPTH {
            return Err(Unparseable);
        }
        let mut inner = Self::at(self.chars, self.pos, self.depth + 1);
        inner.lex(Some(close))?;
        self.pos = inner.pos;
        let mentions = inner
            .commands
            .iter()
            .chain(&inner.substituted)
            .flat_map(|c| &c.words)
            .any(|w| HARNESSES.contains(&basename(&w.text)));
        self.word().mentions_harness |= mentions;
        self.substituted.append(&mut inner.commands);
        self.substituted.append(&mut inner.substituted);
        Ok(())
    }

    fn heredoc(&mut self) -> Result<(), Unparseable> {
        self.end_word();
        self.pos += 2;
        let strip_tabs = self.peek() == Some('-');
        if strip_tabs {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.pos += 1;
        }
        let mut delimiter = String::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() || matches!(c, ';' | '&' | '|' | '<' | '>' | '(' | ')') {
                break;
            }
            self.pos += 1;
            if !matches!(c, '\'' | '"' | '\\') {
                delimiter.push(c);
            }
        }
        if delimiter.is_empty() {
            return Err(Unparseable);
        }
        self.heredocs.push(Heredoc { delimiter, strip_tabs, owner: self.commands.len() });
        Ok(())
    }

    fn read_heredocs(&mut self) {
        for doc in std::mem::take(&mut self.heredocs) {
            let mut body = String::new();
            while self.pos < self.chars.len() {
                let end = self.chars[self.pos..]
                    .iter()
                    .position(|&c| c == '\n')
                    .map_or(self.chars.len(), |n| self.pos + n);
                let line: String = self.chars[self.pos..end].iter().collect();
                self.pos = (end + 1).min(self.chars.len());
                let probe = if doc.strip_tabs { line.trim_start_matches('\t') } else { &line };
                if probe == doc.delimiter {
                    break;
                }
                body.push_str(&line);
                body.push('\n');
            }
            if let Some(owner) = self.commands.get_mut(doc.owner) {
                owner.bodies.push(body);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(command: &str) -> String {
        serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": command },
        })
        .to_string()
    }

    fn denied(command: &str) -> bool {
        let (code, message) = decide(&hook(command));
        assert!(code == 0 || code == BLOCK, "unexpected exit {code}");
        if code == BLOCK {
            assert!(message.is_some_and(|m| m.contains("cctui")), "a denial explains itself");
        }
        code == BLOCK
    }

    #[test]
    fn direct_harness_launches_are_denied() {
        for command in [
            "claude -p x",
            "codex exec y",
            "opencode run z",
            "gemini -p hello",
            "claude",
            "/home/u/.local/bin/claude -p x",
            "./node_modules/.bin/codex exec y",
            "\\claude -p x",
            "cl\"\"aude -p x",
        ] {
            assert!(denied(command), "{command}");
        }
    }

    #[test]
    fn the_denial_points_at_cctui_agent() {
        let (code, message) = decide(&hook("codex exec y"));
        assert_eq!(code, BLOCK);
        let message = message.unwrap();
        assert!(message.contains("CctuiAgent"), "{message}");
        assert!(message.contains("`codex`"), "{message}");
    }

    #[test]
    fn launches_hidden_in_shells_chains_and_substitutions_are_denied() {
        for command in [
            "bash -c \"claude -p x\"",
            "sh -c 'codex exec y'",
            "bash -lc 'opencode run z'",
            "zsh -o pipefail -c 'claude -p x'",
            "cd /w && claude -p x",
            "ls; codex exec y",
            "true || opencode run z",
            "echo $(claude -p x)",
            "echo \"`codex exec y`\"",
            "$(which claude) -p x",
            "( claude -p x )",
            "if true; then claude -p x; fi",
            "eval 'codex exec y'",
            "echo 'claude -p x' | bash",
            "bash <<'EOF'\nclaude -p x\nEOF",
            "bash -c \"bash -c 'claude -p x'\"",
        ] {
            assert!(denied(command), "{command}");
        }
    }

    #[test]
    fn wrappers_and_runners_do_not_hide_a_launch() {
        for command in [
            "env FOO=1 claude -p x",
            "FOO=1 BAR=2 codex exec y",
            "sudo -u bob claude -p x",
            "timeout 30s codex exec y",
            "nohup opencode run z &",
            "exec claude -p x",
            "npx @anthropic-ai/claude-code -p x",
            "npx -y @openai/codex@latest exec y",
            "bunx opencode-ai run z",
            "pnpm dlx @google/gemini-cli -p x",
            "npm exec -- codex exec y",
        ] {
            assert!(denied(command), "{command}");
        }
    }

    #[test]
    fn ordinary_commands_are_allowed() {
        for command in [
            "ls -la",
            "cargo test -p cctui-daemon",
            "git commit -m \"$(cat <<'EOF'\nfix(claude): it's the codex path\nEOF\n)\"",
            "grep -rn claude src/",
            "cat CLAUDE.md",
            "which claude",
            "command -v codex",
            "claude --version",
            "codex --help",
            "echo 'run claude -p later' > notes.txt",
            "cat <<EOF > doc.md\nclaude -p x\nEOF",
            "rg 'codex exec' docs | head -5",
            "npx prettier --write .",
            "ls 2>&1 | tee out.log",
            "make build # then claude -p x",
            "bash scripts/test.sh",
        ] {
            assert!(!denied(command), "{command}");
        }
    }

    #[test]
    fn a_codex_argv_command_is_checked_too() {
        let payload = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": ["bash", "-lc", "claude -p 'it''s'"] },
        });
        assert_eq!(decide(&payload.to_string()).0, BLOCK);
        let payload = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": ["ls", "-la"] },
        });
        assert_eq!(decide(&payload.to_string()).0, 0);
    }

    #[test]
    fn the_hook_shim_cli_names_this_shim() {
        use clap::Parser as _;
        #[derive(clap::Parser)]
        struct Cli {
            #[command(subcommand)]
            shim: super::super::shim::ShimCmd,
        }
        let cli = Cli::try_parse_from(["hook-shim", "no-direct-harness"]).unwrap();
        assert!(matches!(cli.shim, super::super::shim::ShimCmd::NoDirectHarness));
        let rule = cctui_proto::hook_policy::no_direct_harness_rule();
        assert!(rule.command.starts_with("cctui-daemon hook-shim no-direct-harness"));
    }

    #[test]
    fn bad_input_fails_closed() {
        assert_eq!(decide("not json").0, BLOCK);
        assert_eq!(decide("{}").0, BLOCK);
        assert_eq!(decide(r#"{"tool_input": {"command": 7}}"#).0, BLOCK);
        assert_eq!(decide(&hook("echo 'unterminated")).0, BLOCK);
        assert_eq!(decide(&hook("echo $(ls")).0, BLOCK);
        let nesting = MAX_DEPTH + 2;
        let deep = format!("{}claude -p x{}", "$(".repeat(nesting), ")".repeat(nesting));
        assert_eq!(decide(&hook(&deep)).0, BLOCK);
    }
}
