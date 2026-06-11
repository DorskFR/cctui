//! Rule evaluation: Bash segment splitting, git-flag normalization, and the
//! allow/disallow keyword matcher.

use serde_json::Value;

/// Built-in Claude Code tool names. These keywords denote *tools*, not shell
/// phrases, so they must only match a tool call by identity — never as a
/// substring of a Bash command's text. Without this guard the bare keyword
/// `Write` (from the `code-write` set) would substring-match shell text like
/// "URL rewrite", and `Edit` would match "edited"/"credit", wrongly denying
/// legitimate Bash commands in any step that disallows `code-write`.
const BUILTIN_TOOL_KEYWORDS: &[&str] = &[
    "read",
    "grep",
    "glob",
    "lsp",
    "webfetch",
    "websearch",
    "edit",
    "write",
    "notebookedit",
    "task",
    "agent",
    "toolsearch",
    "todowrite",
];

fn is_builtin_tool_keyword(kw: &str) -> bool {
    let lower = kw.to_ascii_lowercase();
    BUILTIN_TOOL_KEYWORDS.contains(&lower.as_str())
}

/// Split a Bash command on shell operators (`&&`, `||`, `;`, `|`) into
/// individual segments, respecting single/double quotes. Each segment is
/// trimmed. Returns `[cmd]` if no operators split it.
#[must_use]
pub fn split_bash_segments(cmd: &str) -> Vec<String> {
    let chars: Vec<char> = cmd.chars().collect();
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;

    let push_seg = |current: &mut String, segments: &mut Vec<String>| {
        let seg = current.trim().to_string();
        if !seg.is_empty() {
            segments.push(seg);
        }
        current.clear();
    };

    while i < chars.len() {
        let c = chars[i];

        if c == '\'' && !in_double {
            in_single = !in_single;
            current.push(c);
        } else if c == '"' && !in_single {
            in_double = !in_double;
            current.push(c);
        } else if !in_single && !in_double {
            if c == ';' || c == '|' {
                if c == '|' && i + 1 < chars.len() && chars[i + 1] == '|' {
                    push_seg(&mut current, &mut segments);
                    i += 2;
                    continue;
                }
                push_seg(&mut current, &mut segments);
            } else if c == '&' && i + 1 < chars.len() && chars[i + 1] == '&' {
                push_seg(&mut current, &mut segments);
                i += 2;
                continue;
            } else {
                current.push(c);
            }
        } else {
            current.push(c);
        }

        i += 1;
    }

    let seg = current.trim().to_string();
    if !seg.is_empty() {
        segments.push(seg);
    }

    if segments.is_empty() { vec![cmd.to_string()] } else { segments }
}

/// Normalize a Bash segment so phrase keywords match real-world invocations.
///
/// Strips git's global option flags that sit between `git` and the subcommand
/// (e.g. `git -C /workspace/repo fetch` → `git fetch`,
/// `git -c k=v --no-pager log` → `git log`), so allowlist phrases like
/// `git fetch` match regardless of how the working directory or config is
/// passed. Mirrors the Python regex:
/// `^(git)(\s+(?:-C\s+\S+|-c\s+\S+|--no-pager|--git-dir[= ]\S+|--work-tree[= ]\S+))+\s+`
#[must_use]
pub fn normalize_bash_segment(seg: &str) -> String {
    let seg = seg.trim();
    let rest = match seg.strip_prefix("git") {
        Some(r) if r.starts_with(char::is_whitespace) => r,
        _ => return seg.to_string(),
    };

    // Consume one-or-more global-flag groups. Track whether we consumed at least
    // one, and where the subcommand begins.
    let mut remainder = rest;
    let mut consumed_any = false;

    loop {
        let trimmed = remainder.trim_start();
        let ws_len = remainder.len() - trimmed.len();
        if ws_len == 0 {
            // No whitespace before next token: cannot be a flag group boundary.
            break;
        }
        if let Some(after) = consume_git_flag(trimmed) {
            consumed_any = true;
            remainder = after;
        } else {
            // The next token is the subcommand. `remainder` still has its
            // leading whitespace (the `\s+` before the subcommand in the regex).
            break;
        }
    }

    if !consumed_any {
        return seg.to_string();
    }

    // The regex requires trailing `\s+` then leaves the subcommand. If what
    // remains after the flags has no whitespace separator + token, the pattern
    // would not have matched; fall back to the original.
    let subcommand = remainder.trim_start();
    if remainder.len() == subcommand.len() || subcommand.is_empty() {
        return seg.to_string();
    }
    format!("git {subcommand}")
}

/// If `s` begins with a single git global-flag token, return the slice after it.
/// Handles: `-C <arg>`, `-c <arg>`, `--no-pager`, `--git-dir[= ]<arg>`,
/// `--work-tree[= ]<arg>`.
fn consume_git_flag(s: &str) -> Option<&str> {
    let next_token_arg = |after_flag: &str| -> Option<usize> {
        // after_flag begins right after the flag name; expect whitespace then a
        // non-whitespace argument (\S+). Return offset (within s) past the arg.
        let trimmed = after_flag.trim_start();
        let ws = after_flag.len() - trimmed.len();
        if ws == 0 {
            return None;
        }
        let arg_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        if arg_end == 0 {
            return None;
        }
        Some((s.len() - after_flag.len()) + ws + arg_end)
    };

    if let Some(after) = s.strip_prefix("--no-pager") {
        // Must be a full token (followed by whitespace or end).
        if after.is_empty() || after.starts_with(char::is_whitespace) {
            return Some(after);
        }
        return None;
    }
    for prefix in ["--git-dir", "--work-tree"] {
        if let Some(after) = s.strip_prefix(prefix) {
            // `=<arg>` or ` <arg>`.
            if let Some(eq_rest) = after.strip_prefix('=') {
                let end = eq_rest.find(char::is_whitespace).unwrap_or(eq_rest.len());
                if end == 0 {
                    return None;
                }
                return Some(&eq_rest[end..]);
            }
            if let Some(end) = next_token_arg(after) {
                return Some(&s[end..]);
            }
            return None;
        }
    }
    for prefix in ["-C", "-c"] {
        if let Some(after) = s.strip_prefix(prefix) {
            // `-C` / `-c` take a following whitespace-separated argument.
            if let Some(end) = next_token_arg(after) {
                return Some(&s[end..]);
            }
            return None;
        }
    }
    None
}

/// Check a single match string against allowed/disallowed keyword lists.
/// Returns `(is_allowed, reason)`.
fn check_single(match_str: &str, allowed: &[String], disallowed: &[String]) -> (bool, String) {
    let contains = |kw: &str| match_str.to_ascii_lowercase().contains(&kw.to_ascii_lowercase());
    let has_wildcard = |v: &[String]| v.iter().any(|s| s == "*");

    if !disallowed.is_empty() {
        if has_wildcard(disallowed) {
            if !allowed.is_empty() && !has_wildcard(allowed) {
                for kw in allowed {
                    if contains(kw) {
                        return (true, String::new());
                    }
                }
            } else if has_wildcard(allowed) {
                return (false, "All tools blocked in this step".to_string());
            }
            let parts: Vec<&str> = match_str.split_whitespace().collect();
            let label = if parts.len() > 1 {
                parts[1]
            } else if let Some(first) = parts.first() {
                first
            } else {
                match_str
            };
            return (false, format!("'{label}' not in allowed list"));
        }

        for kw in disallowed {
            if contains(kw) {
                return (false, format!("'{kw}' is disallowed in this step"));
            }
        }
    }

    if !allowed.is_empty() {
        if has_wildcard(allowed) {
            return (true, String::new());
        }
        for kw in allowed {
            if contains(kw) {
                return (true, String::new());
            }
        }
        return (false, "Tool not in allowed list for this step".to_string());
    }

    (true, String::new())
}

/// Build a string representation of a tool call for keyword matching.
fn build_match_string(tool: &str, tool_input: &Value) -> String {
    if tool == "Bash" {
        let cmd = tool_input.get("command").and_then(Value::as_str).unwrap_or("");
        format!("Bash {cmd}")
    } else if tool.starts_with("mcp__") {
        let input = serde_json::to_string(tool_input).unwrap_or_else(|_| "{}".to_string());
        format!("mcp {tool} {input}")
    } else {
        let file_path = tool_input.get("file_path").and_then(Value::as_str).unwrap_or("");
        format!("{tool} {file_path}")
    }
}

/// Check whether a tool call is permitted under the current step's rules.
/// Returns `(is_allowed, reason)`.
///
/// For Bash commands, splits on shell operators and checks each segment; every
/// segment must pass. Built-in tool-name keywords are stripped from the lists
/// when evaluating Bash so they cannot substring-collide with command text.
#[must_use]
pub fn check_rules(
    tool: &str,
    tool_input: &Value,
    allowed: &[String],
    disallowed: &[String],
) -> (bool, String) {
    if tool == "Bash" {
        let allowed: Vec<String> =
            allowed.iter().filter(|kw| !is_builtin_tool_keyword(kw)).cloned().collect();
        let disallowed: Vec<String> =
            disallowed.iter().filter(|kw| !is_builtin_tool_keyword(kw)).cloned().collect();
        let cmd = tool_input.get("command").and_then(Value::as_str).unwrap_or("");
        for seg in split_bash_segments(cmd) {
            let match_str = format!("Bash {}", normalize_bash_segment(&seg));
            let (ok, reason) = check_single(&match_str, &allowed, &disallowed);
            if !ok {
                return (false, reason);
            }
        }
        return (true, String::new());
    }

    let match_str = build_match_string(tool, tool_input);
    check_single(&match_str, allowed, disallowed)
}
