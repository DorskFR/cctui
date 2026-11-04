//! The capability registry, the one authored policy source. A step grants
//! capabilities in `[allowed]` (`git:push`: the colon separates them from
//! harness tool names); both the exec table and `policy.json` derive from it.

pub mod matcher;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::ir::{Workflow, WorkflowStep};
use crate::lint::{Diagnostic, Severity};
use crate::parser::expand_set;

pub use matcher::{ExecRule, ExecTable};

pub const REGISTRY_FILE: &str = "capabilities.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sourced<T> {
    pub value: T,
    pub layer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecSpec {
    pub bin: String,
    #[serde(default)]
    pub subcommands: Option<Vec<String>>,
    #[serde(default)]
    pub allow_flags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityDef {
    #[serde(default)]
    extend: bool,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    requires: Vec<String>,
    #[serde(default)]
    exec: Vec<ExecSpec>,
    #[serde(default)]
    net: Vec<String>,
    #[serde(default)]
    assumes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    #[serde(default)]
    capability: BTreeMap<String, CapabilityDef>,
}

/// A capability with every entry tagged by the registry layer that wrote it.
/// `assumes` names a server-side rule (branch protection, RBAC) the capability
/// relies on for what argv policy cannot stop.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capability {
    pub name: String,
    pub description: Option<String>,
    pub defined_in: String,
    pub requires: Vec<Sourced<String>>,
    pub exec: Vec<Sourced<ExecSpec>>,
    pub net: Vec<Sourced<String>>,
    pub assumes: Vec<Sourced<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    Unknown { name: String, via: Option<String> },
    Cycle(Vec<String>),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { name, via: None } => write!(f, "unknown capability '{name}'"),
            Self::Unknown { name, via: Some(via) } => {
                write!(f, "unknown capability '{name}' (required by '{via}')")
            }
            Self::Cycle(path) => write!(f, "circular capability requires: {}", path.join(" -> ")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetGrant {
    pub host: String,
    pub capability: String,
}

/// What a step's grant list compiles to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants {
    pub capabilities: Vec<String>,
    pub exec: ExecTable,
    pub net: Vec<NetGrant>,
}

impl Grants {
    #[must_use]
    pub fn hosts(&self) -> Vec<String> {
        self.net.iter().map(|n| n.host.clone()).collect()
    }
}

/// `ns:name`, both halves starting with a letter, so `localhost:9999` and
/// `Bash(npm:*)` stay plain keywords.
#[must_use]
pub fn is_capability_name(token: &str) -> bool {
    let word = |s: &str| {
        s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    token.split_once(':').is_some_and(|(ns, rest)| word(ns) && rest.split(':').all(word))
}

fn validate(name: &str, def: &CapabilityDef) -> Result<(), String> {
    if !is_capability_name(name) {
        return Err("a capability name is `namespace:name` (e.g. `git:push`)".to_string());
    }
    if let Some(bad) = def.requires.iter().find(|r| !is_capability_name(r)) {
        return Err(format!("requires '{bad}' is not a capability name"));
    }
    for spec in &def.exec {
        if spec.bin.is_empty() || spec.bin.contains('/') || spec.bin.contains(char::is_whitespace)
        {
            return Err(format!("exec bin '{}' must be a bare binary name", spec.bin));
        }
        if let Some(subs) = &spec.subcommands
            && (subs.is_empty() || subs.iter().any(|s| s.trim().is_empty()))
        {
            return Err(format!("exec '{}' has an empty subcommand list or entry", spec.bin));
        }
        if let Some(flag) = spec.allow_flags.iter().find(|f| !f.starts_with('-') || f.len() < 2) {
            return Err(format!("exec '{}' allow_flags entry '{flag}' is not a flag", spec.bin));
        }
    }
    if let Some(host) = def.net.iter().find(|h| !h.contains(':')) {
        return Err(format!("net entry '{host}' is not host:port"));
    }
    Ok(())
}

fn sourced<T: Clone>(items: &[T], layer: &str) -> Vec<Sourced<T>> {
    items.iter().map(|v| Sourced { value: v.clone(), layer: layer.to_string() }).collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    capabilities: BTreeMap<String, Capability>,
}

impl Registry {
    /// Apply one registry layer. A later layer's definition replaces the
    /// capability; `extend = true` appends to it instead.
    pub fn apply(&mut self, text: &str, layer: &str) -> Result<(), String> {
        let file: RegistryFile = toml::from_str(text).map_err(|e| format!("{layer}: {e}"))?;
        for (name, def) in &file.capability {
            validate(name, def).map_err(|e| format!("{layer}: capability '{name}': {e}"))?;
        }
        for (name, def) in file.capability {
            let cap = self.capabilities.entry(name.clone()).or_default();
            if !def.extend {
                *cap = Capability::default();
            }
            cap.name = name;
            cap.defined_in = layer.to_string();
            if def.description.is_some() {
                cap.description = def.description;
            }
            cap.requires.extend(sourced(&def.requires, layer));
            cap.exec.extend(sourced(&def.exec, layer));
            cap.net.extend(sourced(&def.net, layer));
            cap.assumes.extend(sourced(&def.assumes, layer));
        }
        Ok(())
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Capability> {
        self.capabilities.get(name)
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }

    /// Every capability `grants` reaches through `requires`, each once, in
    /// depth-first pre-order.
    pub fn resolve(&self, grants: &[String]) -> Result<Vec<&Capability>, ResolveError> {
        let mut out = Vec::new();
        let mut done = HashSet::new();
        let mut stack = Vec::new();
        for grant in grants {
            self.visit(grant, &mut stack, &mut done, &mut out)?;
        }
        Ok(out)
    }

    fn visit<'a>(
        &'a self,
        name: &str,
        stack: &mut Vec<String>,
        done: &mut HashSet<String>,
        out: &mut Vec<&'a Capability>,
    ) -> Result<(), ResolveError> {
        if done.contains(name) {
            return Ok(());
        }
        if let Some(pos) = stack.iter().position(|s| s == name) {
            let mut cycle = stack[pos..].to_vec();
            cycle.push(name.to_string());
            return Err(ResolveError::Cycle(cycle));
        }
        let cap = self.capabilities.get(name).ok_or_else(|| ResolveError::Unknown {
            name: name.to_string(),
            via: stack.last().cloned(),
        })?;
        out.push(cap);
        stack.push(name.to_string());
        for req in &cap.requires {
            self.visit(&req.value, stack, done, out)?;
        }
        stack.pop();
        done.insert(name.to_string());
        Ok(())
    }

    pub fn compile(&self, grants: &[String]) -> Result<Grants, ResolveError> {
        let mut out = Grants::default();
        for cap in self.resolve(grants)? {
            out.capabilities.push(cap.name.clone());
            for spec in &cap.exec {
                out.exec.insert(
                    &spec.value.bin,
                    ExecRule {
                        capability: cap.name.clone(),
                        subcommands: spec.value.subcommands.clone(),
                        allow_flags: spec.value.allow_flags.clone(),
                        layer: spec.layer.clone(),
                    },
                );
            }
            for host in &cap.net {
                if !out.net.iter().any(|n| n.host == host.value) {
                    out.net
                        .push(NetGrant { host: host.value.clone(), capability: cap.name.clone() });
                }
            }
        }
        Ok(out)
    }

    /// The effective binaries, argv predicates and hosts of `name`, each
    /// annotated with the capability and layer it came from.
    pub fn explain(&self, name: &str) -> Result<String, ResolveError> {
        let resolved = self.resolve(&[name.to_string()])?;
        let root = resolved[0];
        let mut out = format!("{name} (defined in {})\n", root.defined_in);
        if let Some(description) = &root.description {
            let _ = writeln!(out, "  {description}");
        }
        let edges: Vec<String> = resolved
            .iter()
            .flat_map(|c| c.requires.iter().map(move |r| (c, r)))
            .map(|(c, r)| format!("{} -> {} ({})", c.name, r.value, r.layer))
            .collect();
        push_section(&mut out, "requires", &edges, false);
        let assumes: Vec<String> = resolved
            .iter()
            .flat_map(|c| c.assumes.iter().map(move |a| (c, a)))
            .map(|(c, a)| format!("{} ({}, {})", a.value, c.name, a.layer))
            .collect();
        push_section(&mut out, "assumes", &assumes, false);
        let exec: Vec<String> = resolved
            .iter()
            .flat_map(|c| c.exec.iter().map(move |e| (c, e)))
            .map(|(c, e)| format!("{} ({}, {})", describe_exec(&e.value), c.name, e.layer))
            .collect();
        push_section(&mut out, "exec", &exec, true);
        let net: Vec<String> = resolved
            .iter()
            .flat_map(|c| c.net.iter().map(move |h| (c, h)))
            .map(|(c, h)| format!("{} ({}, {})", h.value, c.name, h.layer))
            .collect();
        push_section(&mut out, "net", &net, true);
        Ok(out)
    }
}

fn push_section(out: &mut String, title: &str, lines: &[String], always: bool) {
    if lines.is_empty() {
        if always {
            let _ = writeln!(out, "  {title}: (none)");
        }
        return;
    }
    let _ = writeln!(out, "  {title}:");
    for line in lines {
        let _ = writeln!(out, "    {line}");
    }
}

fn describe_exec(spec: &ExecSpec) -> String {
    spec.subcommands.as_ref().map_or_else(
        || format!("{} (any arguments)", spec.bin),
        |subs| {
            let flags = if spec.allow_flags.is_empty() {
                "no flags".to_string()
            } else {
                format!("flags: {}", spec.allow_flags.join(", "))
            };
            format!("{} {} [{flags}]", spec.bin, subs.join(" | "))
        },
    )
}

/// The capability names `step` grants: its `[allowed]` tokens, sets expanded.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn step_grants(step: &WorkflowStep, tool_sets: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in step.allowed.expand(tool_sets) {
        if is_capability_name(&token) && !out.contains(&token) {
            out.push(token);
        }
    }
    out
}

/// Compile every step that grants capabilities. Steps without grants are
/// absent from the map and keep phrase-only exec checks.
#[allow(clippy::implicit_hasher)]
pub fn compile_steps(
    steps: &BTreeMap<u32, WorkflowStep>,
    tool_sets: &HashMap<String, Vec<String>>,
    registry: &Registry,
) -> Result<BTreeMap<u32, Grants>, Vec<String>> {
    let mut out = BTreeMap::new();
    let mut errors = Vec::new();
    for (id, step) in steps {
        let grants = step_grants(step, tool_sets);
        if grants.is_empty() {
            continue;
        }
        match registry.compile(&grants) {
            Ok(g) => {
                out.insert(*id, g);
            }
            Err(e) => errors.push(format!("Step {id}: {e}")),
        }
    }
    if errors.is_empty() { Ok(out) } else { Err(errors) }
}

fn network_hosts(tokens: &[String], tool_sets: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut out = Vec::new();
    for token in tokens {
        let mut seen = HashSet::new();
        let mut expanded = Vec::new();
        expand_set(token.trim(), tool_sets, &mut seen, &mut expanded);
        out.extend(expanded.into_iter().filter(|h| h.contains(':')));
    }
    out
}

const fn diag(severity: Severity, step: u32, message: String) -> Diagnostic {
    Diagnostic { severity, step: Some(step), message }
}

/// Capability findings: unresolvable grants, capability names in
/// `[disallowed]`, and `[network]` hosts a granted capability already covers.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn lint(
    workflow: &Workflow,
    tool_sets: &HashMap<String, Vec<String>>,
    registry: &Registry,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for step in &workflow.steps {
        for token in step.disallowed.expand(tool_sets).iter().filter(|t| is_capability_name(t)) {
            out.push(diag(
                Severity::Warning,
                step.id,
                format!(
                    "capability '{token}' in [disallowed] has no effect; capabilities are only \
                     granted"
                ),
            ));
        }
        let grants = step_grants(step, tool_sets);
        if grants.is_empty() {
            continue;
        }
        let compiled = match registry.compile(&grants) {
            Ok(c) => c,
            Err(e) => {
                out.push(diag(Severity::Error, step.id, e.to_string()));
                continue;
            }
        };
        for host in network_hosts(&step.network, tool_sets) {
            if let Some(grant) = compiled.net.iter().find(|n| n.host == host) {
                out.push(diag(
                    Severity::Warning,
                    step.id,
                    format!(
                        "[network] host {host} is already granted by capability '{}'; drop it \
                         from [network]",
                        grant.capability
                    ),
                ));
            }
        }
    }
    out
}

/// A `[rules]` import naming a capability registry rather than a set file.
#[must_use]
pub fn is_registry_import(import: &str) -> bool {
    Path::new(import).extension().is_some_and(|e| e.eq_ignore_ascii_case("toml"))
}

/// The registry file that sits next to a guard-rules file.
#[must_use]
pub fn adjacent(rules: &Path) -> PathBuf {
    rules.with_file_name(REGISTRY_FILE)
}

#[derive(Debug, Clone, Default)]
pub struct LoadedRegistry {
    pub registry: Registry,
    pub errors: Vec<String>,
}

fn apply_path(loaded: &mut LoadedRegistry, path: &Path, layer: &str, required: bool) {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            if let Err(e) = loaded.registry.apply(&text, layer) {
                loaded.errors.push(e);
            }
        }
        Err(e) if required || e.kind() != std::io::ErrorKind::NotFound => {
            loaded.errors.push(format!("{layer}: {} is unreadable: {e}", path.display()));
        }
        Err(_) => {}
    }
}

/// Load the layered registry, lowest to highest precedence: `base` (the
/// image) < `pack` (next to `--rules`) < each `.toml` `[rules]` import of the
/// prompt in authored order. Missing `base`/`pack` files are skipped; a
/// missing import or any unparsable layer is an error.
#[must_use]
pub fn resolve_registry(
    base: Option<&Path>,
    pack: Option<&Path>,
    prompt: Option<&Path>,
    workflow: Option<&Workflow>,
) -> LoadedRegistry {
    let mut loaded = LoadedRegistry::default();
    if let Some(base) = base {
        apply_path(&mut loaded, base, "--capabilities-base", false);
    }
    if let Some(pack) = pack.filter(|p| base != Some(*p)) {
        apply_path(&mut loaded, pack, "--capabilities", false);
    }
    let base_dir = prompt.and_then(Path::parent).filter(|p| !p.as_os_str().is_empty());
    for import in workflow.iter().flat_map(|w| &w.rules).filter(|i| is_registry_import(i)) {
        let path = base_dir.map_or_else(|| Path::new(import).to_path_buf(), |d| d.join(import));
        apply_path(&mut loaded, &path, &format!("[rules] {import}"), true);
    }
    loaded
}

#[cfg(test)]
mod tests;
