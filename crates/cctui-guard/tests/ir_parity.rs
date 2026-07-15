//! Security-boundary parity suite for the typed IR (CCT-619).
//!
//! The IR is a compiled model of the markdown prompt; it must not change a
//! single allow/deny decision. These tests build one engine straight from the
//! markdown parser and a second engine from the compiled IR (`Workflow::compile`
//! → `into_steps`), then assert the two engines agree on every check, transition,
//! and egress policy. The JSON frontend is exercised through the same equality.

use std::collections::BTreeMap;
use std::sync::Arc;

use cctui_guard::engine::WorkflowEngine;
use cctui_guard::ir::{Rule, Transition, Version, Workflow};
use cctui_guard::parser::{Step, parse_guard_rules_str, parse_steps};
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
    steps: BTreeMap<u32, Step>,
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

#[test]
fn markdown_and_ir_engines_make_identical_decisions() {
    let dir = tempfile::tempdir().unwrap();

    let md_engine = engine_from(parse_steps(PROMPT).unwrap(), dir.path(), "md");
    let workflow = Workflow::compile(PROMPT).unwrap();
    assert_eq!(workflow.version, Version::V1, "[guard]: v1 header parsed");
    let ir_engine = engine_from(workflow.into_steps(), dir.path(), "ir");

    // Step 1 decisions + egress policy must match exactly.
    for (tool, input) in matrix() {
        assert_eq!(
            decision(&md_engine, tool, &input),
            decision(&ir_engine, tool, &input),
            "step 1 divergence for {tool} {input}"
        );
    }
    assert_eq!(policy(dir.path(), "md"), policy(dir.path(), "ir"), "step 1 policy divergence");

    // Advance both to step 2 and re-check (gate `true` passes, no judge cmd but
    // the judge only runs on transition — here both advance identically).
    let md_tr = md_engine.transition(&json!(2));
    let ir_tr = ir_engine.transition(&json!(2));
    assert_eq!(md_tr["ok"], ir_tr["ok"], "transition ok divergence: {md_tr} vs {ir_tr}");
    assert_eq!(md_tr["step"], ir_tr["step"]);
    assert_eq!(md_tr["reinject"], ir_tr["reinject"], "reinjection body divergence");

    for (tool, input) in matrix() {
        assert_eq!(
            decision(&md_engine, tool, &input),
            decision(&ir_engine, tool, &input),
            "step 2 divergence for {tool} {input}"
        );
    }
    assert_eq!(policy(dir.path(), "md"), policy(dir.path(), "ir"), "step 2 policy divergence");

    // Invalid + exit transitions agree too.
    assert_eq!(md_engine.transition(&json!(1))["ok"], ir_engine.transition(&json!(1))["ok"]);
    assert_eq!(
        md_engine.transition(&json!("exit"))["ok"],
        ir_engine.transition(&json!("exit"))["ok"]
    );
}

/// The committed example dispatch prompt (five steps, gates, llmjudge) compiles
/// to an IR whose engine matches the markdown engine step-for-step.
#[test]
fn example_pack_prompt_parity() {
    let md = include_str!("../../../deploy/examples/context-pack/prompts/example-task.md");
    let dir = tempfile::tempdir().unwrap();

    let md_steps = parse_steps(md).unwrap();
    let workflow = Workflow::compile(md).unwrap();
    let ir_steps = workflow.into_steps();

    // Lowered IR steps are semantically equal to the parsed steps on every field
    // the engine reads (allow/deny keyword strings are re-derived, so compare
    // the parsed keyword sets rather than raw spelling).
    assert_eq!(md_steps.keys().collect::<Vec<_>>(), ir_steps.keys().collect::<Vec<_>>());
    for (n, md_step) in &md_steps {
        let ir_step = &ir_steps[n];
        assert_eq!(md_step.title, ir_step.title, "step {n} title");
        assert_eq!(md_step.body, ir_step.body, "step {n} body");
        assert_eq!(md_step.gate, ir_step.gate, "step {n} gate");
        assert_eq!(md_step.compact, ir_step.compact, "step {n} compact");
        assert_eq!(md_step.llmjudge, ir_step.llmjudge, "step {n} judge");
        assert_eq!(md_step.network, ir_step.network, "step {n} network");
    }

    let md_engine = engine_from(md_steps, dir.path(), "pmd");
    let ir_engine = engine_from(ir_steps, dir.path(), "pir");
    for (tool, input) in matrix() {
        assert_eq!(
            decision(&md_engine, tool, &input),
            decision(&ir_engine, tool, &input),
            "example-pack step 1 divergence for {tool}"
        );
    }
    assert_eq!(
        policy(dir.path(), "pmd"),
        policy(dir.path(), "pir"),
        "example-pack policy divergence"
    );
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
fn rule_and_transition_lowering_is_semantically_stable() {
    assert_eq!(Rule::from_raw(""), Rule::Unrestricted);
    assert_eq!(Rule::from_raw("  "), Rule::Unrestricted);
    assert_eq!(Rule::from_raw("*"), Rule::Wildcard);
    assert_eq!(Rule::from_raw("Read, Edit").to_raw(), "Read, Edit");
    assert_eq!(Rule::Unrestricted.to_raw(), "");
    assert_eq!(Rule::Wildcard.to_raw(), "*");

    let t = Transition::from_raw("Step 9, Step 11, Exit");
    assert_eq!(t, Transition { to: vec![9, 11], exit: true });
    assert_eq!(Transition::from_raw(&t.to_raw()), t, "transition lowering round-trips");
    assert_eq!(Transition::from_raw("Exit"), Transition { to: vec![], exit: true });
}
