//! Exec interception.
//!
//! cctui-supervisor installs a seccomp user-notification filter on
//! `execve`/`execveat` and hands its listener here; every exec in the worker
//! tree then waits for this module's verdict from the engine.

pub mod argv;
pub mod decide;
pub mod why;

#[cfg(target_os = "linux")]
mod notify;
#[cfg(target_os = "linux")]
mod sys;

pub use argv::ExecRequest;
#[cfg(target_os = "linux")]
pub use notify::spawn;
pub use why::{Denial, WhyRing};

#[cfg(test)]
mod tests {
    use crate::decision_log::{DecisionLog, Kind, Source, parse_log};
    use crate::engine::{Verdict, WorkflowEngine};
    use crate::ir::Workflow;
    use crate::parser::parse_guard_rules_str;

    use super::ExecRequest;

    const RULES: &str = "[code-read]: Read, Grep\n";

    const PROMPT: &str = "\
# Step 1: Research
[allowed]: code-read
[disallowed]: git push, gh pr comment
[transition]: 2

# Step 2: Locked down
[allowed]: code-read
[disallowed]: *
[transition]: Exit
";

    fn engine(dir: &tempfile::TempDir) -> WorkflowEngine {
        WorkflowEngine::new_with_log(
            Workflow::compile(PROMPT).unwrap().into_steps(),
            parse_guard_rules_str(RULES),
            dir.path().join("state"),
            dir.path().join("policy.json"),
            vec![],
            dir.path().to_path_buf(),
            None,
            false,
            DecisionLog::new(Some(dir.path().join("decisions.jsonl"))),
            None,
        )
    }

    fn check(engine: &WorkflowEngine, path: &str, argv: &[&str]) -> Verdict {
        let req = ExecRequest {
            path: path.to_string(),
            argv: argv.iter().map(ToString::to_string).collect(),
        };
        engine.check_exec(&req.path, &req.argv, &req.match_commands())
    }

    fn denied(v: &Verdict) -> bool {
        matches!(v, Verdict::Deny { .. })
    }

    #[test]
    fn a_disallowed_phrase_is_denied_however_git_is_reached() {
        let dir = tempfile::tempdir().unwrap();
        let e = engine(&dir);
        let cases: &[(&str, &[&str])] = &[
            ("/usr/bin/git", &["git", "push"]),
            ("/usr/bin/git", &["/usr/bin/git", "push", "origin", "main"]),
            ("/usr/bin/git", &["git", "-C", "/workspace", "push"]),
            ("/bin/sh", &["sh", "-c", "git push"]),
            ("/usr/bin/env", &["env", "-i", "git", "push"]),
            ("/usr/bin/git", &["python-disguise", "push"]),
            ("/workspace/static-bin", &["git", "push"]),
            ("/usr/bin/gh", &["gh", "pr", "comment", "1", "--body", "x"]),
        ];
        for (path, argv) in cases {
            let v = check(&e, path, argv);
            assert!(denied(&v), "{path} {argv:?} should be denied: {v:?}");
        }
    }

    #[test]
    fn other_execs_pass() {
        let dir = tempfile::tempdir().unwrap();
        let e = engine(&dir);
        for (path, argv) in [
            ("/usr/bin/git", &["git", "status"][..]),
            ("/usr/bin/git", &["git", "fetch", "origin"][..]),
            ("/usr/bin/rg", &["rg", "-n", "push"][..]),
            ("/usr/local/bin/node", &["node", "server.js"][..]),
        ] {
            assert_eq!(check(&e, path, argv), Verdict::Allow, "{path} {argv:?}");
        }
    }

    #[test]
    fn wildcard_disallow_does_not_block_every_exec() {
        let dir = tempfile::tempdir().unwrap();
        let e = engine(&dir);
        assert_eq!(e.transition(&serde_json::json!(2))["ok"], true);
        assert_eq!(check(&e, "/bin/ls", &["ls", "-la"]), Verdict::Allow);
    }

    #[test]
    fn exec_verdicts_are_logged_with_tool_argv_and_rule() {
        let dir = tempfile::tempdir().unwrap();
        let e = engine(&dir);
        let _ = check(&e, "/usr/bin/git", &["git", "push", "--force"]);
        let _ = check(&e, "/usr/bin/git", &["git", "status"]);
        let log = std::fs::read_to_string(dir.path().join("decisions.jsonl")).unwrap();
        let execs: Vec<_> = parse_log(&log).into_iter().filter(|r| r.kind == Kind::Exec).collect();
        assert_eq!(execs.len(), 2);
        assert_eq!(execs[0].source, Source::Exec);
        assert_eq!(execs[0].tool.as_deref(), Some("/usr/bin/git"));
        assert_eq!(execs[0].argv, ["git", "push", "--force"]);
        assert_eq!(execs[0].verdict, "deny");
        assert!(execs[0].rule.contains("git push"), "{}", execs[0].rule);
        assert_eq!(execs[1].verdict, "allow");
        assert_eq!(execs[0].step, Some(1));
    }

    #[test]
    fn an_exec_check_does_not_wait_on_a_running_gate() {
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let started = dir.path().join("gate-started");
        let prompt = format!(
            "# Step 1: Work\n[allowed]: code-read\n[disallowed]: git push\n[gate]: touch {} && \
             sleep 3\n[transition]: 2\n\n# Step 2: Done\n[allowed]: code-read\n[transition]: \
             Exit\n",
            started.display()
        );
        let e = Arc::new(WorkflowEngine::new(
            Workflow::compile(&prompt).unwrap().into_steps(),
            parse_guard_rules_str(RULES),
            dir.path().join("state"),
            dir.path().join("policy.json"),
            vec![],
            dir.path().to_path_buf(),
            None,
            false,
        ));
        let transition = {
            let e = Arc::clone(&e);
            std::thread::spawn(move || e.transition(&serde_json::json!(2)))
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !started.exists() {
            assert!(Instant::now() < deadline, "the gate never started");
            std::thread::sleep(Duration::from_millis(10));
        }

        let start = Instant::now();
        let checked = {
            let e = Arc::clone(&e);
            super::decide::within(Duration::from_secs(1), move || {
                (check(&e, "/usr/bin/git", &["git", "push"]), e.why_header())
            })
        };
        let elapsed = start.elapsed();
        let (verdict, header) = checked.expect("exec check must not wait for the gate");
        assert!(elapsed < Duration::from_secs(1), "waited {elapsed:?}");
        assert!(denied(&verdict), "step 1 stays in force while its gate runs: {verdict:?}");
        assert!(header.starts_with("step 1"), "{header}");

        assert_eq!(transition.join().unwrap()["ok"], true);
        assert_eq!(check(&e, "/usr/bin/git", &["git", "push"]), Verdict::Allow);
    }

    #[test]
    fn no_active_step_allows() {
        let dir = tempfile::tempdir().unwrap();
        let e = WorkflowEngine::new(
            std::collections::BTreeMap::new(),
            std::collections::HashMap::new(),
            dir.path().join("state"),
            dir.path().join("policy.json"),
            vec![],
            dir.path().to_path_buf(),
            None,
            false,
        );
        assert_eq!(check(&e, "/usr/bin/git", &["git", "push"]), Verdict::Allow);
    }
}
