//! Human gate rule sets; rules only allow, the request invariants stay in
//! [`crate::gate`].

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use anyhow::{Context as _, bail};
use serde::Deserialize;

const BUILTIN: &str = include_str!("gate_builtin.json");

pub const UNENFORCED: &str = "outwardHeldForApproval";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileSpec {
    gates: BTreeMap<String, SetSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetSpec {
    #[serde(default)]
    extends: Option<String>,
    #[serde(default)]
    protected_branches: Vec<String>,
    #[serde(default)]
    hosts: BTreeMap<String, HostSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostSpec {
    #[serde(default)]
    kind: Option<HostKind>,
    #[serde(default)]
    push: Option<Push>,
    #[serde(default)]
    rest: Vec<RestSpec>,
    #[serde(default)]
    mutations: Vec<MutationSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestSpec {
    method: Methods,
    path: String,
    #[serde(default)]
    check: Option<Check>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Methods {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MutationSpec {
    Name(String),
    Rule(MutationRuleSpec),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MutationRuleSpec {
    name: String,
    #[serde(default)]
    check: Option<Check>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostKind {
    Api,
    Rest,
    Git,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Push {
    #[default]
    Refuse,
    UnprotectedBranches,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Check {
    BranchInPath,
    /// GraphQL checks the `input` variable instead of the body.
    OnlyKeys(Vec<String>),
    IsTrue(String),
    /// Absent names the default branch, which is refused.
    BranchField(String),
    RefField(String),
}

#[derive(Debug, Clone, Default)]
pub struct RuleBook {
    sets: BTreeMap<String, RuleSet>,
}

#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    protected: Vec<String>,
    hosts: BTreeMap<String, HostRules>,
}

#[derive(Debug, Clone)]
pub struct HostRules {
    pub kind: HostKind,
    pub push: Push,
    rest: Vec<RestRule>,
    mutations: Vec<MutationRule>,
}

#[derive(Debug, Clone)]
pub struct RestRule {
    methods: Option<Vec<String>>,
    path: PathPattern,
    pub check: Option<Check>,
}

#[derive(Debug, Clone)]
pub struct MutationRule {
    name: String,
    pub check: Option<Check>,
}

#[derive(Debug, Clone)]
struct PathPattern {
    repo: bool,
    segments: Vec<Option<String>>,
    tail: bool,
}

impl RuleBook {
    pub fn builtin() -> anyhow::Result<Self> {
        Self::default().layered(BUILTIN).context("built-in gate rule sets")
    }

    pub fn with_file(json: &str) -> anyhow::Result<Self> {
        Self::builtin()?.layered(json)
    }

    fn layered(&self, json: &str) -> anyhow::Result<Self> {
        let file: FileSpec = serde_json::from_str(json)?;
        let mut sets = self.sets.clone();
        for name in file.gates.keys() {
            if name.is_empty() || name.trim() != name {
                bail!("gate name {name:?} is blank or padded");
            }
            if name == UNENFORCED {
                bail!("gate name `{UNENFORCED}` is reserved");
            }
            let set = resolve(name, &self.sets, &file.gates, &mut Vec::new())
                .with_context(|| format!("gate `{name}`"))?;
            sets.insert(name.clone(), set);
        }
        Ok(Self { sets })
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&RuleSet> {
        self.sets.get(name)
    }
}

/// `extends` naming the set itself refers to the base set of that name.
fn resolve<'a>(
    name: &'a str,
    base: &BTreeMap<String, RuleSet>,
    file: &'a BTreeMap<String, SetSpec>,
    stack: &mut Vec<&'a str>,
) -> anyhow::Result<RuleSet> {
    let spec = file.get(name).with_context(|| format!("no gate `{name}`"))?;
    stack.push(name);
    let parent = match spec.extends.as_deref() {
        None => RuleSet::default(),
        Some(parent) if parent == name => base
            .get(parent)
            .cloned()
            .with_context(|| format!("extends itself, but there is no built-in `{parent}`"))?,
        Some(parent) if file.contains_key(parent) => {
            if stack.contains(&parent) {
                bail!("`extends` cycles through `{parent}`");
            }
            resolve(parent, base, file, stack)?
        }
        Some(parent) => {
            base.get(parent).cloned().with_context(|| format!("extends unknown gate `{parent}`"))?
        }
    };
    stack.pop();
    parent.extended(spec)
}

impl RuleSet {
    fn extended(mut self, spec: &SetSpec) -> anyhow::Result<Self> {
        self.protect(&spec.protected_branches);
        for (host, hs) in &spec.hosts {
            let key = host.trim().to_ascii_lowercase();
            if key.is_empty() {
                bail!("a blank host");
            }
            let rules = match self.hosts.entry(key) {
                Entry::Occupied(e) => e.into_mut(),
                Entry::Vacant(e) => {
                    let kind = hs.kind.with_context(|| format!("host `{host}` needs a `kind`"))?;
                    e.insert(HostRules {
                        kind,
                        push: Push::Refuse,
                        rest: Vec::new(),
                        mutations: Vec::new(),
                    })
                }
            };
            rules.apply(hs).with_context(|| format!("host `{host}`"))?;
        }
        Ok(self)
    }

    fn protect(&mut self, branches: &[String]) {
        for branch in branches.iter().map(String::as_str).map(str::trim).filter(|b| !b.is_empty()) {
            if !self.protected.iter().any(|p| p.eq_ignore_ascii_case(branch)) {
                self.protected.push(branch.to_owned());
            }
        }
    }

    #[must_use]
    pub fn with_protected(mut self, extra: &[String]) -> Self {
        self.protect(extra);
        self
    }

    #[must_use]
    pub fn is_protected(&self, branch: &str) -> bool {
        self.protected.iter().any(|p| p.eq_ignore_ascii_case(branch))
    }

    #[must_use]
    pub fn host(&self, host: &str) -> Option<&HostRules> {
        self.hosts.iter().find(|(h, _)| h.eq_ignore_ascii_case(host)).map(|(_, r)| r)
    }
}

impl HostRules {
    fn apply(&mut self, spec: &HostSpec) -> anyhow::Result<()> {
        if let Some(kind) = spec.kind {
            self.kind = kind;
        }
        if let Some(push) = spec.push {
            if self.kind != HostKind::Git {
                bail!("`push` applies to a `git` host only");
            }
            self.push = push;
        }
        for rest in &spec.rest {
            let rule = RestRule::compile(rest).with_context(|| format!("path `{}`", rest.path))?;
            self.rest.push(rule);
        }
        for mutation in &spec.mutations {
            self.mutations.push(MutationRule::compile(mutation)?);
        }
        match self.kind {
            HostKind::Git if !self.rest.is_empty() => bail!("a `git` host takes no `rest` rules"),
            HostKind::Git | HostKind::Rest if !self.mutations.is_empty() => {
                bail!("only an `api` host takes `mutations`")
            }
            _ => Ok(()),
        }
    }

    pub fn rest_rules<'a>(
        &'a self,
        method: &'a str,
        lower: &'a [String],
    ) -> impl Iterator<Item = (&'a RestRule, usize)> + 'a {
        self.rest.iter().filter_map(move |r| r.matches(method, lower).map(|at| (r, at)))
    }

    pub fn mutation_rules<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a MutationRule> {
        self.mutations.iter().filter(move |m| m.name == name)
    }
}

impl RestRule {
    fn compile(spec: &RestSpec) -> anyhow::Result<Self> {
        let methods = match &spec.method {
            Methods::One(m) => std::slice::from_ref(m),
            Methods::Many(ms) => ms.as_slice(),
        };
        let methods: Vec<String> = methods.iter().map(|m| m.trim().to_ascii_uppercase()).collect();
        if methods.is_empty() || methods.iter().any(String::is_empty) {
            bail!("a blank `method`");
        }
        let methods = (!methods.iter().any(|m| m == "*")).then_some(methods);
        let path = PathPattern::parse(&spec.path)?;
        if spec.check == Some(Check::BranchInPath) && !path.tail {
            bail!("`branchInPath` needs a path ending in `**`");
        }
        check_fields(spec.check.as_ref())?;
        Ok(Self { methods, path, check: spec.check.clone() })
    }

    fn matches(&self, method: &str, lower: &[String]) -> Option<usize> {
        if self.methods.as_ref().is_some_and(|ms| !ms.iter().any(|m| m == method)) {
            return None;
        }
        self.path.matches(lower)
    }
}

impl MutationRule {
    fn compile(spec: &MutationSpec) -> anyhow::Result<Self> {
        let (name, check) = match spec {
            MutationSpec::Name(name) => (name, None),
            MutationSpec::Rule(rule) => (&rule.name, rule.check.clone()),
        };
        if name.is_empty() || name.trim() != name {
            bail!("mutation name {name:?} is blank or padded");
        }
        if check == Some(Check::BranchInPath) {
            bail!("mutation `{name}`: `branchInPath` applies to a REST rule only");
        }
        check_fields(check.as_ref()).with_context(|| format!("mutation `{name}`"))?;
        Ok(Self { name: name.clone(), check })
    }
}

fn check_fields(check: Option<&Check>) -> anyhow::Result<()> {
    match check {
        Some(Check::IsTrue(f) | Check::BranchField(f) | Check::RefField(f)) if f.is_empty() => {
            bail!("a check on a blank field")
        }
        _ => Ok(()),
    }
}

impl PathPattern {
    /// `{repo}` may lead the path (`repos/{owner}/{repo}` or
    /// `repositories/{id}`), `**` may end it (zero or more segments), `*` is
    /// one segment.
    fn parse(raw: &str) -> anyhow::Result<Self> {
        let mut parts: Vec<&str> = raw.split('/').filter(|s| !s.is_empty()).collect();
        let repo = parts.first() == Some(&"{repo}");
        if repo {
            parts.remove(0);
        }
        let tail = parts.last() == Some(&"**");
        if tail {
            parts.pop();
        }
        let mut segments = Vec::with_capacity(parts.len());
        for part in parts {
            if part.contains(['{', '}']) || part.contains("**") {
                bail!("`{part}`: `{{repo}}` may only lead a path and `**` only end it");
            }
            segments.push((part != "*").then(|| part.to_ascii_lowercase()));
        }
        Ok(Self { repo, segments, tail })
    }

    fn matches(&self, lower: &[String]) -> Option<usize> {
        let start = if self.repo { repo_prefix(lower)? } else { 0 };
        let rest = &lower[start..];
        let n = self.segments.len();
        let fits = if self.tail { rest.len() >= n } else { rest.len() == n };
        let same = self.segments.iter().zip(rest).all(|(w, g)| w.as_ref().is_none_or(|s| s == g));
        (fits && same).then_some(start + n)
    }
}

fn repo_prefix(lower: &[String]) -> Option<usize> {
    match lower.first().map(String::as_str) {
        Some("repos") if lower.len() >= 3 => Some(3),
        Some("repositories") if lower.len() >= 2 => Some(2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lower(path: &str) -> Vec<String> {
        path.split('/').filter(|s| !s.is_empty()).map(str::to_owned).collect()
    }

    #[test]
    fn builtin_rule_sets_parse() {
        let book = RuleBook::builtin().unwrap();
        for name in ["noPush", "draftPrOnly"] {
            let set = book.get(name).unwrap_or_else(|| panic!("{name}"));
            for host in ["github.com", "www.github.com", "gist.github.com", "api.github.com"] {
                assert!(set.host(host).is_some(), "{name} {host}");
            }
            assert_eq!(set.host("uploads.github.com").unwrap().kind, HostKind::Rest);
            assert!(set.is_protected("Main") && set.is_protected("trunk"), "{name}");
        }
        assert!(book.get(UNENFORCED).is_none());
    }

    #[test]
    fn path_patterns_match_repo_prefix_wildcards_and_tail() {
        let p = PathPattern::parse("{repo}/git/refs/heads/**").unwrap();
        assert_eq!(p.matches(&lower("repos/o/r/git/refs/heads/a/b")), Some(6));
        assert_eq!(p.matches(&lower("repositories/42/git/refs/heads")), Some(5));
        assert_eq!(p.matches(&lower("repos/o/git/refs/heads/a")), None);
        let p = PathPattern::parse("/user-attachments/assets").unwrap();
        assert!(p.matches(&lower("user-attachments/assets")).is_some());
        assert!(p.matches(&lower("user-attachments/assets/1")).is_none());
        let p = PathPattern::parse("{repo}/issues/*/comments").unwrap();
        assert!(p.matches(&lower("repos/o/r/issues/3/comments")).is_some());
        assert!(p.matches(&lower("repos/o/r/issues/comments")).is_none());
    }

    #[test]
    fn rules_files_that_do_not_hold_are_refused() {
        for bad in [
            "not json",
            r#"{"gate": {}}"#,
            r#"{"gates": {"x": {"extends": "nope"}}}"#,
            r#"{"gates": {"a": {"extends": "b"}, "b": {"extends": "a"}}}"#,
            r#"{"gates": {"custom": {"extends": "custom"}}}"#,
            r#"{"gates": {"outwardHeldForApproval": {}}}"#,
            r#"{"gates": {" padded": {}}}"#,
            r#"{"gates": {"x": {"hosts": {"example.com": {}}}}}"#,
            r#"{"gates": {"x": {"hosts": {"example.com": {"kind": "proxy"}}}}}"#,
            r#"{"gates": {"x": {"hosts": {"e.com": {"kind": "rest", "push": "refuse"}}}}}"#,
            r#"{"gates": {"x": {"hosts": {"e.com": {"kind": "git",
                "rest": [{"method": "POST", "path": "/a"}]}}}}}"#,
            r#"{"gates": {"x": {"hosts": {"e.com": {"kind": "rest", "mutations": ["a"]}}}}}"#,
            r#"{"gates": {"x": {"hosts": {"e.com": {"kind": "rest",
                "rest": [{"method": "POST", "path": "/a", "check": "branchInPath"}]}}}}}"#,
            r#"{"gates": {"x": {"hosts": {"e.com": {"kind": "rest",
                "rest": [{"method": "POST", "path": "/a/**/b"}]}}}}}"#,
            r#"{"gates": {"x": {"hosts": {"e.com": {"kind": "rest",
                "rest": [{"method": [], "path": "/a"}]}}}}}"#,
            r#"{"gates": {"x": {"hosts": {"e.com": {"kind": "rest",
                "rest": [{"method": "POST", "path": "/a", "check": "isTrue"}]}}}}}"#,
            r#"{"gates": {"x": {"hosts": {"e.com": {"kind": "rest",
                "rest": [{"method": "POST", "path": "/a", "allow": true}]}}}}}"#,
        ] {
            assert!(RuleBook::with_file(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_file_set_extends_a_builtin_a_file_set_or_itself() {
        let book = RuleBook::with_file(
            r#"{"gates": {
                "draftPrOnly": {"extends": "draftPrOnly", "protectedBranches": ["release"]},
                "reviewer": {"extends": "draftPrOnly", "protectedBranches": ["dev"]},
                "scratch": {"hosts": {"example.com": {"kind": "rest"}}}
            }}"#,
        )
        .unwrap();
        let draft = book.get("draftPrOnly").unwrap();
        assert!(draft.is_protected("release") && draft.is_protected("main"));
        let reviewer = book.get("reviewer").unwrap();
        assert!(reviewer.is_protected("release") && reviewer.is_protected("dev"));
        let scratch = book.get("scratch").unwrap();
        assert!(scratch.host("api.github.com").is_none() && scratch.host("Example.com").is_some());
        assert!(book.get("noPush").is_some(), "built-ins stay");
    }
}
