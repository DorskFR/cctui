//! `cctui-daemon hook-shim apply-patch`: runs a Claude-shaped guard against a
//! Codex hook payload.
//!
//! Codex reports every file edit as one `apply_patch` call carrying the whole
//! `*** Begin Patch … *** End Patch` text. The shim splits that patch per file
//! and runs the guard once per file with the payload Claude Code would have
//! sent: `Add File` and `Delete File` become `Write` (`content` is empty for a
//! delete), `Update File` becomes `Edit` (`old_string` / `new_string` from the
//! hunks; a `Move to` also runs the guard on the destination). Any other
//! payload reaches the guard unchanged.
//!
//! Codex fails open on a hook that exits 1, prints bad JSON or answers `ask`,
//! so the shim never does: its own failures exit 2, and a guard's `ask` is
//! forwarded as `deny`.

use std::io::{Read as _, Write as _};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

pub const BLOCK: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Tools {
    Edit,
    Write,
    Any,
}

impl Tools {
    #[must_use]
    pub const fn as_arg(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Write => "write",
            Self::Any => "any",
        }
    }

    const fn keeps(self, op: &FileOp) -> bool {
        matches!(
            (self, op),
            (Self::Any, _)
                | (Self::Edit, FileOp::Update { .. })
                | (Self::Write, FileOp::Add { .. } | FileOp::Delete { .. })
        )
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
pub enum ShimCmd {
    /// Split a Codex `apply_patch` payload into per-file Claude `Edit`/`Write`
    /// payloads and run `--run` through `sh -c` on each.
    ApplyPatch {
        #[arg(long, value_enum, default_value = "any")]
        tools: Tools,
        #[arg(long)]
        run: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    Add { path: String, content: String },
    Delete { path: String },
    Update { path: String, move_to: Option<String>, old: String, new: String },
}

fn marker<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.trim().strip_prefix(prefix).map(str::trim).filter(|p| !p.is_empty())
}

#[derive(Default)]
struct Hunks {
    old: Vec<String>,
    new: Vec<String>,
}

impl Hunks {
    fn take(&mut self, line: &str) -> Result<(), String> {
        if line.trim() == "*** End of File" {
            return Ok(());
        }
        if line.starts_with("@@") {
            if !self.old.is_empty() || !self.new.is_empty() {
                self.old.push(String::new());
                self.new.push(String::new());
            }
            return Ok(());
        }
        let mut chars = line.chars();
        match chars.next() {
            Some('-') => self.old.push(chars.as_str().to_owned()),
            Some('+') => self.new.push(chars.as_str().to_owned()),
            Some(' ') => {
                self.old.push(chars.as_str().to_owned());
                self.new.push(chars.as_str().to_owned());
            }
            None => {
                self.old.push(String::new());
                self.new.push(String::new());
            }
            Some(_) => return Err(format!("unexpected patch line: {line}")),
        }
        Ok(())
    }
}

enum Open {
    None,
    Add { path: String, lines: Vec<String> },
    Update { path: String, move_to: Option<String>, hunks: Hunks },
}

impl Open {
    fn close(self, ops: &mut Vec<FileOp>) {
        match self {
            Self::None => {}
            Self::Add { path, lines } => {
                let mut content = lines.join("\n");
                if !lines.is_empty() {
                    content.push('\n');
                }
                ops.push(FileOp::Add { path, content });
            }
            Self::Update { path, move_to, hunks } => ops.push(FileOp::Update {
                path,
                move_to,
                old: hunks.old.join("\n"),
                new: hunks.new.join("\n"),
            }),
        }
    }
}

/// Parse an `apply_patch` envelope into one operation per file.
///
/// # Errors
/// A text that is not a well-formed patch: the caller blocks the call.
pub fn parse_patch(text: &str) -> Result<Vec<FileOp>, String> {
    let mut lines = text.lines().skip_while(|l| l.trim() != "*** Begin Patch");
    if lines.next().is_none() {
        return Err("no `*** Begin Patch` line".to_owned());
    }
    let mut ops = Vec::new();
    let mut open = Open::None;
    for line in lines {
        if line.trim() == "*** End Patch" {
            open.close(&mut ops);
            return if ops.is_empty() { Err("empty patch".to_owned()) } else { Ok(ops) };
        }
        if let Some(path) = marker(line, "*** Add File:") {
            std::mem::replace(&mut open, Open::Add { path: path.to_owned(), lines: Vec::new() })
                .close(&mut ops);
        } else if let Some(path) = marker(line, "*** Delete File:") {
            std::mem::replace(&mut open, Open::None).close(&mut ops);
            ops.push(FileOp::Delete { path: path.to_owned() });
        } else if let Some(path) = marker(line, "*** Update File:") {
            let next = Open::Update { path: path.to_owned(), move_to: None, hunks: Hunks::default() };
            std::mem::replace(&mut open, next).close(&mut ops);
        } else if let Some(dest) = marker(line, "*** Move to:") {
            let Open::Update { move_to, .. } = &mut open else {
                return Err("`*** Move to` outside an `*** Update File`".to_owned());
            };
            *move_to = Some(dest.to_owned());
        } else {
            match &mut open {
                Open::Add { lines, .. } => match line.strip_prefix('+') {
                    Some(rest) => lines.push(rest.to_owned()),
                    None => return Err(format!("added file line without `+`: {line}")),
                },
                Open::Update { hunks, .. } => hunks.take(line)?,
                Open::None if line.trim().is_empty() => {}
                Open::None => return Err(format!("line outside a file section: {line}")),
            }
        }
    }
    Err("no `*** End Patch` line".to_owned())
}

fn patch_text(payload: &Value) -> Option<&str> {
    let input = payload.get("tool_input")?;
    input
        .as_str()
        .or_else(|| input.get("command").and_then(Value::as_str))
        .or_else(|| input.get("input").and_then(Value::as_str))
        .or_else(|| input.get("patch").and_then(Value::as_str))
}

fn reshaped(payload: &Value, tool: &str, input: Value) -> Value {
    let mut out = payload.clone();
    out["tool_name"] = json!(tool);
    out["tool_input"] = input;
    out
}

/// The payloads the guard runs on. `None`: not an `apply_patch` call, run the
/// guard on the original payload.
///
/// # Errors
/// An `apply_patch` call whose patch does not parse.
pub fn claude_payloads(payload: &Value, tools: Tools) -> Result<Option<Vec<Value>>, String> {
    if payload.get("tool_name").and_then(Value::as_str) != Some("apply_patch") {
        return Ok(None);
    }
    let text = patch_text(payload).ok_or("apply_patch payload carries no patch text")?;
    let mut out = Vec::new();
    for op in parse_patch(text)?.iter().filter(|op| tools.keeps(op)) {
        match op {
            FileOp::Add { path, content } => out.push(reshaped(
                payload,
                "Write",
                json!({ "file_path": path, "content": content }),
            )),
            FileOp::Delete { path } => {
                out.push(reshaped(payload, "Write", json!({ "file_path": path, "content": "" })));
            }
            FileOp::Update { path, move_to, old, new } => {
                for file in std::iter::once(path).chain(move_to) {
                    out.push(reshaped(
                        payload,
                        "Edit",
                        json!({ "file_path": file, "old_string": old, "new_string": new }),
                    ));
                }
            }
        }
    }
    Ok(Some(out))
}

/// A guard's stdout with any `ask` decision turned into `deny`, and whether
/// it decided `deny`.
#[must_use]
pub fn deny_not_ask(stdout: &str) -> (String, bool) {
    let Ok(mut v) = serde_json::from_str::<Value>(stdout.trim()) else {
        return (stdout.to_owned(), false);
    };
    let Some(decision) = v.pointer_mut("/hookSpecificOutput/permissionDecision") else {
        return (stdout.to_owned(), false);
    };
    if *decision == "ask" {
        *decision = json!("deny");
        return (v.to_string(), true);
    }
    let denied = *decision == "deny";
    (stdout.to_owned(), denied)
}

/// What one guard run decided, ready to be written out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    fn blocked(reason: &str) -> Self {
        Self { code: BLOCK, stdout: String::new(), stderr: format!("cctui hook-shim: {reason}\n") }
    }

    const fn stops(&self, denied: bool) -> bool {
        self.code == BLOCK || denied
    }
}

fn run_guard(command: &str, payload: &Value) -> Outcome {
    let child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(err) => return Outcome::blocked(&format!("cannot start the guard: {err}")),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.to_string().as_bytes());
    }
    match child.wait_with_output() {
        Ok(out) => Outcome {
            code: out.status.code().unwrap_or(BLOCK),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        Err(err) => Outcome::blocked(&format!("guard did not finish: {err}")),
    }
}

/// Run `guard` on every payload `stdin` reshapes into; stop at the first block.
#[must_use]
pub fn decide(stdin: &str, tools: Tools, guard: impl Fn(&Value) -> Outcome) -> Outcome {
    let payload: Value = match serde_json::from_str(stdin) {
        Ok(v) => v,
        Err(err) => return Outcome::blocked(&format!("hook payload is not JSON: {err}")),
    };
    let payloads = match claude_payloads(&payload, tools) {
        Ok(Some(payloads)) => payloads,
        Ok(None) => vec![payload],
        Err(err) => return Outcome::blocked(&format!("unreadable apply_patch: {err}")),
    };
    let mut last = Outcome { code: 0, stdout: String::new(), stderr: String::new() };
    for payload in &payloads {
        let mut out = guard(payload);
        let (stdout, denied) = deny_not_ask(&out.stdout);
        out.stdout = stdout;
        if out.stops(denied) {
            return out;
        }
        if out.code != 0 || last.code == 0 {
            last = out;
        }
    }
    last
}

#[must_use]
pub fn run(cmd: &ShimCmd) -> i32 {
    let ShimCmd::ApplyPatch { tools, run: guard } = cmd;
    let mut stdin = String::new();
    let out = match std::io::stdin().read_to_string(&mut stdin) {
        Ok(_) => decide(&stdin, *tools, |payload| run_guard(guard, payload)),
        Err(err) => Outcome::blocked(&format!("cannot read the hook payload: {err}")),
    };
    let _ = std::io::stdout().write_all(out.stdout.as_bytes());
    let _ = std::io::stderr().write_all(out.stderr.as_bytes());
    out.code
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    const MULTI: &str = "*** Begin Patch
*** Add File: src/new.rs
+fn a() {}
+
*** Update File: src/lib.rs
@@ fn main
 keep
-old line
+new line
@@
-second
+changed
*** End of File
*** Delete File: src/gone.rs
*** Update File: a.txt
*** Move to: b.txt
@@
-x
+y
*** End Patch
";

    fn apply_patch(patch: &str) -> Value {
        json!({
            "session_id": "s1",
            "hook_event_name": "PreToolUse",
            "cwd": "/w",
            "tool_name": "apply_patch",
            "tool_input": { "command": patch },
        })
    }

    fn ok(stdout: &str) -> Outcome {
        Outcome { code: 0, stdout: stdout.to_owned(), stderr: String::new() }
    }

    #[test]
    fn a_multi_file_patch_parses_one_op_per_file_section() {
        let ops = parse_patch(MULTI).unwrap();
        assert_eq!(
            ops,
            vec![
                FileOp::Add { path: "src/new.rs".into(), content: "fn a() {}\n\n".into() },
                FileOp::Update {
                    path: "src/lib.rs".into(),
                    move_to: None,
                    old: "keep\nold line\n\nsecond".into(),
                    new: "keep\nnew line\n\nchanged".into(),
                },
                FileOp::Delete { path: "src/gone.rs".into() },
                FileOp::Update {
                    path: "a.txt".into(),
                    move_to: Some("b.txt".into()),
                    old: "x".into(),
                    new: "y".into(),
                },
            ]
        );
    }

    #[test]
    fn a_text_that_is_not_a_patch_is_an_error() {
        assert!(parse_patch("rm -rf /").is_err());
        assert!(parse_patch("*** Begin Patch\n*** Add File: a\n+x\n").is_err());
        assert!(parse_patch("*** Begin Patch\n*** End Patch\n").is_err());
        assert!(parse_patch("*** Begin Patch\n*** Add File: a\nno plus\n*** End Patch").is_err());
        assert!(parse_patch("*** Begin Patch\n*** Move to: b\n*** End Patch").is_err());
    }

    #[test]
    fn an_added_file_becomes_a_claude_write() {
        let patch = "*** Begin Patch\n*** Add File: /w/x.txt\n+hello\n*** End Patch";
        let got = claude_payloads(&apply_patch(patch), Tools::Any).unwrap().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["tool_name"], "Write");
        assert_eq!(got[0]["tool_input"], json!({ "file_path": "/w/x.txt", "content": "hello\n" }));
        assert_eq!(got[0]["session_id"], "s1");
        assert_eq!(got[0]["hook_event_name"], "PreToolUse");
    }

    #[test]
    fn an_updated_file_becomes_a_claude_edit() {
        let patch = "*** Begin Patch\n*** Update File: m.rs\n@@\n-a\n+b\n*** End Patch";
        let got = claude_payloads(&apply_patch(patch), Tools::Any).unwrap().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["tool_name"], "Edit");
        assert_eq!(
            got[0]["tool_input"],
            json!({ "file_path": "m.rs", "old_string": "a", "new_string": "b" })
        );
    }

    #[test]
    fn a_deleted_file_becomes_an_empty_claude_write() {
        let patch = "*** Begin Patch\n*** Delete File: secrets.env\n*** End Patch";
        let got = claude_payloads(&apply_patch(patch), Tools::Any).unwrap().unwrap();
        assert_eq!(got[0]["tool_name"], "Write");
        assert_eq!(got[0]["tool_input"], json!({ "file_path": "secrets.env", "content": "" }));
    }

    #[test]
    fn a_multi_file_patch_yields_one_payload_per_file_and_tools_filter_them() {
        let names = |tools| {
            claude_payloads(&apply_patch(MULTI), tools)
                .unwrap()
                .unwrap()
                .iter()
                .map(|p| {
                    format!(
                        "{} {}",
                        p["tool_name"].as_str().unwrap(),
                        p["tool_input"]["file_path"].as_str().unwrap()
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(Tools::Any),
            ["Write src/new.rs", "Edit src/lib.rs", "Write src/gone.rs", "Edit a.txt", "Edit b.txt"]
        );
        assert_eq!(names(Tools::Edit), ["Edit src/lib.rs", "Edit a.txt", "Edit b.txt"]);
        assert_eq!(names(Tools::Write), ["Write src/new.rs", "Write src/gone.rs"]);
    }

    #[test]
    fn other_tools_pass_through_unchanged() {
        let bash = json!({ "tool_name": "Bash", "tool_input": { "command": "ls" } });
        assert_eq!(claude_payloads(&bash, Tools::Any), Ok(None));
        let seen = RefCell::new(Vec::new());
        let out = decide(&bash.to_string(), Tools::Edit, |p| {
            seen.borrow_mut().push(p.clone());
            ok("")
        });
        assert_eq!(out.code, 0);
        assert_eq!(seen.into_inner(), vec![bash]);
    }

    #[test]
    fn the_guard_runs_once_per_file_and_the_first_block_wins() {
        let calls = RefCell::new(0);
        let out = decide(&apply_patch(MULTI).to_string(), Tools::Any, |p| {
            *calls.borrow_mut() += 1;
            if p["tool_input"]["file_path"] == "src/gone.rs" {
                Outcome { code: BLOCK, stdout: String::new(), stderr: "no deletes".into() }
            } else {
                ok("")
            }
        });
        assert_eq!(out.code, BLOCK);
        assert_eq!(out.stderr, "no deletes");
        assert_eq!(calls.into_inner(), 3);
    }

    #[test]
    fn a_json_deny_stops_and_is_forwarded() {
        let deny = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"nope"}}"#;
        let calls = RefCell::new(0);
        let out = decide(&apply_patch(MULTI).to_string(), Tools::Any, |_| {
            *calls.borrow_mut() += 1;
            ok(deny)
        });
        assert_eq!(calls.into_inner(), 1);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, deny);
    }

    #[test]
    fn a_guard_ask_is_forwarded_as_deny() {
        let ask = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"check"}}"#;
        let out = decide(&apply_patch(MULTI).to_string(), Tools::Any, |_| ok(ask));
        let v: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(v["hookSpecificOutput"]["permissionDecisionReason"], "check");
        assert!(!out.stdout.contains("\"ask\""), "{}", out.stdout);
    }

    #[test]
    fn the_shim_never_emits_ask_and_its_own_failures_block() {
        let ask = r#"{"hookSpecificOutput":{"permissionDecision":"ask"}}"#;
        for stdin in [
            "not json".to_owned(),
            apply_patch("garbage").to_string(),
            json!({ "tool_name": "apply_patch", "tool_input": {} }).to_string(),
        ] {
            let out = decide(&stdin, Tools::Any, |_| ok(ask));
            assert_eq!(out.code, BLOCK, "{stdin}");
            assert!(out.stderr.starts_with("cctui hook-shim:"), "{}", out.stderr);
            assert!(!out.stdout.contains("ask") && !out.stderr.contains("\"ask\""));
        }
        let bash = json!({ "tool_name": "Bash", "tool_input": { "command": "ls" } });
        let out = decide(&bash.to_string(), Tools::Any, |_| ok(ask));
        assert!(!out.stdout.contains("\"ask\""), "{}", out.stdout);
    }

    #[test]
    fn a_patch_with_nothing_for_the_rule_runs_no_guard() {
        let patch = "*** Begin Patch\n*** Update File: m.rs\n@@\n-a\n+b\n*** End Patch";
        let out =
            decide(&apply_patch(patch).to_string(), Tools::Write, |_| panic!("guard must not run"));
        assert_eq!(out.code, 0);
    }

    #[test]
    fn a_real_guard_receives_the_claude_payload_on_stdin() {
        let patch = "*** Begin Patch\n*** Add File: /w/blocked.txt\n+x\n*** End Patch";
        let guard = r#"p=$(cat)
case "$p" in *'"tool_name":"Write"'*) ;; *) exit 0 ;; esac
case "$p" in *blocked.txt*) echo denied >&2; exit 2 ;; esac
exit 0"#;
        let out = decide(&apply_patch(patch).to_string(), Tools::Any, |p| run_guard(guard, p));
        assert_eq!(out.code, BLOCK, "{out:?}");
        assert_eq!(out.stderr, "denied\n");
    }
}
