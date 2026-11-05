//! `cctui-daemon hook-shim no-interactive-shell`: refuses shell commands that
//! open an interactive program, because Codex's `write_stdin` into a `tty`
//! session never fires `PreToolUse`. `tool_input.tty: false` passes: Codex
//! refuses stdin writes to those. Steering only.
//!
//! Exit 2 denies; so does anything unreadable, as Codex fails open otherwise.

use std::io::Read as _;

use serde_json::Value;

pub const BLOCK: i32 = 2;
const MAX_DEPTH: usize = 8;
const SHELLS: &[&str] =
    &["sh", "bash", "zsh", "dash", "ksh", "mksh", "ash", "fish", "csh", "tcsh", "pwsh"];
const KEYWORDS: &[&str] = &["if", "then", "else", "elif", "do", "while", "until", "!", "{", "time"];
const WRAPPERS: &[&str] = &[
    "sudo", "doas", "env", "exec", "command", "builtin", "nohup", "nice", "timeout", "stdbuf",
    "xargs", "setsid", "unbuffer", "busybox", "chrt", "ionice", "taskset",
];
/// Interactive whatever their arguments, short of one of their batch flags.
const FULL_SCREEN: &[(&str, &[&str])] = &[
    ("vi", &["-es", "-Es"]),
    ("vim", &["-es", "-Es"]),
    ("nvim", &["-es", "-Es", "--headless"]),
    ("view", &[]),
    ("vimdiff", &[]),
    ("emacs", &["--batch", "--script"]),
    ("nano", &[]),
    ("pico", &[]),
    ("less", &[]),
    ("more", &[]),
    ("most", &[]),
    ("man", &[]),
    ("top", &["-b"]),
    ("htop", &[]),
    ("tmux", &[]),
    ("screen", &[]),
    ("ssh", &[]),
    ("telnet", &[]),
    ("su", &[]),
    ("script", &[]),
    ("gdb", &["-batch", "--batch"]),
    ("lldb", &["--batch", "-b"]),
];
const CONTAINER_CLIS: &[&str] = &["docker", "podman", "nerdctl", "kubectl", "oc"];

#[derive(Clone, Copy)]
enum Operands {
    /// The first operand is a script to run.
    Run,
    /// A second operand is the input to run (`sqlite3 db 'select 1'`).
    Second,
    /// Operands name a target, never input (`psql mydb`).
    Target,
}

struct Repl {
    names: &'static [&'static str],
    batch: &'static [&'static str],
    interactive: &'static [&'static str],
    operands: Operands,
}

const INFO: &[&str] = &["-v", "-V", "--version", "-h", "--help"];

const REPLS: &[Repl] = &[
    Repl {
        names: &["node", "nodejs"],
        batch: &["-e", "--eval", "-p", "--print", "-c", "--check", "--test"],
        interactive: &["-i", "--interactive"],
        operands: Operands::Run,
    },
    Repl {
        names: &["deno", "bun"],
        batch: &["eval", "run", "test", "task", "install", "x", "add", "build", "fmt", "check"],
        interactive: &["repl"],
        operands: Operands::Run,
    },
    Repl {
        names: &["ruby", "perl", "irb", "pry", "tclsh", "wish", "expect", "guile", "racket"],
        batch: &["-e", "-E", "-c", "--eval", "-n", "-p"],
        interactive: &[],
        operands: Operands::Run,
    },
    Repl {
        names: &["lua", "luajit", "julia", "ipython", "bpython", "ghci", "ocaml", "R", "octave"],
        batch: &["-e", "--eval", "-c", "--vanilla", "--no-gui"],
        interactive: &["-i", "--interactive"],
        operands: Operands::Run,
    },
    Repl {
        names: &["php"],
        batch: &["-r", "-l", "-S", "-f", "-m", "-i"],
        interactive: &["-a", "--interactive"],
        operands: Operands::Run,
    },
    Repl {
        names: &["scala", "jshell", "erl", "iex", "sbcl", "clojure", "clj"],
        batch: &["-e", "--eval", "-noshell", "--script", "-M"],
        interactive: &[],
        operands: Operands::Run,
    },
    Repl {
        names: &["sqlite3", "duckdb"],
        batch: &["-cmd", "-init", "-c", "-batch"],
        interactive: &["-interactive"],
        operands: Operands::Second,
    },
    Repl {
        names: &[
            "psql",
            "mysql",
            "mariadb",
            "mongosh",
            "mongo",
            "redis-cli",
            "cqlsh",
            "clickhouse",
        ],
        batch: &["-c", "-e", "-f", "--command", "--execute", "--file", "--eval", "-x", "-l"],
        interactive: &[],
        operands: Operands::Target,
    },
];

const PYTHON: Repl =
    Repl { names: &[], batch: &["-c", "-m"], interactive: &["-i"], operands: Operands::Run };

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
    if payload.pointer("/tool_input/tty") == Some(&Value::Bool(false)) {
        return (0, None);
    }
    let Some(command) = shell_command(&payload) else {
        return (BLOCK, Some("cctui: the hook payload carries no shell command".to_owned()));
    };
    match opened_session(&command) {
        Ok(None) => (0, None),
        Ok(Some(program)) => (
            BLOCK,
            Some(format!(
                "cctui: `{program}` opens an interactive session. Input typed into it later \
                 skips this session's hook policy, so interactive sessions are refused. Run the \
                 work non-interactively instead: a script file, `-c`/`-e`, a heredoc or a pipe, \
                 with `tty` off."
            )),
        ),
        Err(Unparseable) => (
            BLOCK,
            Some(
                "cctui: this shell command could not be checked for an interactive session \
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
            let words: Option<Vec<String>> = argv
                .iter()
                .map(|w| w.as_str().map(|w| format!("'{}'", w.replace('\'', r"'\''"))))
                .collect();
            words.map(|w| w.join(" "))
        }
        _ => None,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct Unparseable;

/// The interactive program `command` would open, if any.
///
/// # Errors
/// The command does not lex or nests deeper than the shim follows.
pub fn opened_session(command: &str) -> Result<Option<String>, Unparseable> {
    check(command, 0)
}

fn check(script: &str, depth: usize) -> Result<Option<String>, Unparseable> {
    if depth > MAX_DEPTH {
        return Err(Unparseable);
    }
    let parsed = parse(script)?;
    for cmd in &parsed.commands {
        if let Some(hit) = check_simple(cmd, depth)? {
            return Ok(Some(hit));
        }
    }
    for nested in &parsed.nested {
        if let Some(hit) = check(nested, depth + 1)? {
            return Ok(Some(hit));
        }
    }
    Ok(None)
}

fn check_simple(cmd: &Simple, depth: usize) -> Result<Option<String>, Unparseable> {
    let Some((program, args)) = program_and_args(&cmd.words) else { return Ok(None) };
    let name = basename(program);
    if name == "eval" {
        return check(&args.join(" "), depth + 1);
    }
    if SHELLS.contains(&name) {
        return check_shell(name, args, cmd.fed, depth);
    }
    if let Some((_, batch)) = FULL_SCREEN.iter().find(|(n, _)| *n == name) {
        let batched = args.iter().any(|a| batch.contains(&a.as_str()));
        return Ok((!batched).then(|| name.to_owned()));
    }
    if CONTAINER_CLIS.contains(&name) {
        return Ok(container_attaches(args).then(|| format!("{name} -i")));
    }
    if let Some(repl) = REPLS.iter().find(|r| r.names.contains(&name)) {
        return Ok(repl_opens(repl, args, cmd.fed).then(|| name.to_owned()));
    }
    if is_python(name) {
        return Ok(repl_opens(&PYTHON, args, cmd.fed).then(|| name.to_owned()));
    }
    Ok(None)
}

/// `-i` opens an interactive shell; `-c` runs a script, checked in turn; a
/// script operand runs it; otherwise the shell reads its stdin.
fn check_shell(
    name: &str,
    args: &[String],
    fed: bool,
    depth: usize,
) -> Result<Option<String>, Unparseable> {
    let mut it = args.iter();
    let mut reads_stdin = false;
    while let Some(arg) = it.next() {
        let text = arg.as_str();
        match text {
            "--" if reads_stdin || it.next().is_none() => break,
            "--" => return Ok(None),
            "--interactive" => return Ok(Some(format!("{name} -i"))),
            "--rcfile" | "--init-file" | "-o" | "+o" | "-O" | "+O" => {
                it.next();
                continue;
            }
            "-" => {
                reads_stdin = true;
                continue;
            }
            _ => {}
        }
        if text.starts_with("--") {
            continue;
        }
        if text.starts_with('-') || text.starts_with('+') {
            let flags = text.trim_start_matches(['-', '+']);
            if flags.contains('c') {
                return it.next().map_or(Ok(None), |script| check(script, depth + 1));
            }
            if text.starts_with('-') && flags.contains('i') {
                return Ok(Some(format!("{name} -i")));
            }
            reads_stdin |= flags.contains('s');
            continue;
        }
        if !reads_stdin {
            return Ok(None);
        }
        break;
    }
    Ok((!fed).then(|| name.to_owned()))
}

fn repl_opens(repl: &Repl, args: &[String], fed: bool) -> bool {
    let mut operands = 0;
    for arg in args {
        let key = arg.split_once('=').map_or(arg.as_str(), |(k, _)| k);
        if repl.interactive.contains(&key) {
            return true;
        }
        if repl.batch.contains(&key) || INFO.contains(&key) {
            return false;
        }
        if arg.starts_with('-') {
            continue;
        }
        operands += 1;
        match repl.operands {
            Operands::Run => return false,
            Operands::Second if operands >= 2 => return false,
            Operands::Second | Operands::Target => {}
        }
    }
    !fed
}

fn container_attaches(args: &[String]) -> bool {
    args.iter().any(|a| {
        let a = a.as_str();
        matches!(a, "attach" | "--interactive" | "--stdin" | "--interactive=true" | "--stdin=true")
            || (a.len() > 1
                && a.starts_with('-')
                && !a.starts_with("--")
                && a[1..].chars().all(|c| c.is_ascii_alphabetic())
                && a.contains('i'))
    })
}

fn is_python(name: &str) -> bool {
    let rest = name.strip_prefix("python").or_else(|| name.strip_prefix("pypy"));
    rest.is_some_and(|r| r.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

fn program_and_args(words: &[String]) -> Option<(&str, &[String])> {
    let mut i = 0;
    while let Some(word) = words.get(i) {
        let text = word.as_str();
        if is_assignment(text) || KEYWORDS.contains(&text) {
            i += 1;
            continue;
        }
        let name = basename(text);
        if !WRAPPERS.contains(&name) {
            return Some((text, &words[i + 1..]));
        }
        if name == "command" && words.get(i + 1).is_some_and(|w| w == "-v" || w == "-V") {
            return None;
        }
        i = skip_wrapper_options(words, i + 1, wrapper_arg_flags(name));
    }
    None
}

fn skip_wrapper_options(words: &[String], mut i: usize, takes_arg: &[&str]) -> usize {
    while let Some(text) = words.get(i).map(String::as_str) {
        if text == "--" {
            return i + 1;
        }
        if text.starts_with('-') {
            i += if takes_arg.contains(&text) { 2 } else { 1 };
        } else if is_assignment(text) || is_number(text) {
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
        "nice" | "ionice" => &["-n", "-c"],
        "taskset" => &["-c"],
        _ => &[],
    }
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

fn is_number(word: &str) -> bool {
    let digits = word.trim_end_matches(['s', 'm', 'h', 'd']);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
}

#[derive(Debug, Default)]
struct Simple {
    words: Vec<String>,
    /// Stdin comes from a pipe, a file or a heredoc, not the terminal.
    fed: bool,
}

#[derive(Default)]
struct Parsed {
    commands: Vec<Simple>,
    /// Command and process substitution bodies, checked as scripts.
    nested: Vec<String>,
}

struct Lexer<'a> {
    chars: &'a [char],
    pos: usize,
    out: Parsed,
    current: Simple,
    word: Option<String>,
    skip_word: bool,
    heredocs: Vec<(String, bool)>,
}

fn parse(script: &str) -> Result<Parsed, Unparseable> {
    let chars: Vec<char> = script.chars().collect();
    let mut lexer = Lexer {
        chars: &chars,
        pos: 0,
        out: Parsed::default(),
        current: Simple::default(),
        word: None,
        skip_word: false,
        heredocs: Vec::new(),
    };
    while let Some(c) = lexer.peek() {
        lexer.step(c)?;
    }
    lexer.end_command(false);
    Ok(lexer.out)
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn word(&mut self) -> &mut String {
        self.word.get_or_insert_with(String::new)
    }

    fn end_word(&mut self) {
        if let Some(word) = self.word.take() {
            if std::mem::take(&mut self.skip_word) {
                return;
            }
            self.current.words.push(word);
        }
    }

    fn end_command(&mut self, piped_next: bool) {
        self.end_word();
        let done = std::mem::take(&mut self.current);
        if !done.words.is_empty() {
            self.out.commands.push(done);
        }
        self.current.fed = piped_next;
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
                self.skip_heredoc_bodies();
            }
            ';' | '(' | ')' | '&' => {
                self.pos += 1;
                self.end_command(false);
            }
            '|' => {
                let or = self.peek_at(1) == Some('|');
                self.pos += if or { 2 } else { 1 };
                if self.peek() == Some('&') {
                    self.pos += 1;
                }
                self.end_command(!or);
            }
            '<' | '>' if self.peek_at(1) == Some('(') => {
                self.pos += 2;
                self.substitution(')')?;
            }
            '<' => self.input_redirect(),
            '>' => self.output_redirect(),
            '#' if self.word.is_none() => {
                while self.peek().is_some_and(|c| c != '\n') {
                    self.pos += 1;
                }
            }
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
                self.word().push(c);
            }
        }
        Ok(())
    }

    /// A pending all-digit word is the redirect's file descriptor.
    fn drop_fd_word(&mut self) {
        let is_fd = |w: &str| !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit());
        if self.word.as_deref().is_some_and(is_fd) {
            self.word = None;
        }
        self.end_word();
    }

    fn input_redirect(&mut self) {
        self.drop_fd_word();
        self.current.fed = true;
        self.pos += 1;
        if self.peek() != Some('<') {
            if self.peek() == Some('&') {
                self.pos += 1;
            }
            self.skip_word = true;
            return;
        }
        self.pos += 1;
        if self.peek() == Some('<') {
            self.pos += 1;
            self.skip_word = true;
            return;
        }
        let strip_tabs = self.peek() == Some('-');
        if strip_tabs {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.pos += 1;
        }
        let mut delimiter = String::new();
        while let Some(c) = self.peek() {
            if matches!(c, ' ' | '\t' | '\n' | ';' | '|' | '&' | '<' | '>' | '(' | ')') {
                break;
            }
            self.pos += 1;
            if !matches!(c, '\'' | '"' | '\\') {
                delimiter.push(c);
            }
        }
        self.heredocs.push((delimiter, strip_tabs));
    }

    fn output_redirect(&mut self) {
        self.drop_fd_word();
        self.pos += 1;
        while matches!(self.peek(), Some('>' | '|')) {
            self.pos += 1;
        }
        if self.peek() == Some('&') {
            self.pos += 1;
            if self.peek().is_some_and(|c| c.is_ascii_digit() || c == '-') {
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '-') {
                    self.pos += 1;
                }
                return;
            }
        }
        self.skip_word = true;
    }

    fn skip_heredoc_bodies(&mut self) {
        for (delimiter, strip_tabs) in std::mem::take(&mut self.heredocs) {
            while self.pos < self.chars.len() {
                let start = self.pos;
                while self.peek().is_some_and(|c| c != '\n') {
                    self.pos += 1;
                }
                let line: String = self.chars[start..self.pos].iter().collect();
                if self.peek() == Some('\n') {
                    self.pos += 1;
                }
                let line = if strip_tabs { line.trim_start_matches('\t') } else { line.as_str() };
                if line == delimiter {
                    break;
                }
            }
        }
    }

    fn escaped(&mut self) {
        self.pos += 1;
        match self.peek() {
            Some('\n') => self.pos += 1,
            Some(c) => {
                self.pos += 1;
                self.word().push(c);
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
            self.word().push(c);
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
                        if !matches!(next, '"' | '\\' | '$' | '`') {
                            self.word().push('\\');
                        }
                        self.word().push(next);
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
                    self.word().push(c);
                }
            }
        }
        Err(Unparseable)
    }

    /// Captures a substitution body up to its `close`, for a separate check.
    fn substitution(&mut self, close: char) -> Result<(), Unparseable> {
        let start = self.pos;
        let mut depth = 0usize;
        let mut quote: Option<char> = None;
        while let Some(c) = self.peek() {
            self.pos += 1;
            match (quote, c) {
                (Some('\''), '\'') | (Some('"'), '"') => quote = None,
                (None, '\'' | '"') if close != '`' => quote = Some(c),
                (None, '\\') => self.pos += 1,
                (None, '(') if close == ')' => depth += 1,
                (None, c) if c == close && depth == 0 => {
                    let body: String = self.chars[start..self.pos - 1].iter().collect();
                    self.out.nested.push(body);
                    self.word().push_str("$()");
                    return Ok(());
                }
                (None, ')') => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        Err(Unparseable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opens(command: &str) -> Option<String> {
        opened_session(command).unwrap_or_else(|_| panic!("unparseable: {command}"))
    }

    #[test]
    fn interactive_shells_are_refused() {
        for command in [
            "bash -i",
            "bash",
            "sh",
            "/bin/zsh -il",
            "exec bash --login -i",
            "env TERM=xterm bash",
            "sudo -u root bash",
            "cd /repo && bash -i",
            "bash --interactive",
            "bash -s",
            "busybox sh",
            "timeout 300 bash -i",
            "bash -c 'bash -i'",
            "sh -c \"python3\"",
            "eval bash -i",
            "echo $(bash -i)",
            "x=`zsh -i`",
            "bash -i 2>&1",
            "bash 2>/dev/null",
        ] {
            assert!(opens(command).is_some(), "{command}");
        }
    }

    #[test]
    fn shells_that_run_a_script_pass() {
        for command in [
            "bash -c 'ls -la'",
            "bash script.sh",
            "bash -e ./build.sh --flag",
            "sh -- run.sh",
            "echo ls | bash",
            "curl -s https://x | sh -s -- --yes",
            "bash < setup.sh",
            "bash <<'EOF'\nls\nless README\nEOF",
            "bash <<-EOF\n\tvim\n\tEOF\necho done",
            "bash <<< 'ls'",
            "command -v bash",
            "git status; cargo test",
            "ls -la # bash -i",
            "echo 'bash -i'",
        ] {
            assert_eq!(opens(command), None, "{command}");
        }
    }

    #[test]
    fn repls_without_a_script_are_refused() {
        for command in [
            "python3",
            "python",
            "python3.12 -i script.py",
            "pypy3",
            "node",
            "node --interactive",
            "deno repl",
            "irb",
            "ruby",
            "perl",
            "php -a",
            "lua",
            "psql mydb",
            "mysql -u root",
            "sqlite3 app.db",
            "ghci",
        ] {
            assert!(opens(command).is_some(), "{command}");
        }
    }

    #[test]
    fn repls_running_a_script_or_fed_input_pass() {
        for command in [
            "python3 script.py",
            "python3 -c 'print(1)'",
            "python -m pytest -q",
            "python3 - <<'EOF'\nimport os\nless = 1\nEOF",
            "echo 1 | python3",
            "node build.js",
            "node -e 'console.log(1)'",
            "deno run main.ts",
            "ruby -e 'puts 1'",
            "perl -ne 'print' file",
            "psql -c 'select 1' mydb",
            "mysql -e 'show tables' db",
            "psql mydb < schema.sql",
            "sqlite3 app.db 'select 1'",
            "python3 --version",
            "pythonic-tool run",
        ] {
            assert_eq!(opens(command), None, "{command}");
        }
    }

    #[test]
    fn full_screen_programs_and_container_attaches_are_refused() {
        for command in [
            "vim src/main.rs",
            "git log | less",
            "man bash",
            "top",
            "tmux new -s x",
            "ssh host",
            "docker exec -it app sh",
            "kubectl exec --stdin pod -- ls",
            "docker attach app",
            "gdb ./a.out",
        ] {
            assert!(opens(command).is_some(), "{command}");
        }
        for command in [
            "emacs --batch -l x.el",
            "top -b -n 1",
            "nvim --headless +q",
            "gdb -batch -ex bt ./a.out",
            "docker exec app ls",
            "docker run --rm -e X=1 img",
            "kubectl get pods",
        ] {
            assert_eq!(opens(command), None, "{command}");
        }
    }

    #[test]
    fn the_hook_denies_with_a_reason_and_fails_closed() {
        let deny = |command: &str| {
            let payload =
                serde_json::json!({ "tool_name": "Bash", "tool_input": { "command": command } });
            decide(&payload.to_string())
        };
        let (code, reason) = deny("bash -i");
        assert_eq!(code, BLOCK);
        assert!(reason.unwrap().contains("`bash` opens an interactive session"));
        assert_eq!(deny("cargo test"), (0, None));
        let (code, reason) = deny("echo 'unterminated");
        assert_eq!(code, BLOCK);
        assert!(reason.unwrap().contains("could not be checked"));
        assert_eq!(decide("not json").0, BLOCK);
        assert_eq!(decide(r#"{"tool_input":{}}"#).0, BLOCK);
        let argv = serde_json::json!({ "tool_input": { "command": ["bash", "-i"] } });
        assert_eq!(decide(&argv.to_string()).0, BLOCK);
    }

    #[test]
    fn a_session_without_a_tty_passes() {
        let payload = serde_json::json!({ "tool_input": { "command": "bash -i", "tty": false } });
        assert_eq!(decide(&payload.to_string()), (0, None));
        let payload = serde_json::json!({ "tool_input": { "cmd": "bash -i", "tty": true } });
        assert_eq!(decide(&payload.to_string()).0, BLOCK);
    }

    #[test]
    fn nesting_past_the_limit_is_unparseable() {
        let mut command = "bash -i".to_owned();
        for _ in 0..=MAX_DEPTH {
            command = format!("eval {command}");
        }
        assert_eq!(opened_session(&command), Err(Unparseable));
    }
}
