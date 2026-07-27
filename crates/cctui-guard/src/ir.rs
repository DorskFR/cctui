//! Typed, versioned intermediate representation (IR) of a guard workflow.
//!
//! Markdown stays the authoring frontend, but every prompt compiles into this
//! canonical typed model: `Workflow { version, steps }` with enums instead of
//! raw strings. The IR *is* the schema — its JSON Schema (see [`json_schema`])
//! is the published, versioned artifact, and a second frontend can hand-author
//! a `workflow.json` that deserializes straight into [`Workflow`]. Both
//! frontends compile to the same [`Workflow`], and [`Workflow::into_steps`]
//! lowers it back into the [`Step`] map the engine already enforces, so the
//! allow/deny behavior is byte-for-byte the markdown path.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::parser::{JudgeQuestion, ParseError, Step, parse_steps, parse_transitions};

/// The IR schema version. Authored as a `[guard]: v1` header line (or the
/// `version` field of a `workflow.json`); missing ⇒ [`Version::V1`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Version {
    #[default]
    V1,
}

/// Document-level egress default for guarded steps that omit `[network]`.
///
/// Authored as a `[network-default]: allow|deny` header above the first step.
/// Absent ⇒ `deny`: a step-guarded prompt locks egress closed unless a step
/// grants hosts (or `[network]: *`). `allow` restores the legacy open behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NetworkDefault {
    Allow,
    Deny,
}

/// A capability rule (`[allowed]` / `[disallowed]`).
///
/// A wildcard, an explicit keyword/tool-set list, or unrestricted. Matches the
/// markdown semantics exactly — empty is unrestricted, `*` is the wildcard,
/// everything else is a comma-separated list of keywords and tool-set names
/// expanded at check time.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    /// No restriction — an empty `[allowed]`/`[disallowed]` line.
    #[default]
    Unrestricted,
    /// The `*` wildcard: matches every tool.
    Wildcard,
    /// An explicit list of keywords and/or tool-set names.
    List(Vec<String>),
}

impl Rule {
    /// Compile a raw `[allowed]`/`[disallowed]` value into a typed [`Rule`].
    #[must_use]
    pub fn from_raw(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Self::Unrestricted;
        }
        if trimmed == "*" {
            return Self::Wildcard;
        }
        Self::List(
            trimmed
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        )
    }

    /// Lower the [`Rule`] back into the raw string the engine's keyword parser
    /// consumes (`parse_keywords`). Semantically identical to the authored value.
    #[must_use]
    pub fn to_raw(&self) -> String {
        match self {
            Self::Unrestricted => String::new(),
            Self::Wildcard => "*".to_string(),
            Self::List(items) => items.join(", "),
        }
    }
}

/// A typed `[transition]`: the valid next step numbers and whether `Exit` is an
/// authored target (Exit is always allowed regardless, per the engine).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Transition {
    /// Numeric next steps, in authored order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<u32>,
    /// Whether `Exit` appears in the authored transition list.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exit: bool,
}

impl Transition {
    /// Compile a raw `[transition]` value into a typed [`Transition`].
    #[must_use]
    pub fn from_raw(raw: &str) -> Self {
        let (to, exit) = parse_transitions(raw);
        Self { to, exit }
    }

    /// Lower back into the raw `[transition]` string (`parse_transitions` reads
    /// only the digit runs and the presence of `exit`, so this is equivalent).
    #[must_use]
    pub fn to_raw(&self) -> String {
        let mut parts: Vec<String> = self.to.iter().map(u32::to_string).collect();
        if self.exit {
            parts.push("Exit".to_string());
        }
        parts.join(", ")
    }
}

/// One compiled workflow step: the typed equivalent of a parsed [`Step`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowStep {
    /// The step number (`# Step N`).
    pub id: u32,
    /// The step title (text after `Step N:`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// The authoritative prose body re-injected on transition.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// Tools permitted in this step.
    #[serde(default, skip_serializing_if = "is_unrestricted")]
    pub allowed: Rule,
    /// Tools denied in this step.
    #[serde(default, skip_serializing_if = "is_unrestricted")]
    pub disallowed: Rule,
    /// Network-set names granted for the step's egress policy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network: Vec<String>,
    /// The valid transitions out of the step.
    pub transition: Transition,
    /// Optional deterministic completion `[gate]` shell command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
    /// Opt-in `[compact]` directive.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub compact: bool,
    /// Optional `[llmjudge]` acceptance questions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub judge: Vec<JudgeQuestion>,
}

const fn is_unrestricted(rule: &Rule) -> bool {
    matches!(rule, Rule::Unrestricted)
}

impl WorkflowStep {
    fn from_step(id: u32, step: &Step) -> Self {
        let gate = step.gate.trim();
        Self {
            id,
            title: step.title.clone(),
            body: step.body.clone(),
            allowed: Rule::from_raw(&step.allowed),
            disallowed: Rule::from_raw(&step.disallowed),
            network: step
                .network
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            transition: Transition::from_raw(&step.transition),
            gate: if gate.is_empty() { None } else { Some(gate.to_string()) },
            compact: step.compact,
            judge: step.llmjudge.clone(),
        }
    }

    fn into_step(self) -> Step {
        Step {
            title: self.title,
            allowed: self.allowed.to_raw(),
            disallowed: self.disallowed.to_raw(),
            transition: self.transition.to_raw(),
            network: self.network.join(", "),
            body: self.body,
            gate: self.gate.unwrap_or_default(),
            compact: self.compact,
            llmjudge: self.judge,
        }
    }
}

/// The canonical compiled workflow: a version tag plus the ordered steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Workflow {
    /// The IR schema version.
    #[serde(default)]
    pub version: Version,
    /// Document-level egress default for guarded steps without `[network]`.
    /// Absent ⇒ deny.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_default: Option<NetworkDefault>,
    /// The workflow steps, ordered by step number.
    pub steps: Vec<WorkflowStep>,
}

impl Workflow {
    /// Compile prompt markdown into the typed IR.
    ///
    /// Reuses the markdown step parser, then reads the optional `[guard]: vN`
    /// header for the version. The step semantics are unchanged; only the shape
    /// (typed enums vs raw strings) differs.
    pub fn compile(markdown: &str) -> Result<Self, ParseError> {
        let steps = parse_steps(markdown)?;
        let version = parse_version_header(markdown)?;
        let network_default = parse_network_default_header(markdown)?;
        let steps = steps.iter().map(|(id, step)| WorkflowStep::from_step(*id, step)).collect();
        Ok(Self { version, network_default, steps })
    }

    /// Load the IR from a machine-authored `workflow.json` document.
    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        serde_json::from_str(json)
    }

    /// Lower the IR back into the `Step` map the engine enforces. The lowering
    /// is semantically lossless — the engine's allow/deny decisions are
    /// identical to the ones it makes from the markdown parser directly.
    #[must_use]
    pub fn into_steps(self) -> BTreeMap<u32, Step> {
        self.steps.into_iter().map(|s| (s.id, s.into_step())).collect()
    }
}

/// Read the `[guard]: vN` header line (case-insensitive, `v` optional) that sits
/// above the first `# Step` heading. Missing ⇒ [`Version::V1`]. An unknown
/// version is a parse error.
fn parse_version_header(markdown: &str) -> Result<Version, ParseError> {
    for line in markdown.lines() {
        let stripped = line.trim();
        let lower = stripped.to_ascii_lowercase();
        if lower.starts_with("[guard]") {
            let value = stripped.split_once(':').map_or("", |(_, v)| v.trim());
            let digits = value.trim_start_matches(['v', 'V']).trim();
            return match digits {
                "1" => Ok(Version::V1),
                other => Err(ParseError {
                    step: 0,
                    message: format!(
                        "[guard] header declares unsupported IR version '{other}' (supported: v1)"
                    ),
                }),
            };
        }
        // A `[guard]` header only counts before the first step heading; once
        // steps begin, a bracket line is step content, not the workflow header.
        if stripped.starts_with('#') {
            break;
        }
    }
    Ok(Version::V1)
}

/// Read the optional `[network-default]: allow|deny` header above the first step
/// heading. Missing ⇒ `None` (engine treats it as deny). An unknown value is a
/// parse error.
fn parse_network_default_header(markdown: &str) -> Result<Option<NetworkDefault>, ParseError> {
    for line in markdown.lines() {
        let stripped = line.trim();
        let lower = stripped.to_ascii_lowercase();
        if lower.starts_with("[network-default]") {
            let value = stripped.split_once(':').map_or("", |(_, v)| v.trim()).to_ascii_lowercase();
            return match value.as_str() {
                "allow" => Ok(Some(NetworkDefault::Allow)),
                "deny" => Ok(Some(NetworkDefault::Deny)),
                other => Err(ParseError {
                    step: 0,
                    message: format!(
                        "[network-default] header must be 'allow' or 'deny', got '{other}'"
                    ),
                }),
            };
        }
        if stripped.starts_with('#') {
            break;
        }
    }
    Ok(None)
}

/// The published JSON Schema for the versioned [`Workflow`] IR.
#[must_use]
pub fn json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Workflow))
        .expect("Workflow schema serializes to JSON")
}
