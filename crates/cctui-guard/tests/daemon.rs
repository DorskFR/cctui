//! Port of `workflow-guard/test_daemon.py` — parser, splitter, and rule-eval
//! tests plus engine-level allow/deny scenarios.

use std::collections::HashMap;

use cctui_guard::engine::WorkflowEngine;
use cctui_guard::parser::{parse_guard_rules_str, parse_keywords, parse_steps, parse_transitions};
use cctui_guard::rules::{check_rules, split_bash_segments};
use serde_json::json;

fn sets(pairs: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.iter().map(|s| (*s).to_string()).collect()))
        .collect()
}

fn no_sets() -> HashMap<String, Vec<String>> {
    HashMap::new()
}

fn kw(rule: &str) -> Vec<String> {
    parse_keywords(rule, &no_sets())
}

fn strvec(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

// --- Markdown parsing ---

const SAMPLE_MD: &str = r"
! Read CLAUDE.md

## Workflow

### Step 1: Research the task

- Read TRIAGE.md
- Explore codebase

[disallowed]: git push, gh, execute_command
[allowed]: *
[transition]: 2, Exit

### Step 2: Pick up task

- Assign yourself
- Checkout branch

[allowed]: git checkout, execute_command
[disallowed]: *
[transition]: 3

# Step 10: Finalize

- Mark PR ready

[allowed]: gh pr ready, Edit
[disallowed]: *
[transition]: Exit
";

#[test]
fn test_parse_steps() {
    let steps = parse_steps(SAMPLE_MD).unwrap();
    let keys: Vec<u32> = steps.keys().copied().collect();
    assert_eq!(keys, vec![1, 2, 10], "parse_steps: step numbers");
    assert_eq!(steps[&1].title, "Research the task");
    assert_eq!(steps[&2].title, "Pick up task");
    assert_eq!(steps[&10].title, "Finalize", "step 10 title (single #)");
    assert_eq!(steps[&1].allowed, "*");
    assert_eq!(steps[&1].disallowed, "git push, gh, execute_command");
    assert_eq!(steps[&2].transition, "3");
}

#[test]
fn test_parse_steps_body_and_gate() {
    // CCT-440: the prose body (non-annotation lines) is captured for re-injection,
    // and a `[gate]` annotation carries the deterministic completion check.
    let md = "# Step 1: Implement\n\
              Make the change.\n\
              Run the tests.\n\
              [allowed]: *\n\
              [gate]: cargo test\n\
              [transition]: 2\n";
    let steps = parse_steps(md).unwrap();
    assert_eq!(steps[&1].body, "Make the change.\nRun the tests.", "body excludes annotations");
    assert_eq!(steps[&1].gate, "cargo test", "gate captured");

    // No gate ⇒ empty (trusted transition, as before).
    let steps2 = parse_steps("# Step 1: X\nbody\n[allowed]: *\n[transition]: Exit").unwrap();
    assert_eq!(steps2[&1].gate, "");
    assert_eq!(steps2[&1].body, "body");
}

#[test]
fn test_parse_steps_case_insensitive() {
    let steps = parse_steps("### STEP 5: Upper case\n[allowed]: *\n[transition]: 6").unwrap();
    assert!(steps.contains_key(&5), "case insensitive STEP");

    let steps2 = parse_steps("## step 3: lower case\n[allowed]: *\n[transition]: 4").unwrap();
    assert!(steps2.contains_key(&3), "case insensitive step");
}

#[test]
fn test_parse_steps_various_heading_levels() {
    for level in 1..=6 {
        let prefix = "#".repeat(level);
        let md = format!("{prefix} Step 1: Level {level}\n[allowed]: *\n[transition]: Exit");
        let steps = parse_steps(&md).unwrap();
        assert!(steps.contains_key(&1), "heading level {level}");
    }
}

#[test]
fn test_parse_transitions() {
    let (nums, has_exit) = parse_transitions("2, Exit");
    assert_eq!(nums, vec![2]);
    assert!(has_exit);

    let (nums, has_exit) = parse_transitions("Step 9, Step 11");
    assert_eq!(nums, vec![9, 11]);
    assert!(!has_exit);

    let (nums, has_exit) = parse_transitions("Exit");
    assert_eq!(nums, Vec::<u32>::new());
    assert!(has_exit);

    let (nums, has_exit) = parse_transitions("3");
    assert_eq!(nums, vec![3]);
    assert!(!has_exit);

    let (nums, _) = parse_transitions("10, 12");
    assert_eq!(nums, vec![10, 12]);
}

#[test]
fn test_parse_keywords() {
    assert_eq!(kw("*"), strvec(&["*"]), "wildcard");
    assert_eq!(kw(""), Vec::<String>::new(), "empty");
    assert_eq!(kw("git push, gh"), strvec(&["git push", "gh"]), "comma");
    assert_eq!(kw("  git push ,  gh  "), strvec(&["git push", "gh"]), "with spaces");
}

// --- Bash segment splitting ---

#[test]
fn test_split_bash_segments() {
    assert_eq!(
        split_bash_segments("cd /workspace && git push"),
        strvec(&["cd /workspace", "git push"])
    );
    assert_eq!(
        split_bash_segments("echo hello; echo world"),
        strvec(&["echo hello", "echo world"])
    );
    assert_eq!(
        split_bash_segments("cat file | grep pattern"),
        strvec(&["cat file", "grep pattern"])
    );
    assert_eq!(split_bash_segments("cmd1 || cmd2"), strvec(&["cmd1", "cmd2"]));
    assert_eq!(split_bash_segments("git push"), strvec(&["git push"]));
    assert_eq!(
        split_bash_segments("sleep 60 && git push --force && echo done"),
        strvec(&["sleep 60", "git push --force", "echo done"])
    );
}

#[test]
fn test_split_respects_quotes() {
    assert_eq!(
        split_bash_segments(r#"echo "hello && world""#),
        strvec(&[r#"echo "hello && world""#])
    );
    assert_eq!(split_bash_segments("echo 'hello && world'"), strvec(&["echo 'hello && world'"]));
    assert_eq!(
        split_bash_segments(r#"echo "a;b" && echo c"#),
        strvec(&[r#"echo "a;b""#, "echo c"])
    );
}

#[test]
fn test_split_for_loop() {
    assert_eq!(
        split_bash_segments("for i in 1 2 3; do echo $i; done"),
        strvec(&["for i in 1 2 3", "do echo $i", "done"])
    );
}

// --- Rule evaluation ---

fn ok(tool: &str, input: &serde_json::Value, allowed: &[&str], disallowed: &[&str]) -> bool {
    check_rules(tool, input, &strvec(allowed), &strvec(disallowed)).0
}

#[test]
fn test_wildcard_allow() {
    assert!(ok("Read", &json!({}), &["*"], &[]));
    assert!(ok("Bash", &json!({"command": "rm -rf /"}), &["*"], &[]));
}

#[test]
fn test_wildcard_disallow() {
    assert!(!ok("Read", &json!({}), &[], &["*"]));
}

#[test]
fn test_disallow_takes_precedence() {
    assert!(!ok("Read", &json!({}), &["*"], &["*"]));
}

#[test]
fn test_specific_disallow() {
    assert!(!ok("Bash", &json!({"command": "git push --force"}), &["*"], &["git push"]));
    assert!(ok("Bash", &json!({"command": "git commit -m 'hello'"}), &["*"], &["git push"]));
}

#[test]
fn test_specific_allow_with_wildcard_disallow() {
    assert!(ok("Bash", &json!({"command": "git checkout -b branch"}), &["git checkout"], &["*"]));
    assert!(!ok("Bash", &json!({"command": "rm -rf /"}), &["git checkout"], &["*"]));
}

#[test]
fn test_compound_command_split() {
    assert!(!ok(
        "Bash",
        &json!({"command": "cd /workspace && rm -rf /"}),
        &["cd", "git checkout"],
        &["*"]
    ));
    assert!(!ok("Bash", &json!({"command": "sleep 60 && git push"}), &["*"], &["git push"]));
    assert!(ok(
        "Bash",
        &json!({"command": "cd /workspace && git checkout -b foo"}),
        &["cd", "git checkout"],
        &["*"]
    ));
}

#[test]
fn test_git_global_flags_normalized() {
    assert!(ok(
        "Bash",
        &json!({"command": "git -C /workspace/acme fetch origin"}),
        &["git fetch"],
        &[]
    ));
    assert!(ok(
        "Bash",
        &json!({"command": "git -c core.pager=cat --no-pager log --oneline"}),
        &["git log"],
        &[]
    ));
    assert!(!ok(
        "Bash",
        &json!({"command": "git -C /workspace/acme push origin main"}),
        &["*"],
        &["git push"]
    ));
    assert!(ok(
        "Bash",
        &json!({"command": "git -C /workspace status && git -C /workspace fetch"}),
        &["git status", "git fetch"],
        &[]
    ));
}

#[test]
fn test_mcp_tool_matching() {
    assert!(ok(
        "mcp__youtrack__get_issue",
        &json!({"issueId": "PROJ-123"}),
        &["*"],
        &["execute_command"]
    ));
    assert!(!ok(
        "mcp__youtrack__execute_command",
        &json!({"command": "for me"}),
        &["*"],
        &["execute_command"]
    ));
}

#[test]
fn test_mcp_allow_specific() {
    assert!(ok(
        "mcp__youtrack__execute_command",
        &json!({"command": "for me"}),
        &["execute_command"],
        &["*"]
    ));
    assert!(!ok(
        "mcp__youtrack__get_issue",
        &json!({"issueId": "PROJ-123"}),
        &["execute_command"],
        &["*"]
    ));
}

#[test]
fn test_builtin_tool_matching() {
    assert!(ok("Edit", &json!({"file_path": "/tmp/foo.txt"}), &["Edit", "Read"], &["*"]));
    assert!(!ok("Write", &json!({"file_path": "/tmp/foo.txt"}), &["Edit", "Read"], &["*"]));
}

#[test]
fn test_builtin_keyword_no_bash_substring_collision() {
    let allowed = &["Read", "Grep", "Glob", "Bash", "git diff"];
    let disallowed = &["Edit", "Write", "git push"];

    assert!(ok("Bash", &json!({"command": "gh pr diff 5 | grep rewrite"}), allowed, disallowed));
    assert!(ok(
        "Bash",
        &json!({"command": "tee ctx.md <<'EOF'\nURL rewrite logic\nEOF"}),
        allowed,
        disallowed
    ));
    assert!(ok("Bash", &json!({"command": "echo 'recently edited file'"}), allowed, disallowed));
    assert!(!ok("Bash", &json!({"command": "git push origin main"}), allowed, disallowed));
    assert!(!ok("Bash", &json!({"command": "gh pr edit 5 --title x"}), &["Bash"], &["gh pr edit"]));
    assert!(!ok("Write", &json!({"file_path": "/tmp/foo.txt"}), &["Read"], &["Write"]));
}

#[test]
fn test_no_rules_allows() {
    assert!(ok("Bash", &json!({"command": "anything"}), &[], &[]));
}

// --- Tool set expansion ---

#[test]
fn test_parse_keywords_with_tool_sets() {
    let s = sets(&[
        ("exploration", &["Read", "Grep", "Glob", "Bash"]),
        ("coding", &["Read", "Edit", "Write"]),
    ]);
    assert_eq!(
        parse_keywords("exploration, gh", &s),
        strvec(&["Read", "Grep", "Glob", "Bash", "gh"])
    );
    assert_eq!(parse_keywords("*", &s), strvec(&["*"]));
    assert_eq!(parse_keywords("coding", &s), strvec(&["Read", "Edit", "Write"]));
    assert_eq!(parse_keywords("unknown_set", &s), strvec(&["unknown_set"]));
}

#[test]
fn test_parse_keywords_recursive_expansion() {
    let s = sets(&[
        ("code-read", &["Read", "Grep", "Glob"]),
        ("git-read", &["git log", "git diff"]),
        ("exploration", &["code-read", "git-read", "WebSearch"]),
    ]);
    assert_eq!(
        parse_keywords("exploration", &s),
        strvec(&["Read", "Grep", "Glob", "git log", "git diff", "WebSearch"])
    );

    let circular = sets(&[("a", &["b", "Read"]), ("b", &["a", "Write"])]);
    let result = parse_keywords("a", &circular);
    assert!(result.contains(&"Read".to_string()), "circular includes Read");
    assert!(result.contains(&"Write".to_string()), "circular includes Write");
    assert!(
        !result.contains(&"a".to_string()) || !result.contains(&"b".to_string()),
        "circular: no infinite loop"
    );
}

// --- Guard rules parsing ---

const SAMPLE_GUARD_RULES: &str = "\
# Guard Rules

[code-read]: Read, Grep, Glob, LSP, WebFetch, WebSearch
[code-write]: Edit, Write
[git-read]: git log, git diff
[git-write]: git checkout, git commit, git push
[github-read]: gh pr list, gh pr view, gh api
[github-write]: gh pr create, gh pr edit, git push
[all-read]: code-read, git-read, github-read
[all-write]: code-write, git-write, github-write
[remote-write]: git push, github-write
";

#[test]
fn test_parse_guard_rules() {
    let tool_sets = parse_guard_rules_str(SAMPLE_GUARD_RULES);
    assert_eq!(tool_sets.len(), 9, "9 tool sets");
    assert_eq!(
        tool_sets["code-read"],
        strvec(&["Read", "Grep", "Glob", "LSP", "WebFetch", "WebSearch"])
    );
    assert_eq!(tool_sets["code-write"], strvec(&["Edit", "Write"]));
    assert_eq!(tool_sets["github-read"], strvec(&["gh pr list", "gh pr view", "gh api"]));
    assert_eq!(
        tool_sets["all-read"],
        strvec(&["code-read", "git-read", "github-read"]),
        "all-read set (unexpanded)"
    );
    assert_eq!(tool_sets["remote-write"], strvec(&["git push", "github-write"]));
}

// --- Engine helpers ---

struct TestEngine {
    engine: WorkflowEngine,
    _dir: tempfile::TempDir,
}

fn make_engine(rules_text: &str, prompt_text: &str) -> TestEngine {
    let dir = tempfile::tempdir().unwrap();
    let state_file = dir.path().join("state");
    // No proxy dir → policy writes are skipped (matches Python guard on missing dir).
    let policy_file = dir.path().join("guard-proxy").join("policy.json");
    let steps = parse_steps(prompt_text).unwrap();
    let tool_sets = parse_guard_rules_str(rules_text);
    let gate_cwd = dir.path().to_path_buf();
    let engine = WorkflowEngine::new(
        steps,
        tool_sets,
        state_file,
        policy_file,
        vec![],
        gate_cwd,
        None,
        false,
    );
    TestEngine { engine, _dir: dir }
}

fn decision(resp: &serde_json::Value) -> String {
    resp["hookSpecificOutput"]["permissionDecision"].as_str().unwrap().to_string()
}

#[test]
fn test_tool_sets_in_step_rules() {
    let rules = "[code-read]: Read, Grep, Glob\n[git-read]: git log, git diff\n";
    let prompt = "### Step 1: Research\n[allowed]: code-read, git-read\n[disallowed]: *\n[transition]: Exit\n";
    let t = make_engine(rules, prompt);

    assert_eq!(decision(&t.engine.check("Read", &json!({"file_path": "/tmp/foo"}))), "allow");
    assert_eq!(
        decision(&t.engine.check("Bash", &json!({"command": "git log --oneline"}))),
        "allow"
    );
    assert_eq!(decision(&t.engine.check("Edit", &json!({"file_path": "/tmp/foo"}))), "deny");
    assert_eq!(
        decision(&t.engine.check("Bash", &json!({"command": "git push origin main"}))),
        "deny"
    );
    assert_eq!(decision(&t.engine.check("Bash", &json!({"command": "rm -rf /tmp/foo"}))), "deny");
}

#[test]
fn test_all_read_pattern() {
    let rules = "[code-read]: Read, Grep, Glob\n\
        [git-read]: git log, git diff, git pull\n\
        [github-read]: gh pr list, gh pr view, gh api\n\
        [all-read]: code-read, git-read, github-read\n";
    let prompt = "### Step 1: Research\n[allowed]: all-read, curl\n[transition]: Exit\n";
    let t = make_engine(rules, prompt);

    assert_eq!(decision(&t.engine.check("Read", &json!({"file_path": "/tmp/foo"}))), "allow");
    assert_eq!(decision(&t.engine.check("Grep", &json!({"pattern": "foo"}))), "allow");
    assert_eq!(
        decision(&t.engine.check("Bash", &json!({"command": "git log --oneline -10"}))),
        "allow"
    );
    assert_eq!(
        decision(&t.engine.check("Bash", &json!({"command": "git pull origin main"}))),
        "allow"
    );
    assert_eq!(
        decision(&t.engine.check("Bash", &json!({"command": "gh pr list --state open"}))),
        "allow"
    );
    assert_eq!(
        decision(&t.engine.check(
            "Bash",
            &json!({"command": "curl -sL https://example.com/file.png -o /tmp/file.png"})
        )),
        "allow"
    );
    assert_eq!(decision(&t.engine.check("Edit", &json!({"file_path": "/tmp/foo"}))), "deny");
    assert_eq!(decision(&t.engine.check("Write", &json!({"file_path": "/tmp/foo"}))), "deny");
    assert_eq!(
        decision(&t.engine.check("Bash", &json!({"command": "git push origin main"}))),
        "deny"
    );
    assert_eq!(
        decision(&t.engine.check("Bash", &json!({"command": "git commit -m 'test'"}))),
        "deny"
    );
    assert_eq!(
        decision(&t.engine.check("Bash", &json!({"command": "npm install express"}))),
        "deny"
    );
}

#[test]
fn test_local_dev_pattern() {
    let rules = "[code-read]: Read, Grep, Glob\n\
        [code-write]: Edit, Write\n\
        [git-read]: git log, git diff\n\
        [github-read]: gh pr list, gh pr view\n\
        [github-write]: gh pr create, gh pr edit, git push\n\
        [all-read]: code-read, git-read, github-read\n\
        [remote-write]: git push, github-write\n";
    let prompt = "### Step 1: Implement\n\
        [allowed]: all-read, code-write, Bash, git commit\n\
        [disallowed]: remote-write\n\
        [transition]: Exit\n";
    let t = make_engine(rules, prompt);

    assert_eq!(decision(&t.engine.check("Read", &json!({"file_path": "/tmp/foo"}))), "allow");
    assert_eq!(decision(&t.engine.check("Edit", &json!({"file_path": "/tmp/foo"}))), "allow");
    assert_eq!(decision(&t.engine.check("Bash", &json!({"command": "npm test"}))), "allow");
    assert_eq!(
        decision(&t.engine.check("Bash", &json!({"command": "git commit -m 'feat: add feature'"}))),
        "allow"
    );
    assert_eq!(
        decision(&t.engine.check("Bash", &json!({"command": "git push origin main"}))),
        "deny"
    );
    assert_eq!(
        decision(&t.engine.check("Bash", &json!({"command": "gh pr create --draft"}))),
        "deny"
    );
    assert_eq!(decision(&t.engine.check("Bash", &json!({"command": "gh pr list"}))), "allow");
}

// --- Network rule parsing and expansion ---

#[test]
fn test_network_rule_parsing() {
    let md = "
## Step 1: Read
[allowed]: all-read
[network]: net-anthropic, net-github
[transition]: 2

## Step 2: Write
[allowed]: all-read, code-write
[network]: net-anthropic, net-github, net-youtrack
[transition]: Exit
";
    let steps = parse_steps(md).unwrap();
    assert_eq!(steps[&1].network, "net-anthropic, net-github");
    assert_eq!(steps[&2].network, "net-anthropic, net-github, net-youtrack");
}

#[test]
fn test_proxy_policy_expansion() {
    // Neutralized hosts (repo is public).
    let rules = "
[net-anthropic]: api.anthropic.com:443
[net-github]: github.com:443, github.com:22
";
    let md = "
## Step 1: Read
[allowed]: *
[network]: net-anthropic, net-github
[transition]: Exit
";
    // Use a proxy dir that exists so policy writes happen and we can assert on it.
    let dir = tempfile::tempdir().unwrap();
    let proxy_dir = dir.path().join("guard-proxy");
    std::fs::create_dir_all(&proxy_dir).unwrap();
    let policy_file = proxy_dir.join("policy.json");
    let state_file = dir.path().join("state");

    let engine = WorkflowEngine::new(
        parse_steps(md).unwrap(),
        parse_guard_rules_str(rules),
        state_file,
        policy_file.clone(),
        vec![],
        dir.path().to_path_buf(),
        None,
        false,
    );

    // Engine initialized on step 1, which has [network]: net-anthropic, net-github.
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&policy_file).unwrap()).unwrap();
    let hosts: Vec<String> = written["allowed_hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(hosts.contains(&"api.anthropic.com:443".to_string()));
    assert!(hosts.contains(&"github.com:443".to_string()));
    assert!(hosts.contains(&"github.com:22".to_string()));
    assert_eq!(hosts.len(), 3, "exactly 3 hosts");
    assert_eq!(written["default"], "deny");
    drop(engine);
}

fn initial_policy(md: &str, rules: &str, guarded_default_allow: bool) -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    let proxy_dir = dir.path().join("guard-proxy");
    std::fs::create_dir_all(&proxy_dir).unwrap();
    let policy_file = proxy_dir.join("policy.json");
    let engine = WorkflowEngine::new(
        parse_steps(md).unwrap(),
        parse_guard_rules_str(rules),
        dir.path().join("state"),
        policy_file.clone(),
        vec!["callback.example.com:443".to_string()],
        dir.path().to_path_buf(),
        None,
        guarded_default_allow,
    );
    let v = serde_json::from_str(&std::fs::read_to_string(&policy_file).unwrap()).unwrap();
    drop(engine);
    v
}

#[test]
fn network_omitted_on_guarded_step_defaults_deny() {
    let md = "# Step 1: Work\n[allowed]: *\n[transition]: Exit\n";
    let p = initial_policy(md, "", false);
    assert_eq!(p["default"], "deny");
    let hosts: Vec<&str> =
        p["allowed_hosts"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(hosts, vec!["callback.example.com:443"], "only the always-allowed callback");
}

#[test]
fn network_wildcard_opens_egress() {
    let md = "# Step 1: Work\n[allowed]: *\n[network]: *\n[transition]: Exit\n";
    let p = initial_policy(md, "", false);
    assert_eq!(p["default"], "allow");
}

#[test]
fn network_default_override_restores_allow() {
    let md = "# Step 1: Work\n[allowed]: *\n[transition]: Exit\n";
    let p = initial_policy(md, "", true);
    assert_eq!(p["default"], "allow");
}

#[test]
fn unguarded_prompt_defaults_allow() {
    let p = initial_policy("No steps here, just prose.\n", "", false);
    assert_eq!(p["default"], "allow");
}

// --- [llmjudge] parsing (CCT-516) ---

#[test]
fn test_parse_llmjudge_questions_and_violations() {
    let md = "# Step 4: Accept\n\
              Assemble the evidence.\n\
              [llmjudge]\n\
              - Does every acceptance condition have evidence? :: only two of three are covered\n\
              - Does the diff implement the change itself?\n\
              [allowed]: *\n\
              [gate]: make check\n\
              [transition]: Exit\n";
    let steps = parse_steps(md).unwrap();
    let judge = &steps[&4].llmjudge;
    assert_eq!(judge.len(), 2);
    assert_eq!(judge[0].question, "Does every acceptance condition have evidence?");
    assert_eq!(judge[0].violation, "only two of three are covered");
    assert_eq!(judge[1].question, "Does the diff implement the change itself?");
    assert_eq!(judge[1].violation, "");
    // Other annotations still parse around the block.
    assert_eq!(steps[&4].gate, "make check");
    assert_eq!(steps[&4].body, "Assemble the evidence.", "questions are not body prose");

    // A step without the block has no judge.
    let steps2 = parse_steps("# Step 1: X\n[allowed]: *\n[transition]: Exit\n").unwrap();
    assert!(steps2[&1].llmjudge.is_empty());
}

#[test]
fn test_parse_llmjudge_malformed_blocks_error() {
    // No questions before the next annotation.
    let err = parse_steps("# Step 1: X\n[llmjudge]\n[allowed]: *\n").unwrap_err();
    assert!(err.message.contains("at least one"), "{err}");
    assert_eq!(err.step, 1);

    // No questions at end of input.
    assert!(parse_steps("# Step 1: X\n[llmjudge]\n").is_err());

    // No questions before the next step heading.
    assert!(parse_steps("# Step 1: X\n[llmjudge]\n# Step 2: Y\n[allowed]: *\n").is_err());

    // Blank line between the annotation and the list is malformed too —
    // questions must immediately follow.
    assert!(parse_steps("# Step 1: X\n[llmjudge]\n\n- Is it done?\n").is_err());

    // Inline value instead of a list.
    let err = parse_steps("# Step 1: X\n[llmjudge]: is it done?\n").unwrap_err();
    assert!(err.message.contains("no inline value"), "{err}");

    // Empty question text.
    assert!(parse_steps("# Step 1: X\n[llmjudge]\n-\n").is_err());
    assert!(parse_steps("# Step 1: X\n[llmjudge]\n- :: only a violation\n").is_err());

    // Duplicate block in one step.
    let err = parse_steps("# Step 1: X\n[llmjudge]\n- Q1?\n[llmjudge]\n- Q2?\n").unwrap_err();
    assert!(err.message.contains("duplicate"), "{err}");

    // Question-count cap.
    let questions = |n: usize| -> String {
        use std::fmt::Write;
        let mut out = String::new();
        for i in 0..n {
            let _ = writeln!(out, "- Question {i}?");
        }
        out
    };
    let md = format!(
        "# Step 1: X\n[llmjudge]\n{}",
        questions(cctui_guard::parser::MAX_JUDGE_QUESTIONS + 1)
    );
    let err = parse_steps(&md).unwrap_err();
    assert!(err.message.contains("more than"), "{err}");

    // Exactly the cap is fine.
    let md =
        format!("# Step 1: X\n[llmjudge]\n{}", questions(cctui_guard::parser::MAX_JUDGE_QUESTIONS));
    assert_eq!(
        parse_steps(&md).unwrap()[&1].llmjudge.len(),
        cctui_guard::parser::MAX_JUDGE_QUESTIONS
    );
}

#[test]
fn test_example_context_pack_prompt_parses_with_llmjudge() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/examples/context-pack/prompts/example-task.md"
    );
    let md = std::fs::read_to_string(path).unwrap();
    let steps = parse_steps(&md).unwrap();
    assert_eq!(steps.len(), 5);
    let judge = &steps[&4].llmjudge;
    assert_eq!(judge.len(), 4, "Accept step carries the [llmjudge] block");
    assert!(judge.iter().all(|q| !q.question.is_empty() && !q.violation.is_empty()));
    assert!(steps[&1].llmjudge.is_empty() && steps[&5].llmjudge.is_empty());
    assert_eq!(steps[&4].transition, "5, Exit", "annotations after the block still parse");
}

#[test]
fn per_transition_gate_runs_only_for_its_target() {
    let prompt = "\
# Step 1
[transition]: 2, 3
```guard
transitions: [{to: 2, gate: \"false\"}, {to: 3, gate: \"true\"}]
```

# Step 2
[transition]: Exit

# Step 3
[transition]: Exit
";
    let t = make_engine("", prompt);
    let denied = t.engine.transition(&json!(2));
    assert_eq!(denied["ok"], json!(false), "→2 gate `false` refuses, stays on Step 1");
    assert_eq!(t.engine.get_state()["step"], json!(1));
    let ok = t.engine.transition(&json!(3));
    assert_eq!(ok["ok"], json!(true));
    assert_eq!(t.engine.get_state()["step"], json!(3));
}

#[test]
fn step_gate_and_transition_gate_both_run() {
    let prompt = "\
# Step 1
[gate]: true
[transition]: 2
```guard
transitions: [{to: 2, gate: \"false\"}]
```

# Step 2
[transition]: Exit
";
    let t = make_engine("", prompt);
    let resp = t.engine.transition(&json!(2));
    assert_eq!(resp["ok"], json!(false), "step gate passes but the per-target gate fails");
    assert_eq!(t.engine.get_state()["step"], json!(1));
}

#[test]
fn max_visits_breaks_a_ping_pong_loop() {
    let prompt = "\
# Step 1
[transition]: 2

# Step 2
[transition]: 1
```guard
max-visits: 2
```
";
    let t = make_engine("", prompt);
    assert_eq!(t.engine.transition(&json!(2))["ok"], json!(true)); // visit 1
    assert_eq!(t.engine.transition(&json!(1))["ok"], json!(true));
    assert_eq!(t.engine.transition(&json!(2))["ok"], json!(true)); // visit 2
    assert_eq!(t.engine.transition(&json!(1))["ok"], json!(true));
    let denied = t.engine.transition(&json!(2)); // would be visit 3
    assert_eq!(denied["ok"], json!(false));
    assert!(denied["error"].as_str().unwrap().contains("maximum"));
    assert_eq!(t.engine.get_state()["step"], json!(1), "stays put on the denied re-entry");
    // Exit is never blocked by the visit bound.
    assert_eq!(t.engine.transition(&json!("exit"))["ok"], json!(true));
}

#[test]
fn max_visits_counts_the_initial_entry() {
    let prompt = "\
# Step 1
[transition]: 2
```guard
max-visits: 1
```

# Step 2
[transition]: 1
";
    let t = make_engine("", prompt);
    assert_eq!(t.engine.transition(&json!(2))["ok"], json!(true));
    let denied = t.engine.transition(&json!(1));
    assert_eq!(denied["ok"], json!(false), "re-entering the max-visits:1 entry step is denied");
}

#[test]
fn legacy_state_file_without_visits_is_read() {
    let prompt = "\
# Step 1
[transition]: 2

# Step 2
[transition]: Exit
";
    let dir = tempfile::tempdir().unwrap();
    let state_file = dir.path().join("state");
    let engine = WorkflowEngine::new(
        parse_steps(prompt).unwrap(),
        no_sets(),
        state_file.clone(),
        dir.path().join("guard-proxy").join("policy.json"),
        vec![],
        dir.path().to_path_buf(),
        None,
        false,
    );
    std::fs::write(&state_file, "{\"step\": 1}").unwrap();
    assert_eq!(engine.transition(&json!(2))["ok"], json!(true));
}
