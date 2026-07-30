//! End-to-end integration test: spin up the axum server and drive a full
//! allow → transition → deny scenario over HTTP using a sample prompt and a
//! neutralized guard-rules file (no homelab hostnames).

use std::sync::Arc;

use cctui_guard::engine::WorkflowEngine;
use cctui_guard::ir::Workflow;
use cctui_guard::parser::parse_guard_rules_str;
use cctui_guard::server::router;
use serde_json::{Value, json};

// Neutralized guard-rules: structure mirrors the real file, hosts are example.com.
const RULES: &str = "\
# Guard Rules (neutralized for the public repo)

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

// A two-step prompt: read-only research → local implementation, then Exit.
const PROMPT: &str = "\
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
[transition]: Exit
";

async fn spawn() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let proxy_dir = dir.path().join("guard-proxy");
    std::fs::create_dir_all(&proxy_dir).unwrap();
    let policy_file = proxy_dir.join("policy.json");
    let state_file = dir.path().join("state");

    let engine = Arc::new(WorkflowEngine::new(
        Workflow::compile(PROMPT).unwrap().into_steps(),
        parse_guard_rules_str(RULES),
        state_file,
        policy_file,
        vec!["callback.example.com:443".to_string()],
        dir.path().to_path_buf(),
        None,
        false,
    ));

    let app = router(engine);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), dir)
}

async fn check(client: &reqwest::Client, base: &str, tool: &str, input: Value) -> String {
    let resp: Value = client
        .post(format!("{base}/check"))
        .json(&json!({"tool_name": tool, "tool_input": input}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    resp["hookSpecificOutput"]["permissionDecision"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn allow_transition_deny_flow() {
    let (base, dir) = spawn().await;
    let client = reqwest::Client::new();
    let policy_file = dir.path().join("guard-proxy").join("policy.json");

    // --- Step 1: read-only ---
    assert_eq!(
        check(&client, &base, "Read", json!({"file_path": "/tmp/x"})).await,
        "allow",
        "step 1: Read allowed via all-read"
    );
    assert_eq!(
        check(&client, &base, "Edit", json!({"file_path": "/tmp/x"})).await,
        "deny",
        "step 1: Edit denied (read-only)"
    );
    assert_eq!(
        check(&client, &base, "Bash", json!({"command": "git push origin main"})).await,
        "deny",
        "step 1: git push denied"
    );

    // Step 1 policy must restrict egress to the net-claude + net-github hosts
    // plus the always-allowed callback host, default deny.
    let policy: Value =
        serde_json::from_str(&std::fs::read_to_string(&policy_file).unwrap()).unwrap();
    assert_eq!(policy["default"], "deny");
    let hosts: Vec<&str> =
        policy["allowed_hosts"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(hosts.contains(&"api.example.com:443"));
    assert!(hosts.contains(&"github.example.com:443"));
    assert!(hosts.contains(&"callback.example.com:443"), "always-allowed seeded");

    // --- transition 1 → 2 ---
    let tr: Value = client
        .post(format!("{base}/transition"))
        .json(&json!({"step": 2}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tr["ok"], true, "transition to step 2 ok");
    assert_eq!(tr["step"], 2);

    // Step 2 policy: only net-claude + callback, github no longer allowed.
    let policy: Value =
        serde_json::from_str(&std::fs::read_to_string(&policy_file).unwrap()).unwrap();
    let hosts: Vec<&str> =
        policy["allowed_hosts"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(hosts.contains(&"api.example.com:443"));
    assert!(!hosts.contains(&"github.example.com:443"), "github egress dropped at step 2");

    // --- Step 2: local writes allowed, push still denied ---
    assert_eq!(
        check(&client, &base, "Edit", json!({"file_path": "/tmp/x"})).await,
        "allow",
        "step 2: Edit allowed"
    );
    assert_eq!(
        check(&client, &base, "Bash", json!({"command": "git commit -m wip"})).await,
        "allow",
        "step 2: git commit allowed"
    );
    assert_eq!(
        check(&client, &base, "Bash", json!({"command": "git push origin main"})).await,
        "deny",
        "step 2: git push denied by remote-write"
    );

    // An invalid transition from step 2 (only Exit allowed) is rejected.
    let tr: Value = client
        .post(format!("{base}/transition"))
        .json(&json!({"step": 1}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tr["ok"], false, "step 2 → step 1 is not a valid transition");

    // --- Exit: everything denied afterwards ---
    let tr: Value = client
        .post(format!("{base}/transition"))
        .json(&json!({"step": "exit"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tr["ok"], true);
    assert_eq!(tr["step"], "exit");

    assert_eq!(
        check(&client, &base, "Read", json!({"file_path": "/tmp/x"})).await,
        "deny",
        "after exit: everything denied"
    );
}

/// A `[gate]` on a step is a deterministic completion check. The
/// transition *out* of the step is refused until the gate command exits 0, and
/// a successful advance re-injects the next step's authoritative prompt body.
#[tokio::test]
async fn gated_transition_requires_proof_and_reinjects() {
    let dir = tempfile::tempdir().unwrap();
    let state_file = dir.path().join("state");
    // Marker file the gate checks for — absent at first, so the gate fails.
    let marker = dir.path().join("done.txt");
    let prompt = format!(
        "# Step 1: Implement\n\
         Make the change.\n\
         [allowed]: *\n\
         [gate]: test -f {}\n\
         [transition]: 2, Exit\n\
         \n\
         # Step 2: Finalize\n\
         Open the PR with the assembled evidence.\n\
         [allowed]: *\n\
         [transition]: Exit\n",
        marker.display()
    );

    let engine = Arc::new(WorkflowEngine::new(
        Workflow::compile(&prompt).unwrap().into_steps(),
        parse_guard_rules_str(RULES),
        state_file,
        dir.path().join("nopolicy").join("policy.json"),
        vec![],
        dir.path().to_path_buf(),
        None,
        false,
    ));

    // Gate not yet satisfied → transition refused, still on Step 1.
    let refused = engine.transition(&json!(2));
    assert_eq!(refused["ok"], false, "gate fails ⇒ transition refused");
    assert_eq!(refused["step"], 1, "stays on the current step");
    assert!(
        refused["error"].as_str().unwrap().contains("transition gate failed"),
        "error explains the gate failure: {refused}"
    );

    // Satisfy the gate, then the transition succeeds and re-injects Step 2.
    std::fs::write(&marker, "ok").unwrap();
    let ok = engine.transition(&json!(2));
    assert_eq!(ok["ok"], true, "gate passes ⇒ transition allowed: {ok}");
    assert_eq!(ok["step"], 2);
    let reinject = ok["reinject"].as_str().unwrap();
    assert!(reinject.contains("Open the PR"), "re-injects the next-step prompt body");
    assert!(
        !reinject.contains("Compact your working context"),
        "no compact directive unless the step opts in via [compact]: {reinject}"
    );

    // Exit ignores the gate — bail-out must always work (back on a gated step).
    let engine2 = Arc::new(WorkflowEngine::new(
        Workflow::compile(&prompt).unwrap().into_steps(),
        parse_guard_rules_str(RULES),
        dir.path().join("state2"),
        dir.path().join("nopolicy2").join("policy.json"),
        vec![],
        dir.path().join("empty"), // gate would fail here, but Exit skips it
        None,
        false,
    ));
    let exit = engine2.transition(&json!("exit"));
    assert_eq!(exit["ok"], true, "Exit always allowed regardless of gate");
}

/// The step is always re-injected, but the compact-context directive is
/// opt-in per step via `[compact]` — so large-context models keep their context
/// unless a prompt explicitly asks to trim it.
#[tokio::test]
async fn compact_directive_is_opt_in_per_step() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = "# Step 1: Plan\n\
         Sketch the approach.\n\
         [allowed]: *\n\
         [transition]: 2, 3\n\
         \n\
         # Step 2: Implement\n\
         Write the code.\n\
         [compact]\n\
         [allowed]: *\n\
         [transition]: Exit\n\
         \n\
         # Step 3: Implement (no compaction)\n\
         Write the code, keeping full context.\n\
         [allowed]: *\n\
         [transition]: Exit\n";

    let make = |state_name: &str| {
        Arc::new(WorkflowEngine::new(
            Workflow::compile(prompt).unwrap().into_steps(),
            parse_guard_rules_str(RULES),
            dir.path().join(state_name),
            dir.path().join("nopolicy").join("policy.json"),
            vec![],
            dir.path().to_path_buf(),
            None,
            false,
        ))
    };

    // Step 2 declares [compact] → directive present.
    let to_compact = make("state-compact").transition(&json!(2));
    let r2 = to_compact["reinject"].as_str().unwrap();
    assert!(r2.contains("Write the code"), "re-injects the step body");
    assert!(
        r2.contains("Compact your working context"),
        "step with [compact] carries the directive: {r2}"
    );

    // Step 3 omits [compact] → body re-injected, but no compaction directive.
    let to_plain = make("state-plain").transition(&json!(3));
    let r3 = to_plain["reinject"].as_str().unwrap();
    assert!(r3.contains("keeping full context"), "re-injects the step body");
    assert!(
        !r3.contains("Compact your working context"),
        "step without [compact] does NOT force compaction: {r3}"
    );
}

/// An `[llmjudge]` block is a semantic acceptance gate. It runs after
/// the deterministic `[gate]`, in a clean context (the judge command gets only
/// the question prompt on stdin), and requires a perfect score: any 0, any
/// malformed verdict, or a missing judge command refuses the transition.
#[tokio::test]
async fn llmjudge_full_score_required_to_transition() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = "# Step 1: Accept\n\
         Assemble the evidence.\n\
         [allowed]: *\n\
         [llmjudge]\n\
         - Does every acceptance condition have evidence? :: two of three covered\n\
         - Does the diff implement the change itself?\n\
         [transition]: 2, Exit\n\
         \n\
         # Step 2: Finalize\n\
         Open the PR.\n\
         [allowed]: *\n\
         [transition]: Exit\n";

    let make = |name: &str, judge_cmd: Option<&str>| {
        Arc::new(WorkflowEngine::new(
            Workflow::compile(prompt).unwrap().into_steps(),
            parse_guard_rules_str(RULES),
            dir.path().join(name),
            dir.path().join("nopolicy").join("policy.json"),
            vec![],
            dir.path().to_path_buf(),
            judge_cmd.map(str::to_string),
            false,
        ))
    };

    // Partial score → refused, failing question + reason returned, state stays.
    let partial = r#"echo '[{"question":1,"answer":1,"reason":"evidence covers all"},{"question":2,"answer":0,"reason":"diff only adds a test"}]'"#;
    let engine = make("state-partial", Some(partial));
    let refused = engine.transition(&json!(2));
    assert_eq!(refused["ok"], false, "1/2 score refuses the transition: {refused}");
    assert_eq!(refused["step"], 1, "stays on the current step");
    let err = refused["error"].as_str().unwrap();
    assert!(err.contains("1/2"), "score surfaced: {err}");
    assert!(err.contains("Q2 FAILED"), "failing question named: {err}");
    assert!(err.contains("diff only adds a test"), "judge reason surfaced: {err}");
    assert_eq!(refused["evidence"][0]["kind"], "judge", "verdicts carried as judge evidence");
    assert_eq!(refused["evidence"][0]["verdicts"][1]["answer"], 0);
    // Still refused on retry (idempotent), then Exit still bails out.
    assert_eq!(engine.transition(&json!(2))["ok"], false);
    assert_eq!(engine.transition(&json!("exit"))["ok"], true, "Exit bypasses the judge");

    // Full score → transition proceeds, per-question verdicts emitted as a
    // `kind: "judge"` evidence entry for the result callback.
    let perfect = r#"echo '[{"question":1,"answer":1,"reason":"all conditions covered"},{"question":2,"answer":1,"reason":"implements the change"}]'"#;
    let engine = make("state-perfect", Some(perfect));
    let ok = engine.transition(&json!(2));
    assert_eq!(ok["ok"], true, "2/2 score allows the transition: {ok}");
    assert_eq!(ok["step"], 2);
    let entry = &ok["evidence"][0];
    assert_eq!(entry["kind"], "judge");
    assert!(entry["summary"].as_str().unwrap().contains("2/2"), "summary: {entry}");
    assert_eq!(entry["verdicts"].as_array().unwrap().len(), 2);
    assert_eq!(entry["verdicts"][0]["answer"], 1);
    assert!(
        entry["detail"].as_str().unwrap().contains("implements the change"),
        "per-question reasons in detail: {entry}"
    );

    // No judge command configured → fail closed.
    let engine = make("state-nocmd", None);
    let refused = engine.transition(&json!(2));
    assert_eq!(refused["ok"], false, "no --judge-cmd ⇒ refused: {refused}");
    assert!(refused["error"].as_str().unwrap().contains("fail closed"));

    // Garbage output → fail closed.
    let engine = make("state-garbage", Some("echo 'looks good to me!'"));
    let refused = engine.transition(&json!(2));
    assert_eq!(refused["ok"], false, "non-JSON verdict ⇒ refused: {refused}");

    // Wrong verdict count → fail closed.
    let one = r#"echo '[{"question":1,"answer":1,"reason":"ok"}]'"#;
    let refused = make("state-count", Some(one)).transition(&json!(2));
    assert_eq!(refused["ok"], false, "1 verdict for 2 questions ⇒ refused: {refused}");
    assert!(refused["error"].as_str().unwrap().contains("expected 2 verdicts"));

    // Judge command failing → fail closed.
    let refused = make("state-exit1", Some("exit 1")).transition(&json!(2));
    assert_eq!(refused["ok"], false, "judge command exit 1 ⇒ refused: {refused}");
}

/// The judge runs **after** the deterministic gate (which stays
/// independently enforced), and its stdin prompt is the clean-context question
/// block — questions + violation examples, no implementer reasoning.
#[tokio::test]
async fn llmjudge_runs_after_gate_with_clean_context() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("gate-ok");
    let captured = dir.path().join("judge-stdin.txt");
    let prompt = format!(
        "# Step 1: Accept\n\
         Prove it.\n\
         [allowed]: *\n\
         [gate]: test -f {}\n\
         [llmjudge]\n\
         - Is the acceptance condition observably met? :: asserted without output\n\
         [transition]: 2, Exit\n\
         \n\
         # Step 2: Finalize\n\
         [allowed]: *\n\
         [transition]: Exit\n",
        marker.display()
    );
    let judge_cmd = format!(
        r#"cat > {} && echo '[{{"question":1,"answer":1,"reason":"verified"}}]'"#,
        captured.display()
    );

    let engine = Arc::new(WorkflowEngine::new(
        Workflow::compile(&prompt).unwrap().into_steps(),
        parse_guard_rules_str(RULES),
        dir.path().join("state"),
        dir.path().join("nopolicy").join("policy.json"),
        vec![],
        dir.path().to_path_buf(),
        Some(judge_cmd),
        false,
    ));

    // Gate fails ⇒ refused with the gate error; the judge never ran.
    let refused = engine.transition(&json!(2));
    assert_eq!(refused["ok"], false);
    assert!(
        refused["error"].as_str().unwrap().contains("transition gate failed"),
        "gate failure reported, not a judge failure: {refused}"
    );
    assert!(!captured.exists(), "judge does not run until the gate passes");

    // Gate passes ⇒ judge runs and the prompt it saw is the clean question block.
    std::fs::write(&marker, "ok").unwrap();
    let ok = engine.transition(&json!(2));
    assert_eq!(ok["ok"], true, "gate + judge pass ⇒ transition: {ok}");
    let stdin = std::fs::read_to_string(&captured).unwrap();
    assert!(stdin.contains("1. Is the acceptance condition observably met?"), "{stdin}");
    assert!(stdin.contains("example violation: asserted without output"), "{stdin}");
    assert!(
        stdin.contains("answer each question independently") || stdin.contains("independently"),
        "{stdin}"
    );
    assert!(stdin.contains("JSON array"), "{stdin}");
}
