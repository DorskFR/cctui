//! End-to-end coverage of the JSONL decision log and the end-of-run report:
//! drive a real engine through a denied tool call and a gated-then-exit run,
//! then assert the log lines and the aggregated report the session page renders.

use std::path::PathBuf;

use cctui_guard::decision_log::{DecisionLog, Kind, build_report, parse_log};
use cctui_guard::engine::WorkflowEngine;
use cctui_guard::ir::Workflow;
use cctui_guard::parser::parse_guard_rules_str;
use serde_json::{Value, json};

const RULES: &str = "\
[code-read]: Read, Grep
[net-claude]: api.example.com:443
";

const PROMPT: &str = "\
# Step 1: Research
[allowed]: code-read
[disallowed]: git push
[network]: net-claude
[transition]: 2, Exit

# Step 2: Implement
[allowed]: code-read
[network]: net-claude
[transition]: Exit
";

struct Fixtures {
    log: PathBuf,
    report: PathBuf,
    _dir: tempfile::TempDir,
}

fn engine() -> (WorkflowEngine, Fixtures) {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("decisions.jsonl");
    let report = dir.path().join("report.json");
    let engine = WorkflowEngine::new_with_log(
        Workflow::compile(PROMPT).unwrap().into_steps(),
        parse_guard_rules_str(RULES),
        dir.path().join("state"),
        dir.path().join("policy.json"),
        vec![],
        dir.path().to_path_buf(),
        None,
        false,
        DecisionLog::new(Some(log.clone())),
        Some(report.clone()),
    );
    (engine, Fixtures { log, report, _dir: dir })
}

#[test]
fn logs_checks_transitions_and_writes_report_on_exit() {
    let (engine, fx) = engine();

    let denied = engine.check("Bash", &json!({ "command": "git push origin main" }));
    assert_eq!(
        denied["hookSpecificOutput"]["permissionDecision"], "deny",
        "git push must be denied in step 1"
    );
    let ok = engine.check("Read", &json!({ "file_path": "/workspace/x" }));
    assert_eq!(ok["hookSpecificOutput"]["permissionDecision"], "allow");

    // Advance to step 2, then exit.
    let adv = engine.transition(&json!(2));
    assert_eq!(adv["ok"], true, "advance to step 2: {adv}");
    let exit = engine.transition(&json!("exit"));
    assert_eq!(exit["step"], "exit");

    // The raw log carries a check-deny, an enter for each active step, and the
    // two transitions.
    let records = parse_log(&std::fs::read_to_string(&fx.log).unwrap());
    let deny_records =
        records.iter().filter(|r| r.kind == Kind::Check && r.verdict == "deny").collect::<Vec<_>>();
    assert_eq!(deny_records.len(), 1);
    assert_eq!(deny_records[0].tool.as_deref(), Some("Bash"));
    assert_eq!(deny_records[0].target, "git push origin main");
    assert_eq!(records.iter().filter(|r| r.kind == Kind::Enter).count(), 3); // 1, 2, exit
    assert_eq!(records.iter().filter(|r| r.kind == Kind::Transition).count(), 2);

    // The report was written on exit and aggregates the denied tool call.
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(&fx.report).unwrap()).unwrap();
    let tools = report["denied_tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["step"], 1);
    assert_eq!(tools[0]["target"], "git push origin main");
    assert_eq!(tools[0]["count"], 1);

    // Two real steps in the timeline (exit is not a duration row); transitions
    // list both hops with allow verdicts.
    assert_eq!(report["steps"].as_array().unwrap().len(), 2);
    let transitions = report["transitions"].as_array().unwrap();
    assert_eq!(transitions.len(), 2);
    assert!(transitions.iter().all(|t| t["verdict"] == "allow"));
}

#[test]
fn failed_gate_transition_records_deny_with_detail() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("d.jsonl");
    let prompt = "\
# Step 1: Gated
[gate]: exit 1
[transition]: 2, Exit

# Step 2: Done
[transition]: Exit
";
    let engine = WorkflowEngine::new_with_log(
        Workflow::compile(prompt).unwrap().into_steps(),
        parse_guard_rules_str(RULES),
        dir.path().join("state"),
        dir.path().join("policy.json"),
        vec![],
        dir.path().to_path_buf(),
        None,
        false,
        DecisionLog::new(Some(log.clone())),
        None,
    );

    let denied = engine.transition(&json!(2));
    assert_eq!(denied["ok"], false, "failed gate must refuse the advance");

    let report = build_report(&log);
    let transitions = report["transitions"].as_array().unwrap();
    let deny = transitions.iter().find(|t| t["verdict"] == "deny").unwrap();
    assert_eq!(deny["step"], 1);
    assert!(deny["detail"].as_str().unwrap().contains("gate failed"));
}

#[test]
fn disabled_log_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("absent.jsonl");
    let engine = WorkflowEngine::new(
        Workflow::compile(PROMPT).unwrap().into_steps(),
        parse_guard_rules_str(RULES),
        dir.path().join("state"),
        dir.path().join("policy.json"),
        vec![],
        dir.path().to_path_buf(),
        None,
        false,
    );
    let _ = engine.check("Bash", &json!({ "command": "git push" }));
    let _ = engine.transition(&json!("exit"));
    assert!(!log.exists(), "no log path configured ⇒ no file written");
}
