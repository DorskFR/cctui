//! Static validation of a compiled workflow against its guard-rules sets.
//!
//! The engine enforces a prompt at run time; the linter catches the classes of
//! authoring mistake the engine would otherwise absorb silently — a typo'd set
//! name that decays into an inert literal keyword or an empty egress set, a
//! transition to a step that does not exist, an unreachable step or one with no
//! path to `Exit`, and a duplicate step number the step map would collapse. It
//! also resolves every step's policy to concrete keyword phrases and `host:port`
//! entries (`--explain`), the single view of the surface a prompt permits.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

use crate::ir::{NetworkDefault, Rule, Workflow};
use crate::parser::{expand_set, parse_keywords};

/// Severity of a lint [`Diagnostic`]. Any [`Severity::Error`] fails the lint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => f.write_str("error"),
            Self::Warning => f.write_str("warning"),
        }
    }
}

/// One finding: a severity, the step it concerns (if any), and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub step: Option<u32>,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.step {
            Some(n) => write!(f, "{}: Step {n}: {}", self.severity, self.message),
            None => write!(f, "{}: {}", self.severity, self.message),
        }
    }
}

/// A step's policy with every set reference expanded to concrete members — the
/// `--explain` view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStep {
    pub id: u32,
    pub title: String,
    pub allowed: Vec<String>,
    pub disallowed: Vec<String>,
    pub network: Vec<String>,
    pub network_open: bool,
    pub transitions: Vec<u32>,
    pub exit: bool,
    pub gate: bool,
    pub judge: usize,
    pub max_visits: Option<u32>,
    pub transition_gates: Vec<u32>,
}

/// The outcome of a lint pass: findings plus the resolved per-step policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintReport {
    pub diagnostics: Vec<Diagnostic>,
    pub resolved: Vec<ResolvedStep>,
}

impl LintReport {
    /// Whether any diagnostic is an [`Severity::Error`].
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }
}

/// A single set-referencing token that is not a defined set but reads like one
/// (hyphenated, no whitespace, set-name charset) — almost certainly a typo for a
/// real set, which the engine would otherwise treat as an inert literal keyword.
fn looks_like_set_ref(token: &str) -> bool {
    !token.is_empty()
        && token.contains('-')
        && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Resolve a `[network]` token list to concrete `host:port` entries, mirroring
/// the engine's `expand_network_rules`.
fn resolve_network(tokens: &[String], tool_sets: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut out = Vec::new();
    for item in tokens {
        let mut seen = HashSet::new();
        let mut expanded = Vec::new();
        expand_set(item, tool_sets, &mut seen, &mut expanded);
        for entry in expanded {
            let entry = entry.trim();
            if entry.contains(':') && !entry.starts_with('[') {
                out.push(entry.to_string());
            }
        }
    }
    out
}

fn rule_tokens(rule: &Rule) -> &[String] {
    match rule {
        Rule::List(items) => items,
        Rule::Unrestricted | Rule::Wildcard => &[],
    }
}

/// Lint a compiled [`Workflow`] against its guard-rules `tool_sets`.
///
/// `declared_ids` is every step id in authoring order *including repeats*, so
/// the markdown frontend (whose step map collapses duplicates) can still surface
/// a duplicate `# Step N` heading; pass the raw heading numbers there and the IR
/// step ids for the JSON frontend.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn lint(
    workflow: &Workflow,
    tool_sets: &HashMap<String, Vec<String>>,
    declared_ids: &[u32],
) -> LintReport {
    let mut diagnostics = Vec::new();
    let default_allow = matches!(workflow.network_default, Some(NetworkDefault::Allow));

    report_duplicate_ids(declared_ids, &mut diagnostics);

    if workflow.steps.is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            step: None,
            message: "no steps defined — prompt is unguarded".to_string(),
        });
        return LintReport { diagnostics, resolved: Vec::new() };
    }

    let defined: BTreeSet<u32> = workflow.steps.iter().map(|s| s.id).collect();
    let mut resolved = Vec::with_capacity(workflow.steps.len());

    for step in &workflow.steps {
        let sid = step.id;

        for token in rule_tokens(&step.allowed).iter().chain(rule_tokens(&step.disallowed)) {
            if looks_like_set_ref(token) && !tool_sets.contains_key(token) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    step: Some(sid),
                    message: format!(
                        "unknown set '{token}' in [allowed]/[disallowed] — it is not a defined \
                         set and would be matched as a literal keyword"
                    ),
                });
            }
        }

        for token in &step.network {
            if !tool_sets.contains_key(token) && !token.contains(':') {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    step: Some(sid),
                    message: format!(
                        "unknown network set '{token}' — not a defined set and not a literal \
                         host:port, so it grants nothing"
                    ),
                });
            }
        }

        for target in &step.transition.to {
            if !defined.contains(target) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    step: Some(sid),
                    message: format!("[transition] targets undefined Step {target}"),
                });
            }
        }

        for target in step.transition.gates.keys() {
            if !step.transition.to.contains(target) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    step: Some(sid),
                    message: format!(
                        "guard block declares a transition gate to Step {target}, which is not a \
                         declared transition target"
                    ),
                });
            }
        }

        if step.max_visits == Some(0) {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                step: Some(sid),
                message: "max-visits: 0 makes the step impossible to enter".to_string(),
            });
        }

        report_contradictions(step, tool_sets, &mut diagnostics);

        let network_open = step.network.iter().any(|t| t == "*");
        if step.network.is_empty() && default_allow {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                step: Some(sid),
                message: "no [network] under [network-default]: allow — egress is silently open"
                    .to_string(),
            });
        }

        resolved.push(ResolvedStep {
            id: sid,
            title: step.title.clone(),
            allowed: parse_keywords(&step.allowed.to_raw(), tool_sets),
            disallowed: parse_keywords(&step.disallowed.to_raw(), tool_sets),
            network: resolve_network(&step.network, tool_sets),
            network_open,
            transitions: step.transition.to.clone(),
            exit: step.transition.exit,
            gate: step.gate.is_some(),
            judge: step.judge.len(),
            max_visits: step.max_visits,
            transition_gates: step.transition.gates.keys().copied().collect(),
        });
    }

    report_reachability(workflow, &defined, &mut diagnostics);

    diagnostics.sort_by_key(|d| (d.step.unwrap_or(0), d.severity == Severity::Warning));
    LintReport { diagnostics, resolved }
}

fn report_duplicate_ids(declared_ids: &[u32], diagnostics: &mut Vec<Diagnostic>) {
    let mut seen = HashSet::new();
    let mut flagged = HashSet::new();
    for &id in declared_ids {
        if !seen.insert(id) && flagged.insert(id) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                step: Some(id),
                message: format!(
                    "duplicate step number {id} — the later definition silently overwrites the \
                     earlier"
                ),
            });
        }
    }
}

fn report_contradictions(
    step: &crate::ir::WorkflowStep,
    tool_sets: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let allowed = parse_keywords(&step.allowed.to_raw(), tool_sets);
    let disallowed = parse_keywords(&step.disallowed.to_raw(), tool_sets);
    let allow_wild = matches!(step.allowed, Rule::Wildcard);
    let disallow_wild = matches!(step.disallowed, Rule::Wildcard);

    if disallow_wild && (allow_wild || allowed.is_empty()) {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            step: Some(step.id),
            message: "[disallowed]: * with no overriding [allowed] blocks every tool".to_string(),
        });
    }

    let allow_set: HashSet<&String> = allowed.iter().collect();
    let mut overlap: Vec<&String> = disallowed.iter().filter(|k| allow_set.contains(*k)).collect();
    overlap.sort();
    overlap.dedup();
    for kw in overlap {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            step: Some(step.id),
            message: format!("'{kw}' is in both [allowed] and [disallowed] — disallow wins"),
        });
    }
}

/// Flag steps unreachable from the entry step, and steps with no path to `Exit`.
/// The entry is the lowest step number, matching the engine's ordered start.
fn report_reachability(
    workflow: &Workflow,
    defined: &BTreeSet<u32>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(entry) = defined.iter().next().copied() else {
        return;
    };
    let edges: HashMap<u32, Vec<u32>> = workflow
        .steps
        .iter()
        .map(|s| (s.id, s.transition.to.iter().copied().filter(|t| defined.contains(t)).collect()))
        .collect();
    let exits: HashSet<u32> =
        workflow.steps.iter().filter(|s| s.transition.exit).map(|s| s.id).collect();

    let mut reachable = HashSet::new();
    let mut stack = vec![entry];
    while let Some(id) = stack.pop() {
        if reachable.insert(id)
            && let Some(next) = edges.get(&id)
        {
            stack.extend(next.iter().copied());
        }
    }

    // Reverse fixpoint: a step reaches Exit if it declares Exit or any successor
    // reaches Exit.
    let mut reaches_exit = exits;
    loop {
        let mut changed = false;
        for (id, next) in &edges {
            if !reaches_exit.contains(id) && next.iter().any(|t| reaches_exit.contains(t)) {
                reaches_exit.insert(*id);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for &id in defined {
        if !reachable.contains(&id) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                step: Some(id),
                message: format!("unreachable — no transition path from entry Step {entry}"),
            });
        }
        if !reaches_exit.contains(&id) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                step: Some(id),
                message: "no path to Exit — the workflow can never terminate from here".to_string(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_guard_rules_str;

    const RULES: &str = "\
[code-read]: Read, Grep, Glob
[code-write]: Edit, Write
[all-read]: code-read
[net-claude]: api.example.com:443
[net-github]: github.example.com:443, github.example.com:22
";

    fn sets() -> HashMap<String, Vec<String>> {
        parse_guard_rules_str(RULES)
    }

    fn lint_md(md: &str) -> LintReport {
        let wf = Workflow::compile(md).unwrap();
        let ids = crate::parser::step_heading_numbers(md);
        lint(&wf, &sets(), &ids)
    }

    fn errors(report: &LintReport) -> Vec<&str> {
        report
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.message.as_str())
            .collect()
    }

    #[test]
    fn clean_workflow_has_no_errors() {
        let md = "\
# Step 1: Research
[allowed]: all-read
[network]: net-claude, net-github
[transition]: 2, Exit

# Step 2: Implement
[allowed]: all-read, code-write
[network]: net-claude
[transition]: Exit
";
        let report = lint_md(md);
        assert!(!report.has_errors(), "{:?}", report.diagnostics);
        assert_eq!(report.resolved.len(), 2);
        assert!(report.resolved[0].network.contains(&"api.example.com:443".to_string()));
    }

    #[test]
    fn unknown_tool_set_is_error() {
        let md = "\
# Step 1
[allowed]: all-reads
[network]: net-claude
[transition]: Exit
";
        assert!(errors(&lint_md(md)).iter().any(|m| m.contains("unknown set 'all-reads'")));
    }

    #[test]
    fn unknown_network_set_is_error() {
        let md = "\
# Step 1
[allowed]: all-read
[network]: net-guthub
[transition]: Exit
";
        assert!(
            errors(&lint_md(md)).iter().any(|m| m.contains("unknown network set 'net-guthub'"))
        );
    }

    #[test]
    fn literal_host_port_is_not_flagged() {
        let md = "\
# Step 1
[network]: extra.example.com:443
[transition]: Exit
";
        assert!(!lint_md(md).has_errors());
    }

    #[test]
    fn literal_command_keyword_is_not_flagged() {
        let md = "\
# Step 1
[allowed]: curl, git commit, Read
[network]: net-claude
[transition]: Exit
";
        assert!(!lint_md(md).has_errors());
    }

    #[test]
    fn transition_to_undefined_step_is_error() {
        let md = "\
# Step 1
[transition]: 5, Exit
";
        assert!(errors(&lint_md(md)).iter().any(|m| m.contains("undefined Step 5")));
    }

    #[test]
    fn unreachable_step_is_error() {
        let md = "\
# Step 1
[transition]: Exit

# Step 2
[transition]: Exit
";
        assert!(errors(&lint_md(md)).iter().any(|m| m.contains("unreachable")));
    }

    #[test]
    fn no_path_to_exit_is_error() {
        let md = "\
# Step 1
[transition]: 2

# Step 2
[transition]: 1
";
        let report = lint_md(md);
        assert!(errors(&report).iter().any(|m| m.contains("no path to Exit")));
    }

    #[test]
    fn duplicate_step_number_is_error() {
        let md = "\
# Step 1
[transition]: Exit

# Step 1: again
[transition]: Exit
";
        assert!(errors(&lint_md(md)).iter().any(|m| m.contains("duplicate step number 1")));
    }

    #[test]
    fn missing_network_warns_only_under_default_allow() {
        let deny = "\
# Step 1
[allowed]: all-read
[transition]: Exit
";
        assert!(lint_md(deny).diagnostics.iter().all(|d| !d.message.contains("silently open")));

        let allow = "\
[network-default]: allow

# Step 1
[allowed]: all-read
[transition]: Exit
";
        assert!(lint_md(allow).diagnostics.iter().any(|d| d.message.contains("silently open")));
    }

    #[test]
    fn contradiction_disallow_wildcard_blocks_all() {
        let md = "\
# Step 1
[disallowed]: *
[transition]: Exit
";
        assert!(
            lint_md(md).diagnostics.iter().any(|d| d.message.contains("blocks every tool")),
            "expected a block-all warning"
        );
    }

    #[test]
    fn contradiction_overlap_warns() {
        let md = "\
# Step 1
[allowed]: Write
[disallowed]: code-write
[network]: net-claude
[transition]: Exit
";
        assert!(lint_md(md).diagnostics.iter().any(|d| d.message.contains("both [allowed]")));
    }

    #[test]
    fn empty_workflow_warns() {
        let report = lint_md("Just prose, no steps.\n");
        assert!(!report.has_errors());
        assert!(report.diagnostics.iter().any(|d| d.message.contains("unguarded")));
    }

    #[test]
    fn transition_gate_to_undeclared_target_is_error() {
        let wf = Workflow::from_json(
            r#"{"steps":[
                {"id":1,"transition":{"to":[2],"gates":{"3":"make x"}}},
                {"id":2,"transition":{"exit":true}}
            ]}"#,
        )
        .unwrap();
        let report = lint(&wf, &sets(), &[1, 2]);
        assert!(
            errors(&report).iter().any(|m| m.contains("transition gate to Step 3")),
            "{:?}",
            report.diagnostics
        );
    }

    #[test]
    fn transition_gate_to_declared_target_is_clean() {
        let md = "\
# Step 1
[transition]: Exit
```guard
transitions: [{to: 2, gate: make test}]
```

# Step 2
[transition]: Exit
";
        assert!(!lint_md(md).has_errors(), "{:?}", lint_md(md).diagnostics);
    }

    #[test]
    fn max_visits_zero_warns() {
        let md = "\
# Step 1
[transition]: Exit
```guard
max-visits: 0
```
";
        assert!(lint_md(md).diagnostics.iter().any(|d| d.message.contains("max-visits: 0")));
    }

    #[test]
    fn resolved_expands_nested_sets_and_hosts() {
        let md = "\
# Step 1
[allowed]: all-read, code-write
[network]: net-github
[transition]: Exit
";
        let report = lint_md(md);
        let step = &report.resolved[0];
        assert!(step.allowed.contains(&"Read".to_string()));
        assert!(step.allowed.contains(&"Write".to_string()));
        assert_eq!(
            step.network,
            vec!["github.example.com:443".to_string(), "github.example.com:22".to_string()]
        );
    }
}
