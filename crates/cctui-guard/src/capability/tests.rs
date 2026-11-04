use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde_json::json;

use super::*;
use crate::engine::{Verdict, WorkflowEngine};
use crate::ir::Workflow;

const TICKET_REGISTRY: &str = r#"
[capability."git:read"]
exec = [{ bin = "git", subcommands = ["status", "diff", "log", "show", "fetch"] }]
net  = ["github.com:443"]

[capability."git:push"]
requires = ["git:read"]
exec = [{ bin = "git", subcommands = ["push"], allow_flags = ["--set-upstream", "-u"] }]

[capability."gh:comment"]
exec = [{ bin = "gh", subcommands = ["pr comment", "issue comment"] }]
net  = ["api.github.com:443"]

[capability."make:test"]
exec = [{ bin = "make", subcommands = ["test", "lint"] }, { bin = "cargo" }]
"#;

const GOLDEN_EXEC_TABLE: &str = r#"{
  "cargo": [
    {
      "capability": "make:test"
    }
  ],
  "gh": [
    {
      "capability": "gh:comment",
      "subcommands": [
        "pr comment",
        "issue comment"
      ]
    }
  ],
  "git": [
    {
      "capability": "git:push",
      "subcommands": [
        "push"
      ],
      "allow_flags": [
        "--set-upstream",
        "-u"
      ]
    },
    {
      "capability": "git:read",
      "subcommands": [
        "status",
        "diff",
        "log",
        "show",
        "fetch"
      ]
    }
  ],
  "make": [
    {
      "capability": "make:test",
      "subcommands": [
        "test",
        "lint"
      ]
    }
  ]
}"#;

const GOLDEN_POLICY: &str = r#"{
  "allowed_hosts": [
    "github.com:443",
    "api.github.com:443",
    "extra.example.com:443",
    "callback.example.com:443"
  ],
  "default": "deny"
}"#;

const GRANT_PROMPT: &str = "\
# Step 1: Ship
[allowed]: Read, git:push, gh:comment, make:test
[network]: extra.example.com:443, github.com:443
[transition]: 2

# Step 2: Read only
[allowed]: Read, git:read
[transition]: Exit

# Step 3: Old style
[allowed]: Read, Bash
[disallowed]: git push
[transition]: Exit
";

fn registry(layers: &[(&str, &str)]) -> Registry {
    let mut r = Registry::default();
    for (text, layer) in layers {
        r.apply(text, layer).unwrap();
    }
    r
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(ToString::to_string).collect()
}

fn engine_for(dir: &tempfile::TempDir, prompt: &str, reg: &Registry) -> WorkflowEngine {
    let steps = Workflow::compile(prompt).unwrap().into_steps();
    let sets = HashMap::new();
    let grants = compile_steps(&steps, &sets, reg).unwrap();
    WorkflowEngine::new(
        steps,
        sets,
        dir.path().join("state"),
        dir.path().join("policy.json"),
        vec!["callback.example.com:443".to_string()],
        dir.path().to_path_buf(),
        None,
        false,
    )
    .with_grants(grants)
}

fn exec(engine: &WorkflowEngine, path: &str, argv: &[&str]) -> Verdict {
    let argv = names(argv);
    engine.check_exec(path, &argv, &[])
}

#[test]
fn golden_grant_list_compiles_to_the_exec_table() {
    let reg = registry(&[(TICKET_REGISTRY, "--capabilities")]);
    let grants = reg.compile(&names(&["git:push", "gh:comment", "make:test"])).unwrap();
    assert_eq!(grants.capabilities, ["git:push", "git:read", "gh:comment", "make:test"]);
    assert_eq!(grants.exec.to_json(), GOLDEN_EXEC_TABLE);
    assert_eq!(grants.hosts(), ["github.com:443", "api.github.com:443"]);
}

#[test]
fn golden_grant_list_compiles_to_policy_json() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(&[(TICKET_REGISTRY, "--capabilities")]);
    let _engine = engine_for(&dir, GRANT_PROMPT, &reg);
    let written = std::fs::read_to_string(dir.path().join("policy.json")).unwrap();
    assert_eq!(written, GOLDEN_POLICY);
}

#[test]
fn transition_writes_the_next_steps_granted_hosts() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(&[(TICKET_REGISTRY, "--capabilities")]);
    let engine = engine_for(&dir, GRANT_PROMPT, &reg);
    assert_eq!(engine.transition(&json!(2))["ok"], true);
    let policy: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("policy.json")).unwrap())
            .unwrap();
    assert_eq!(policy["allowed_hosts"], json!(["github.com:443", "callback.example.com:443"]));
    assert_eq!(policy["default"], "deny");
}

#[test]
fn requires_resolves_transitively_once_each() {
    let reg = registry(&[(
        r#"
[capability."a:top"]
requires = ["b:mid", "c:leaf"]
[capability."b:mid"]
requires = ["c:leaf"]
[capability."c:leaf"]
net = ["leaf.example:443"]
"#,
        "base",
    )]);
    let resolved: Vec<&str> =
        reg.resolve(&names(&["a:top"])).unwrap().iter().map(|c| c.name.as_str()).collect();
    assert_eq!(resolved, ["a:top", "b:mid", "c:leaf"]);
    assert_eq!(reg.compile(&names(&["a:top"])).unwrap().hosts(), ["leaf.example:443"]);
}

#[test]
fn require_cycles_are_detected() {
    let reg = registry(&[(
        r#"
[capability."a:x"]
requires = ["b:x"]
[capability."b:x"]
requires = ["c:x"]
[capability."c:x"]
requires = ["a:x"]
[capability."self:loop"]
requires = ["self:loop"]
"#,
        "base",
    )]);
    let err = reg.resolve(&names(&["a:x"])).unwrap_err();
    assert_eq!(err, ResolveError::Cycle(names(&["a:x", "b:x", "c:x", "a:x"])));
    assert_eq!(err.to_string(), "circular capability requires: a:x -> b:x -> c:x -> a:x");
    assert_eq!(
        reg.resolve(&names(&["self:loop"])).unwrap_err(),
        ResolveError::Cycle(names(&["self:loop", "self:loop"]))
    );
    assert!(reg.explain("b:x").is_err());
}

#[test]
fn unknown_capabilities_are_errors() {
    let reg = registry(&[("[capability.\"a:x\"]\nrequires = [\"gone:x\"]\n", "base")]);
    assert_eq!(
        reg.resolve(&names(&["a:x"])).unwrap_err().to_string(),
        "unknown capability 'gone:x' (required by 'a:x')"
    );
    assert_eq!(
        reg.resolve(&names(&["typo:x"])).unwrap_err().to_string(),
        "unknown capability 'typo:x'"
    );
}

#[test]
fn malformed_registries_are_rejected() {
    let bad = [
        ("[capability.\"test\"]\n", "namespace:name"),
        ("[capability.\"a:x\"]\nexec = [{ bin = \"/usr/bin/git\" }]\n", "bare binary"),
        ("[capability.\"a:x\"]\nexec = [{ bin = \"git\", subcommands = [] }]\n", "empty"),
        ("[capability.\"a:x\"]\nexec = [{ bin = \"git\", allow_flags = [\"force\"] }]\n", "flag"),
        ("[capability.\"a:x\"]\nnet = [\"github.com\"]\n", "host:port"),
        ("[capability.\"a:x\"]\nrequires = [\"Bash\"]\n", "not a capability"),
        ("[capability.\"a:x\"]\nexex = []\n", "unknown field"),
        ("[capability.\"a:x\"\n", "base"),
    ];
    for (text, needle) in bad {
        let err = Registry::default().apply(text, "base").unwrap_err();
        assert!(err.contains(needle), "{text:?}: {err}");
        assert!(err.starts_with("base"), "{err}");
    }
}

#[test]
fn capability_names_need_the_namespace_colon() {
    for yes in ["git:push", "gh:pr:comment", "make:test", "oracle:payments-sandbox"] {
        assert!(is_capability_name(yes), "{yes}");
    }
    for no in ["Bash", "git push", "localhost:9999", "Bash(npm:*)", "net-vcs", ":x", "x:", "a:b c"]
    {
        assert!(!is_capability_name(no), "{no}");
    }
}

#[test]
fn grants_are_authored_with_the_existing_allowed_syntax() {
    let wf = Workflow::compile(
        "[git-all]: git:read, git:push\n\n# Step 1\n[allowed]: Read, git-all, gh:comment, git \
         status\n[transition]: Exit\n",
    )
    .unwrap();
    let step = &wf.steps[0];
    assert_eq!(
        step.allowed,
        crate::ir::Rule::List(names(&["Read", "git-all", "gh:comment", "git status"]))
    );
    let sets: HashMap<String, Vec<String>> =
        wf.sets.iter().map(|s| (s.name.clone(), s.members.clone())).collect();
    assert_eq!(step_grants(step, &sets), ["git:read", "git:push", "gh:comment"]);
}

#[test]
fn layering_precedence_with_explain_output() {
    let base = r#"
[capability."git:read"]
description = "Inspect the repository."
exec = [{ bin = "git", subcommands = ["status"] }]
net = ["github.example.com:443"]

[capability."git:push"]
requires = ["git:read"]
exec = [{ bin = "git", subcommands = ["push"], allow_flags = ["--force"] }]
"#;
    let pack = r#"
[capability."git:push"]
requires = ["git:read"]
assumes = ["branch protection rejects force-push"]
exec = [{ bin = "git", subcommands = ["push"], allow_flags = ["--set-upstream", "-u"] }]
"#;
    let import = r#"
[capability."git:read"]
extend = true
net = ["github.example.com:22"]
exec = [{ bin = "rg" }]
"#;
    let reg = registry(&[
        (base, "--capabilities-base"),
        (pack, "--capabilities"),
        (import, "[rules] ./extra.toml"),
    ]);
    let expected = "\
git:push (defined in --capabilities)
  requires:
    git:push -> git:read (--capabilities)
  assumes:
    branch protection rejects force-push (git:push, --capabilities)
  exec:
    git push [flags: --set-upstream, -u] (git:push, --capabilities)
    git status [no flags] (git:read, --capabilities-base)
    rg (any arguments) (git:read, [rules] ./extra.toml)
  net:
    github.example.com:443 (git:read, --capabilities-base)
    github.example.com:22 (git:read, [rules] ./extra.toml)
";
    assert_eq!(reg.explain("git:push").unwrap(), expected);
    let read = reg.explain("git:read").unwrap();
    assert!(read.starts_with("git:read (defined in [rules] ./extra.toml)\n"), "{read}");
    assert!(read.contains("  Inspect the repository.\n"), "{read}");
    assert!(!read.contains("requires:"), "{read}");

    let grants = reg.compile(&names(&["git:push"])).unwrap();
    assert!(grants.exec.check("/usr/bin/git", &names(&["git", "push", "--force"])).is_err());
    assert!(grants.exec.check("/usr/bin/git", &names(&["git", "push", "-u"])).is_ok());
}

#[test]
fn explain_marks_empty_axes() {
    let reg = registry(&[("[capability.\"model:api\"]\nnet = [\"api.example.com:443\"]\n", "b")]);
    assert_eq!(
        reg.explain("model:api").unwrap(),
        "model:api (defined in b)\n  exec: (none)\n  net:\n    api.example.com:443 (model:api, b)\n"
    );
}

#[test]
fn registry_files_layer_base_pack_then_toml_imports() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, body: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        path
    };
    let base = write("base.toml", "[capability.\"a:x\"]\nnet = [\"base.example:443\"]\n");
    let pack = write("capabilities.toml", "[capability.\"a:x\"]\nnet = [\"pack.example:443\"]\n");
    write("extra.toml", "[capability.\"a:x\"]\nextend = true\nnet = [\"import.example:443\"]\n");
    let prompt = write(
        "task.md",
        "[rules]: ./extra.toml\n[rules]: ./missing.toml\n\n# Step 1\n[transition]: Exit\n",
    );
    let wf = Workflow::compile(&std::fs::read_to_string(&prompt).unwrap()).unwrap();
    let loaded = resolve_registry(
        Some(base.as_path()),
        Some(pack.as_path()),
        Some(prompt.as_path()),
        Some(&wf),
    );
    let cap = loaded.registry.get("a:x").unwrap();
    let hosts: Vec<(&str, &str)> =
        cap.net.iter().map(|h| (h.value.as_str(), h.layer.as_str())).collect();
    assert_eq!(
        hosts,
        [("pack.example:443", "--capabilities"), ("import.example:443", "[rules] ./extra.toml")]
    );
    assert_eq!(loaded.errors.len(), 1, "{:?}", loaded.errors);
    assert!(loaded.errors[0].contains("missing.toml"), "{:?}", loaded.errors);

    let none = resolve_registry(
        Some(Path::new("/nonexistent/base.toml")),
        Some(Path::new("/nonexistent/capabilities.toml")),
        None,
        None,
    );
    assert!(none.errors.is_empty() && none.registry.is_empty());
}

#[test]
fn adjacent_registry_sits_next_to_the_rules_file() {
    assert_eq!(
        adjacent(Path::new("/etc/claude-worker/guard-rules.md")),
        Path::new("/etc/claude-worker/capabilities.toml")
    );
}

#[test]
fn grant_steps_judge_execs_against_the_table() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(&[(TICKET_REGISTRY, "--capabilities")]);
    let engine = engine_for(&dir, GRANT_PROMPT, &reg);
    assert_eq!(exec(&engine, "/usr/bin/git", &["git", "-C", "/w", "push", "-u"]), Verdict::Allow);
    assert_eq!(exec(&engine, "/usr/bin/cargo", &["cargo", "test"]), Verdict::Allow);
    for (path, argv) in [
        ("/usr/bin/git", &["git", "push", "--force"][..]),
        ("/usr/bin/git", &["git", "reset", "--hard"][..]),
        ("/usr/bin/curl", &["curl", "https://x"][..]),
        ("/bin/ls", &["ls"][..]),
    ] {
        let v = exec(&engine, path, argv);
        assert!(matches!(&v, Verdict::Deny { reason } if reason.starts_with("[Step 1]")), "{v:?}");
    }
}

#[test]
fn steps_without_grants_keep_phrase_only_exec_checks() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(&[(TICKET_REGISTRY, "--capabilities")]);
    let prompt = "\
# Step 1: Old style
[allowed]: Read, Bash
[disallowed]: git push
[transition]: Exit
";
    let engine = engine_for(&dir, prompt, &reg);
    assert_eq!(exec(&engine, "/bin/ls", &["ls", "-la"]), Verdict::Allow);
    assert_eq!(exec(&engine, "/usr/bin/curl", &["curl", "x"]), Verdict::Allow);
    let push = names(&["git", "push"]);
    let v = engine.check_exec("/usr/bin/git", &push, &["git push".to_string()]);
    assert!(matches!(v, Verdict::Deny { .. }), "{v:?}");
}

#[test]
fn bash_hook_honours_grants_but_not_hidden_effects() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(&[(TICKET_REGISTRY, "--capabilities")]);
    let engine = engine_for(&dir, GRANT_PROMPT, &reg);
    let bash = |cmd: &str| engine.check("Bash", &json!({ "command": cmd }));
    assert_eq!(bash("git status && git push -u origin main"), Verdict::Allow);
    assert_eq!(bash("gh pr comment 3 && make lint"), Verdict::Allow);
    for cmd in [
        "git push --force",
        "git status > /tmp/out",
        "git log $(rm -rf /)",
        "curl https://example.com",
        "git status & curl x",
    ] {
        assert!(matches!(bash(cmd), Verdict::Deny { .. }), "{cmd}");
    }
    assert_eq!(engine.check("Read", &json!({ "file_path": "/w/a" })), Verdict::Allow);
    assert!(matches!(engine.check("Edit", &json!({ "file_path": "/w/a" })), Verdict::Deny { .. }));
}

#[test]
fn lint_flags_unresolvable_grants_and_redundant_network() {
    let reg = registry(&[(TICKET_REGISTRY, "--capabilities")]);
    let wf = Workflow::compile(
        "[net-gh]: api.github.com:443, other.example:443\n\n# Step 1\n[allowed]: gh:comment\n\
         [network]: net-gh\n[transition]: 2\n\n# Step 2\n[allowed]: typo:x\n[disallowed]: \
         git:push\n[transition]: Exit\n",
    )
    .unwrap();
    let sets: HashMap<String, Vec<String>> =
        wf.sets.iter().map(|s| (s.name.clone(), s.members.clone())).collect();
    let diags = lint(&wf, &sets, &reg);
    let rendered: Vec<String> = diags.iter().map(ToString::to_string).collect();
    assert_eq!(
        rendered,
        [
            "warning: Step 1: [network] host api.github.com:443 is already granted by capability \
             'gh:comment'; drop it from [network]",
            "warning: Step 2: capability 'git:push' in [disallowed] has no effect; capabilities \
             are only granted",
            "error: Step 2: unknown capability 'typo:x'",
        ]
    );
}

#[test]
fn compile_steps_reports_every_bad_step() {
    let reg = registry(&[(TICKET_REGISTRY, "--capabilities")]);
    let steps: BTreeMap<u32, _> = Workflow::compile(
        "# Step 1\n[allowed]: nope:a\n[transition]: 2\n\n# Step 2\n[allowed]: nope:b\n\
         [transition]: Exit\n",
    )
    .unwrap()
    .into_steps();
    let errs = compile_steps(&steps, &HashMap::new(), &reg).unwrap_err();
    assert_eq!(
        errs,
        ["Step 1: unknown capability 'nope:a'", "Step 2: unknown capability 'nope:b'"]
    );
}

#[test]
fn shipped_example_registry_loads_and_resolves() {
    let text = include_str!("../../../../deploy/examples/context-pack/capabilities.toml");
    let reg = registry(&[(text, "--capabilities")]);
    for cap in ["read:all", "write:all", "remote:write", "model:api"] {
        assert!(reg.compile(&names(&[cap])).is_ok(), "{cap}");
    }
    let grants = reg.compile(&names(&["read:all"])).unwrap();
    assert!(grants.exec.check("/usr/bin/git", &names(&["git", "log", "--oneline"])).is_ok());
    assert!(grants.exec.check("/usr/bin/git", &names(&["git", "push"])).is_err());
    assert!(grants.exec.check("/usr/bin/gh", &names(&["gh", "api", "-X", "POST", "x"])).is_err());
    let write = reg.compile(&names(&["remote:write"])).unwrap();
    assert!(write.exec.check("/usr/bin/git", &names(&["git", "push", "--force"])).is_err());
}
