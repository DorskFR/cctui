//! End-to-end integration test: spin up the axum server and drive a full
//! allow → transition → deny scenario over HTTP using a sample prompt and a
//! neutralized guard-rules file (no homelab hostnames).

use std::sync::Arc;

use cctui_guard::engine::WorkflowEngine;
use cctui_guard::parser::{parse_guard_rules_str, parse_steps};
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
        parse_steps(PROMPT),
        parse_guard_rules_str(RULES),
        state_file,
        policy_file,
        vec!["callback.example.com:443".to_string()],
        dir.path().to_path_buf(),
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

/// CCT-440: a `[gate]` on a step is a deterministic completion check. The
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
        parse_steps(&prompt),
        parse_guard_rules_str(RULES),
        state_file,
        dir.path().join("nopolicy").join("policy.json"),
        vec![],
        dir.path().to_path_buf(),
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
        "CCT-450: no compact directive unless the step opts in via [compact]: {reinject}"
    );

    // Exit ignores the gate — bail-out must always work (back on a gated step).
    let engine2 = Arc::new(WorkflowEngine::new(
        parse_steps(&prompt),
        parse_guard_rules_str(RULES),
        dir.path().join("state2"),
        dir.path().join("nopolicy2").join("policy.json"),
        vec![],
        dir.path().join("empty"), // gate would fail here, but Exit skips it
    ));
    let exit = engine2.transition(&json!("exit"));
    assert_eq!(exit["ok"], true, "Exit always allowed regardless of gate");
}

/// CCT-450: the step is always re-injected, but the compact-context directive is
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
            parse_steps(prompt),
            parse_guard_rules_str(RULES),
            dir.path().join(state_name),
            dir.path().join("nopolicy").join("policy.json"),
            vec![],
            dir.path().to_path_buf(),
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
