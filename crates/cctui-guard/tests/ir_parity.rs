//! Compilation + frontend parity suite for the typed IR.
//!
//! The IR is the model the engine enforces. Markdown is one frontend and
//! `workflow.json` the other; both compile into the same [`Workflow`], and
//! nothing is lowered back to strings for enforcement. These tests pin the
//! markdown → IR compilation field by field, assert the two frontends produce
//! engines that decide identically, and hold the published JSON Schema in sync.

use std::collections::BTreeMap;
use std::sync::Arc;

use cctui_guard::engine::WorkflowEngine;
use cctui_guard::ir::{Rule, Transition, Version, Workflow, WorkflowStep};
use cctui_guard::parser::{
    Step, parse_guard_rules_str, parse_keywords, parse_steps, parse_transitions,
};
use serde_json::{Value, json};

const RULES: &str = "\
[code-read]: Read, Grep, Glob, LSP, WebFetch, WebSearch
[code-write]: Edit, Write
[git-read]: git log, git diff, git status, git fetch
[git-write]: git checkout, git commit, git push
[github-read]: gh pr list, gh pr view, gh api
[github-write]: gh pr create, gh pr edit, git push
[all-read]: code-read, git-read, github-read
[remote-write]: git push, github-write

[net-claude]: api.example.com:443
[net-github]: github.example.com:443, github.example.com:22
";

const PROMPT: &str = "\
[guard]: v1

# Step 1: Research the task

Look around, do not modify anything.

[allowed]: all-read
[disallowed]: *
[network]: net-claude, net-github
[transition]: 2, Exit

# Step 2: Implement

Make the change locally; pushing is not allowed.

[allowed]: all-read, code-write, Bash, git commit
[disallowed]: remote-write
[network]: net-claude
[gate]: true
[compact]
[llmjudge]
- Does the diff implement the change? :: only a test was added
[transition]: Exit
";

fn engine_from(
    steps: BTreeMap<u32, WorkflowStep>,
    dir: &std::path::Path,
    name: &str,
) -> Arc<WorkflowEngine> {
    let proxy_dir = dir.join(format!("{name}-proxy"));
    std::fs::create_dir_all(&proxy_dir).unwrap();
    Arc::new(WorkflowEngine::new(
        steps,
        parse_guard_rules_str(RULES),
        dir.join(format!("{name}-state")),
        proxy_dir.join("policy.json"),
        vec!["callback.example.com:443".to_string()],
        dir.to_path_buf(),
        None,
        false,
    ))
}

fn policy(dir: &std::path::Path, name: &str) -> Value {
    let p = dir.join(format!("{name}-proxy")).join("policy.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// A representative matrix of tool calls exercising wildcard allow, wildcard
/// deny, tool-set expansion, Bash segment splitting, and git-flag normalization.
fn matrix() -> Vec<(&'static str, Value)> {
    vec![
        ("Read", json!({"file_path": "/tmp/x"})),
        ("Edit", json!({"file_path": "/tmp/x"})),
        ("Write", json!({"file_path": "/tmp/x"})),
        ("Grep", json!({"pattern": "x"})),
        ("Bash", json!({"command": "git push origin main"})),
        ("Bash", json!({"command": "git commit -m wip"})),
        ("Bash", json!({"command": "git -C /repo fetch && git status"})),
        ("Bash", json!({"command": "echo URL rewrite && edited credit"})),
        ("Bash", json!({"command": "gh pr create"})),
        ("ToolSearch", json!({})),
    ]
}

fn decision(engine: &WorkflowEngine, tool: &str, input: &Value) -> String {
    engine.check(tool, input)["hookSpecificOutput"]["permissionDecision"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Assert a parsed markdown [`Step`] compiled into the typed [`WorkflowStep`]
/// the engine enforces, field by field.
fn assert_compiled(n: u32, md: &Step, ir: &WorkflowStep) {
    assert_eq!(md.title, ir.title, "step {n} title");
    assert_eq!(md.body, ir.body, "step {n} body");
    assert_eq!(md.compact, ir.compact, "step {n} compact");
    assert_eq!(md.llmjudge, ir.judge, "step {n} judge");
    assert_eq!(md.max_visits, ir.max_visits, "step {n} max-visits");
    assert_eq!(md.gate.trim(), ir.gate.as_deref().unwrap_or(""), "step {n} gate");
    assert_eq!(Rule::from_raw(&md.allowed), ir.allowed, "step {n} allowed");
    assert_eq!(Rule::from_raw(&md.disallowed), ir.disallowed, "step {n} disallowed");
    assert_eq!(
        parse_transitions(&md.transition),
        (ir.transition.to.clone(), ir.transition.exit),
        "step {n} transition"
    );
    assert_eq!(md.transition_gates, ir.transition.gates, "step {n} transition gates");
    let network: Vec<String> = md
        .network
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    assert_eq!(network, ir.network, "step {n} network");
}

#[test]
fn markdown_compiles_field_by_field_into_the_ir() {
    let md_steps = parse_steps(PROMPT).unwrap();
    let workflow = Workflow::compile(PROMPT).unwrap();
    assert_eq!(workflow.version, Version::V1, "[guard]: v1 header parsed");
    let ir_steps = workflow.into_steps();

    assert_eq!(md_steps.keys().collect::<Vec<_>>(), ir_steps.keys().collect::<Vec<_>>());
    for (n, md_step) in &md_steps {
        assert_compiled(*n, md_step, &ir_steps[n]);
    }
}

#[test]
fn both_frontends_drive_identical_decisions() {
    let dir = tempfile::tempdir().unwrap();

    let from_md = Workflow::compile(PROMPT).unwrap();
    let from_json = Workflow::from_json(&serde_json::to_string_pretty(&from_md).unwrap()).unwrap();

    let md_engine = engine_from(from_md.into_steps(), dir.path(), "md");
    let json_engine = engine_from(from_json.into_steps(), dir.path(), "json");

    for (tool, input) in matrix() {
        assert_eq!(
            decision(&md_engine, tool, &input),
            decision(&json_engine, tool, &input),
            "step 1 divergence for {tool} {input}"
        );
    }
    assert_eq!(policy(dir.path(), "md"), policy(dir.path(), "json"), "step 1 policy divergence");

    let md_tr = md_engine.transition(&json!(2));
    let json_tr = json_engine.transition(&json!(2));
    assert_eq!(md_tr["ok"], json_tr["ok"], "transition ok divergence: {md_tr} vs {json_tr}");
    assert_eq!(md_tr["step"], json_tr["step"]);
    assert_eq!(md_tr["reinject"], json_tr["reinject"], "reinjection body divergence");

    for (tool, input) in matrix() {
        assert_eq!(
            decision(&md_engine, tool, &input),
            decision(&json_engine, tool, &input),
            "step 2 divergence for {tool} {input}"
        );
    }
    assert_eq!(policy(dir.path(), "md"), policy(dir.path(), "json"), "step 2 policy divergence");

    assert_eq!(md_engine.transition(&json!(1))["ok"], json_engine.transition(&json!(1))["ok"]);
    assert_eq!(
        md_engine.transition(&json!("exit"))["ok"],
        json_engine.transition(&json!("exit"))["ok"]
    );
}

/// The committed example dispatch prompt (five steps, gates, llmjudge) compiles
/// field-for-field and yields a working engine.
#[test]
fn example_pack_prompt_compiles() {
    let md = include_str!("../../../deploy/examples/context-pack/prompts/example-task.md");
    let dir = tempfile::tempdir().unwrap();

    let md_steps = parse_steps(md).unwrap();
    let ir_steps = Workflow::compile(md).unwrap().into_steps();

    assert_eq!(md_steps.keys().collect::<Vec<_>>(), ir_steps.keys().collect::<Vec<_>>());
    for (n, md_step) in &md_steps {
        assert_compiled(*n, md_step, &ir_steps[n]);
    }

    let engine = engine_from(ir_steps, dir.path(), "pack");
    for (tool, input) in matrix() {
        decision(&engine, tool, &input);
    }
    assert_eq!(policy(dir.path(), "pack")["default"], json!("deny"));
}

#[test]
fn inline_sets_and_rules_imports_survive_the_json_frontend() {
    let md = "\
[rules]: ./net-common.md
[net-yt]: yt.example.com:443
[code-read]+: mcp__yt

# Step 1: X
[allowed]: code-read
[network]: net-yt
[transition]: Exit
";
    let wf = Workflow::compile(md).unwrap();
    assert_eq!(wf.rules, vec!["./net-common.md".to_string()]);
    assert_eq!(wf.sets.len(), 2);
    assert_eq!(wf.sets[0].name, "net-yt");
    assert!(wf.sets[1].extend, "code-read+ is an extend");

    let json = serde_json::to_string_pretty(&wf).unwrap();
    assert_eq!(Workflow::from_json(&json).unwrap(), wf);
}

#[test]
fn json_frontend_roundtrips_through_the_ir() {
    let from_md = Workflow::compile(PROMPT).unwrap();
    let json = serde_json::to_string_pretty(&from_md).unwrap();
    let from_json = Workflow::from_json(&json).unwrap();
    assert_eq!(from_md, from_json, "workflow.json is a faithful second frontend");

    assert_eq!(from_md.into_steps(), from_json.into_steps());
}

#[test]
fn version_header_defaults_and_rejects() {
    // Missing header ⇒ v1.
    let no_header = Workflow::compile("# Step 1: X\n[allowed]: *\n[transition]: Exit").unwrap();
    assert_eq!(no_header.version, Version::V1);

    // Explicit v1 (with or without the `v`).
    for header in ["[guard]: v1", "[guard]: 1", "[GUARD]: V1"] {
        let md = format!("{header}\n# Step 1: X\n[allowed]: *\n[transition]: Exit");
        assert_eq!(Workflow::compile(&md).unwrap().version, Version::V1, "{header}");
    }

    // Unknown version ⇒ compile error (fail loud, don't silently accept).
    let err = Workflow::compile("[guard]: v2\n# Step 1: X\n[allowed]: *\n[transition]: Exit");
    assert!(err.is_err(), "unsupported version must be rejected");
}

#[test]
fn network_default_header_parses_and_rejects() {
    use cctui_guard::ir::NetworkDefault;

    let none = Workflow::compile("# Step 1: X\n[allowed]: *\n[transition]: Exit").unwrap();
    assert_eq!(none.network_default, None);

    let allow = Workflow::compile(
        "[network-default]: allow\n# Step 1: X\n[allowed]: *\n[transition]: Exit",
    )
    .unwrap();
    assert_eq!(allow.network_default, Some(NetworkDefault::Allow));

    let deny =
        Workflow::compile("[network-default]: DENY\n# Step 1: X\n[allowed]: *\n[transition]: Exit")
            .unwrap();
    assert_eq!(deny.network_default, Some(NetworkDefault::Deny));

    let bad = Workflow::compile(
        "[network-default]: maybe\n# Step 1: X\n[allowed]: *\n[transition]: Exit",
    );
    assert!(bad.is_err(), "an unknown value must be rejected");
}

/// The published JSON Schema artifact is the versioned contract for the IR.
/// It must stay in sync with the Rust types; regenerate with
/// `cargo run -p cctui-guard -- --emit-schema > schema/workflow.v1.json`.
#[test]
fn published_json_schema_matches_committed_artifact() {
    let committed: Value =
        serde_json::from_str(include_str!("../schema/workflow.v1.json")).unwrap();
    assert_eq!(
        cctui_guard::ir::json_schema(),
        committed,
        "schema/workflow.v1.json is stale — re-run `cargo run -p cctui-guard -- --emit-schema`"
    );
}

#[test]
fn rule_and_transition_compilation_is_stable() {
    assert_eq!(Rule::from_raw(""), Rule::Unrestricted);
    assert_eq!(Rule::from_raw("  "), Rule::Unrestricted);
    assert_eq!(Rule::from_raw("*"), Rule::Wildcard);
    assert_eq!(Rule::from_raw("Read, Edit").to_raw(), "Read, Edit");
    assert_eq!(Rule::Unrestricted.to_raw(), "");
    assert_eq!(Rule::Wildcard.to_raw(), "*");

    assert_eq!(
        Transition::from_raw("Step 9, Step 11, Exit"),
        Transition { to: vec![9, 11], exit: true, ..Default::default() }
    );
    assert_eq!(
        Transition::from_raw("Exit"),
        Transition { to: vec![], exit: true, ..Default::default() }
    );
}

#[test]
fn rule_expansion_expands_sets_without_resplitting_tokens() {
    let sets = parse_guard_rules_str(RULES);
    assert!(Rule::Unrestricted.expand(&sets).is_empty());
    assert_eq!(Rule::Wildcard.expand(&sets), vec!["*".to_string()]);
    assert_eq!(
        Rule::List(vec!["code-write".to_string()]).expand(&sets),
        vec!["Edit".to_string(), "Write".to_string()],
        "set names still expand recursively"
    );
    let comma = Rule::List(vec!["git commit -m 'a, b'".to_string()]);
    assert_eq!(
        comma.expand(&sets),
        vec!["git commit -m 'a, b'".to_string()],
        "a token containing a comma stays one keyword"
    );
    assert_eq!(
        parse_keywords(&comma.to_raw(), &sets).len(),
        2,
        "routing the same rule through its raw string splits it — hence expand()"
    );
}

/// A hand-authored `workflow.json` may carry a keyword containing a `,`. It must
/// reach enforcement as ONE keyword: re-splitting it would silently widen the
/// allow-list into two shorter, more permissive phrases.
#[test]
fn comma_bearing_keyword_survives_to_enforcement() {
    let json = r#"{
        "version": "v1",
        "steps": [{
            "id": 1,
            "title": "X",
            "allowed": {"list": ["git log --oneline, git status"]},
            "transition": {"exit": true}
        }]
    }"#;
    let wf = Workflow::from_json(json).unwrap();
    let steps = wf.into_steps();
    assert_eq!(
        steps[&1].allowed,
        Rule::List(vec!["git log --oneline, git status".to_string()]),
        "the authored keyword round-trips whole"
    );

    let dir = tempfile::tempdir().unwrap();
    let engine = engine_from(steps, dir.path(), "comma");
    assert_eq!(
        decision(&engine, "Bash", &json!({"command": "git log --oneline, git status"})),
        "allow",
        "the authored phrase itself is allowed"
    );
    assert_eq!(
        decision(&engine, "Bash", &json!({"command": "git status"})),
        "deny",
        "the comma must not split the keyword into two shorter allowed phrases"
    );
}

/// A `guard` fenced block's per-transition gates and `max-visits` bound survive
/// the markdown → IR → JSON → IR round-trip intact.
#[test]
fn guard_block_fields_round_trip_through_the_ir() {
    let md = "\
# Step 1: Work
[transition]: Exit
```guard
max-visits: 3
transitions:
  - to: 2
    gate: make test
  - to: 5
```

# Step 2: X
[transition]: Exit

# Step 5: Y
[transition]: Exit
";
    let wf = Workflow::compile(md).unwrap();
    let step1 = &wf.steps[0];
    assert_eq!(step1.max_visits, Some(3));
    assert_eq!(step1.transition.gates.get(&2).map(String::as_str), Some("make test"));
    assert!(!step1.transition.gates.contains_key(&5));
    assert!(step1.transition.to.contains(&2) && step1.transition.to.contains(&5));
    assert!(step1.transition.exit, "the bracket-line Exit is preserved");

    let json = serde_json::to_string_pretty(&wf).unwrap();
    let from_json = Workflow::from_json(&json).unwrap();
    assert_eq!(wf, from_json, "guard-block fields survive the JSON frontend");

    let md_steps = parse_steps(md).unwrap();
    let ir_steps = wf.into_steps();
    for (n, md_step) in &md_steps {
        assert_compiled(*n, md_step, &ir_steps[n]);
    }
}
